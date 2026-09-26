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
//!    commit. The commit and its publish hold the Printer's lock, as a
//!    reconcile attempt's do, so events follow commit order.
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
            after_panic(&services, &id).await;
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
    let printer_id = row.printer_id.as_str();
    let Some(mut prepared) = prepare(services, &row) else {
        services.commit_outcome(printer_id, id, never_sent()).await;
        return;
    };
    if !mark_sent(services, printer_id, id).await {
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
    services.commit_outcome(printer_id, id, outcome).await;
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
            let artifact = reconciler::staged_artifact(row)?;
            let reader = services.content.open_verified(&artifact.sha256).ok()?;
            // A blob of the wrong size can't hash right: refuse it now, as
            // `neverSent`, instead of sending bytes the host would reject.
            if reader.size().ok()? != artifact.size {
                return None;
            }
            Some(Prepared::Upload {
                staging,
                artifact,
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
async fn mark_sent<R: tauri::Runtime>(
    services: &Arc<HostOperationServices<R>>,
    printer_id: &str,
    id: &str,
) -> bool {
    if let Some((fault, fired)) = services.take_mark_sent_fault() {
        if fault == MarkSentFault::Fails {
            services.commit_outcome(printer_id, id, never_sent()).await;
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
        Err(error) => {
            super::log_commit_failure(id, "`dispatched_at`", &error);
            // If this fails too (logged), startup recovery finds the row
            // unsent.
            services.commit_outcome(printer_id, id, never_sent()).await;
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
async fn after_panic<R: tauri::Runtime>(services: &Arc<HostOperationServices<R>>, id: &str) {
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
    services.commit_outcome(&row.printer_id, id, outcome).await;
}

/// The upload body: a thread reads the verified blob and hands chunks over
/// a channel, so the blob is streamed, never buffered whole. The thread
/// ends the stream with an explicit end marker. A read error (a hash
/// mismatch at the end) or a thread that stops without the marker (a
/// panic) reaches the body as an error, which aborts the upload rather
/// than ending it as if the file were complete.
struct ChannelReader {
    chunks: tokio::sync::mpsc::Receiver<std::io::Result<Option<Vec<u8>>>>,
    pending: Vec<u8>,
    offset: usize,
    finished: bool,
}

const CHUNK: usize = 64 * 1024;

impl ChannelReader {
    fn spawn(mut reader: impl Read + Send + 'static) -> Self {
        let (sender, chunks) = tokio::sync::mpsc::channel(4);
        std::thread::spawn(move || loop {
            let mut buffer = vec![0u8; CHUNK];
            let item = match reader.read(&mut buffer) {
                Ok(0) => Ok(None),
                Ok(read) => {
                    buffer.truncate(read);
                    Ok(Some(buffer))
                }
                Err(error) => Err(error),
            };
            let last = !matches!(item, Ok(Some(_)));
            if sender.blocking_send(item).is_err() || last {
                return;
            }
        });
        Self {
            chunks,
            pending: Vec::new(),
            offset: 0,
            finished: false,
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
            if self.finished {
                return Poll::Ready(Ok(()));
            }
            match self.chunks.poll_recv(context) {
                Poll::Ready(Some(Ok(Some(chunk)))) => {
                    self.pending = chunk;
                    self.offset = 0;
                }
                Poll::Ready(Some(Ok(None))) => self.finished = true,
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(None) => {
                    return Poll::Ready(Err(std::io::Error::other(
                        "the upload source stopped before the end of the file",
                    )))
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads `reader` to the end through the channel, as reqwest's body
    /// stream would.
    fn read_through_channel(reader: impl Read + Send + 'static) -> std::io::Result<Vec<u8>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let mut channel = ChannelReader::spawn(reader);
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0u8; 1000];
                let mut buffer = ReadBuf::new(&mut chunk);
                std::future::poll_fn(|context| {
                    Pin::new(&mut channel).poll_read(context, &mut buffer)
                })
                .await?;
                if buffer.filled().is_empty() {
                    return Ok(bytes);
                }
                bytes.extend_from_slice(buffer.filled());
            }
        })
    }

    /// Gives its `good` bytes, then fails or panics.
    struct Failing {
        good: std::io::Cursor<Vec<u8>>,
        panic: bool,
    }

    impl Read for Failing {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let read = self.good.read(buffer)?;
            if read > 0 {
                return Ok(read);
            }
            if self.panic {
                // Without the panic hook, so the test output stays quiet.
                std::panic::resume_unwind(Box::new("reader panicked"));
            }
            Err(std::io::Error::other("hash mismatch"))
        }
    }

    #[test]
    fn a_complete_source_streams_every_byte_then_ends() {
        let bytes: Vec<u8> = (0..200_000u32).map(|index| index as u8).collect();
        let read = read_through_channel(std::io::Cursor::new(bytes.clone())).unwrap();
        assert_eq!(read, bytes);
    }

    #[test]
    fn a_source_error_is_a_body_error_not_an_end_of_file() {
        let error = read_through_channel(Failing {
            good: std::io::Cursor::new(vec![7; 5000]),
            panic: false,
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "hash mismatch");
    }

    #[test]
    fn a_panicking_source_is_a_body_error_not_an_end_of_file() {
        let error = read_through_channel(Failing {
            good: std::io::Cursor::new(vec![7; 5000]),
            panic: true,
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("stopped before the end"),
            "{error}"
        );
    }
}
