//! D3/D5: the write-ahead executor. A write command commits its row
//! `dispatching` and calls [`spawn`]; this sends the write and commits the
//! D5 dispatch outcome.
//!
//! The order is the safety argument:
//!
//! 1. Every local precondition: the Printer, its credential, the
//!    capability object, and, for an upload, `ContentStore::open_verified`.
//!    A failure sends nothing and commits `failed { neverSent }`.
//! 2. `mark_sent` commits `dispatched_at`. **Nothing is sent unless it
//!    returned `Ok`.** If it fails, the executor commits `failed
//!    { neverSent }`, or, if that commit fails too, leaves the row for
//!    startup recovery (which also makes it `neverSent`).
//! 3. The write, then its classification (D5's dispatch table), then one
//!    commit.
//!
//! A panic before `mark_sent` commits `failed { neverSent }`; after it,
//! `uncertain` (`unexpectedResponse`). Which one is decided by the row's
//! own `dispatched_at`, not by in-memory state. The executor never sends a
//! second write.

use std::io::Read;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::FutureExt;
use tokio::io::{AsyncRead, ReadBuf};

use crate::connections::capabilities::{
    ArtifactStaging, CommandFailure, HostOperationFailureCode, HostStateQuery, InconclusiveReason,
    LocateOutcome, PrintControl, StagedArtifact,
};
use crate::library::content::VerifiedReader;

use super::reconciler::{self, Decision};
use super::repository::{self, Outcome};
use super::{
    FaultPoint, HostOperation, HostOperationFailure, HostOperationKind, HostOperationResolution,
    HostOperationServices, HostOperationState, MarkSentFault,
};

/// Runs row `id`'s dispatch in the background.
pub(crate) fn spawn<R: tauri::Runtime>(services: &Arc<HostOperationServices<R>>, id: String) {
    let services = Arc::clone(services);
    tauri::async_runtime::spawn(async move {
        let ran = AssertUnwindSafe(dispatch(&services, &id))
            .catch_unwind()
            .await;
        if ran.is_err() {
            after_panic(&services, &id);
        }
    });
}

fn never_sent() -> Outcome {
    Outcome::Failed {
        failure: HostOperationFailure::for_code(HostOperationFailureCode::NeverSent),
    }
}

fn uncertain(reason: InconclusiveReason, no_longer_pending: bool) -> Outcome {
    Outcome::Uncertain {
        reason,
        no_longer_pending,
    }
}

/// What the executor built before `mark_sent`.
enum Prepared {
    Upload {
        staging: Box<dyn ArtifactStaging>,
        artifact: StagedArtifact,
        body: Option<Box<dyn AsyncRead + Send + Unpin>>,
    },
    Start {
        control: Box<dyn PrintControl>,
        host_path: String,
    },
    Control {
        control: Box<dyn PrintControl>,
        host_state: Box<dyn HostStateQuery>,
        kind: HostOperationKind,
        host_path: String,
    },
}

async fn dispatch<R: tauri::Runtime>(services: &Arc<HostOperationServices<R>>, id: &str) {
    let Ok(Some(row)) = services.load(id) else {
        return;
    };
    if row.state != HostOperationState::Dispatching || row.dispatched_at.is_some() {
        return;
    }
    if services.hit(FaultPoint::BeforeMarkSent) {
        return;
    }
    let Some(mut prepared) = prepare(services, &row) else {
        services.commit_outcome(id, never_sent());
        return;
    };
    if !mark_sent(services, id) {
        return;
    }
    if services.hit(FaultPoint::AfterMarkSentBeforeSend) {
        return;
    }
    let answer = send(&mut prepared).await;
    if services.hit(FaultPoint::AfterSend) {
        return;
    }
    let outcome = classify(services, prepared, answer).await;
    if services.hit(FaultPoint::AfterResponseBeforeCommit) {
        return;
    }
    services.commit_outcome(id, outcome);
}

/// D3's local preconditions. `None` means one failed and nothing may be
/// sent.
fn prepare<R: tauri::Runtime>(
    services: &HostOperationServices<R>,
    row: &HostOperation,
) -> Option<Prepared> {
    let printer = services.load_printer(&row.printer_id).ok()??;
    let config = services.endpoint_config(&row.endpoint, &printer);
    let key = services.credential(&printer).ok()?;
    match row.kind {
        HostOperationKind::Upload => {
            let staging = services.factory.staging(&config, key)?;
            let sha256 = row.gcode_sha256.clone()?;
            let size = u64::try_from(row.gcode_size?).ok()?;
            let reader = services.content.open_verified(&sha256).ok()?;
            Some(Prepared::Upload {
                staging,
                artifact: StagedArtifact {
                    host_path: row.host_path.clone(),
                    sha256,
                    size,
                },
                body: Some(Box::new(ChannelReader::spawn(reader))),
            })
        }
        HostOperationKind::Start => Some(Prepared::Start {
            control: services.factory.control(&config, key)?,
            host_path: row.host_path.clone(),
        }),
        HostOperationKind::Pause | HostOperationKind::Resume | HostOperationKind::Cancel => {
            Some(Prepared::Control {
                control: services.factory.control(&config, key.clone())?,
                host_state: services.factory.host_state(&config, key)?,
                kind: row.kind,
                host_path: row.host_path.clone(),
            })
        }
    }
}

/// D3's invariant: `true` only when `dispatched_at` committed. On `false`
/// the row is `failed { neverSent }` or left `dispatching` for startup
/// recovery, and nothing may be sent.
fn mark_sent<R: tauri::Runtime>(services: &Arc<HostOperationServices<R>>, id: &str) -> bool {
    if let Some((fault, fired)) = services.take_mark_sent_fault() {
        if fault == MarkSentFault::Fails {
            services.commit_outcome(id, never_sent());
        }
        let _ = fired.try_send(());
        return false;
    }
    match services
        .storage
        .write_repo(|tx| repository::mark_sent(tx, id))
    {
        Ok(row) => {
            services.publish(std::slice::from_ref(&row));
            true
        }
        Err(_) => {
            // If this fails too, startup recovery finds the row unsent.
            services.commit_outcome(id, never_sent());
            false
        }
    }
}

async fn send(prepared: &mut Prepared) -> Result<(), CommandFailure> {
    match prepared {
        Prepared::Upload {
            staging,
            artifact,
            body,
        } => {
            let Some(body) = body.take() else {
                return Err(CommandFailure::Indeterminate {
                    reason: InconclusiveReason::UnexpectedResponse,
                    no_longer_pending: false,
                });
            };
            staging.upload(artifact, body).await
        }
        Prepared::Start { control, host_path } => control.start(host_path).await,
        Prepared::Control { control, kind, .. } => match kind {
            HostOperationKind::Pause => control.pause().await,
            HostOperationKind::Resume => control.resume().await,
            _ => control.cancel().await,
        },
    }
}

/// D5's dispatch table.
async fn classify<R: tauri::Runtime>(
    services: &HostOperationServices<R>,
    prepared: Prepared,
    answer: Result<(), CommandFailure>,
) -> Outcome {
    match answer {
        Err(CommandFailure::Definitive(code)) => Outcome::Failed {
            failure: HostOperationFailure::for_code(code),
        },
        Err(CommandFailure::Indeterminate {
            reason,
            no_longer_pending,
        }) => uncertain(reason, no_longer_pending),
        Ok(()) => match prepared {
            // A 201 is never proof by itself (D4): only `Matches` is.
            Prepared::Upload {
                staging, artifact, ..
            } => match staging.locate(&artifact).await {
                Ok(LocateOutcome::Matches) => Outcome::Succeeded {
                    resolution: HostOperationResolution::ArtifactVerified { reconciled: false },
                },
                Ok(LocateOutcome::Absent | LocateOutcome::Differs { .. }) => {
                    uncertain(InconclusiveReason::UploadSettling, false)
                }
                Err(_) => uncertain(InconclusiveReason::IdentityCheckFailed, false),
            },
            Prepared::Start { .. } => Outcome::Succeeded {
                resolution: HostOperationResolution::StartAccepted,
            },
            Prepared::Control {
                host_state,
                kind,
                host_path,
                ..
            } => verify(services, host_state.as_ref(), kind, &host_path).await,
        },
    }
}

/// D5's verification window: pause, resume, and cancel answer `ok` even
/// when nothing happened, so only the observed effect proves them.
async fn verify<R: tauri::Runtime>(
    services: &HostOperationServices<R>,
    host_state: &dyn HostStateQuery,
    kind: HostOperationKind,
    host_path: &str,
) -> Outcome {
    let poll = services.timings.verify_poll_interval;
    let deadline = tokio::time::Instant::now() + services.timings.verify_window;
    let mut no_longer_pending = false;
    loop {
        let reason = match reconciler::observe_control(host_state, kind, host_path, false).await {
            Decision::Applied(resolution) => return Outcome::Succeeded { resolution },
            Decision::Inconclusive {
                reason: observed,
                no_longer_pending: restarted,
            } => {
                no_longer_pending |= restarted;
                match observed {
                    InconclusiveReason::HostUnreachable | InconclusiveReason::HostNotReady => {
                        observed
                    }
                    _ => InconclusiveReason::EffectNotObserved,
                }
            }
            Decision::NotApplied(_) => InconclusiveReason::EffectNotObserved,
        };
        if tokio::time::Instant::now() + poll > deadline {
            return uncertain(reason, no_longer_pending);
        }
        tokio::time::sleep(poll).await;
    }
}

/// D5: a panic after `mark_sent` is `uncertain`, before it `neverSent`.
fn after_panic<R: tauri::Runtime>(services: &Arc<HostOperationServices<R>>, id: &str) {
    let Ok(Some(row)) = services.load(id) else {
        return;
    };
    if row.state != HostOperationState::Dispatching {
        return;
    }
    let outcome = if row.dispatched_at.is_some() {
        uncertain(InconclusiveReason::UnexpectedResponse, false)
    } else {
        never_sent()
    };
    services.commit_outcome(id, outcome);
}

/// The upload body: a thread reads the verified blob and hands chunks over
/// a channel, so the blob is streamed, never buffered whole. A hash
/// mismatch at the end arrives as a read error, which aborts the body.
struct ChannelReader {
    chunks: tokio::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>,
    pending: Vec<u8>,
    offset: usize,
}

const CHUNK: usize = 64 * 1024;

impl ChannelReader {
    fn spawn(mut reader: VerifiedReader) -> Self {
        let (sender, chunks) = tokio::sync::mpsc::channel(4);
        std::thread::spawn(move || loop {
            let mut buffer = vec![0u8; CHUNK];
            let item = match reader.read(&mut buffer) {
                Ok(0) => return,
                Ok(read) => {
                    buffer.truncate(read);
                    Ok(buffer)
                }
                Err(error) => Err(error),
            };
            let failed = item.is_err();
            if sender.blocking_send(item).is_err() || failed {
                return;
            }
        });
        Self {
            chunks,
            pending: Vec::new(),
            offset: 0,
        }
    }
}

impl AsyncRead for ChannelReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            if self.offset < self.pending.len() {
                let available = &self.pending[self.offset..];
                let count = available.len().min(buffer.remaining());
                buffer.put_slice(&available[..count]);
                self.offset += count;
                return Poll::Ready(Ok(()));
            }
            match self.chunks.poll_recv(context) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.pending = chunk;
                    self.offset = 0;
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
