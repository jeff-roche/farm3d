//! Recoverable application bootstrap state.
//!
//! Commands consult this state before touching domain services. A retryable
//! startup failure is retried by exactly one caller; non-retryable failures are
//! returned unchanged for the lifetime of the process.

use std::sync::{Arc, Condvar, Mutex};

use crate::contracts::command::CommandError;

type Retry<T> = dyn Fn() -> Result<Arc<T>, CommandError> + Send + Sync;

enum Phase<T> {
    Starting,
    Ready(Arc<T>),
    Failed {
        error: CommandError,
        generation: u64,
        pending_waiters: usize,
    },
    Retrying {
        generation: u64,
        waiters: usize,
    },
}

pub struct BootstrapState<T> {
    phase: Mutex<Phase<T>>,
    retry: Arc<Retry<T>>,
    retry_completed: Condvar,
}

impl<T> BootstrapState<T> {
    pub fn starting() -> Self {
        Self {
            phase: Mutex::new(Phase::Starting),
            retry: Arc::new(|| Err(CommandError::persistence_unavailable())),
            retry_completed: Condvar::new(),
        }
    }

    pub fn ready_with(value: Arc<T>) -> Self {
        Self {
            phase: Mutex::new(Phase::Ready(value)),
            retry: Arc::new(|| Err(CommandError::internal())),
            retry_completed: Condvar::new(),
        }
    }

    pub fn failed(
        error: CommandError,
        retry: impl Fn() -> Result<Arc<T>, CommandError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            phase: Mutex::new(Phase::Failed {
                error,
                generation: 0,
                pending_waiters: 0,
            }),
            retry: Arc::new(retry),
            retry_completed: Condvar::new(),
        }
    }

    pub fn set_ready(&self, value: Arc<T>) {
        *self
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Phase::Ready(value);
        self.retry_completed.notify_all();
    }

    pub fn set_failed(&self, error: CommandError) {
        *self
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Phase::Failed {
            error,
            generation: 0,
            pending_waiters: 0,
        };
        self.retry_completed.notify_all();
    }

    pub fn ready(&self) -> Result<Arc<T>, CommandError> {
        let generation = {
            let mut phase = self.phase.lock().map_err(|_| CommandError::internal())?;
            loop {
                match &mut *phase {
                    Phase::Ready(value) => return Ok(Arc::clone(value)),
                    Phase::Starting => return Err(CommandError::persistence_unavailable()),
                    Phase::Failed { error, .. } if !error.retryable => return Err(error.clone()),
                    Phase::Failed {
                        error,
                        pending_waiters,
                        ..
                    } if *pending_waiters > 0 => return Err(error.clone()),
                    Phase::Failed { generation, .. } => {
                        let next = generation.saturating_add(1);
                        *phase = Phase::Retrying {
                            generation: next,
                            waiters: 0,
                        };
                        break next;
                    }
                    Phase::Retrying {
                        generation,
                        waiters,
                    } => {
                        let awaited_generation = *generation;
                        *waiters = waiters.saturating_add(1);
                        phase = self
                            .retry_completed
                            .wait(phase)
                            .map_err(|_| CommandError::internal())?;
                        match &mut *phase {
                            Phase::Ready(value) => return Ok(Arc::clone(value)),
                            Phase::Failed {
                                error,
                                generation,
                                pending_waiters,
                            } if *generation == awaited_generation => {
                                *pending_waiters = pending_waiters.saturating_sub(1);
                                return Err(error.clone());
                            }
                            _ => continue,
                        }
                    }
                }
            }
        };

        match (self.retry)() {
            Ok(value) => {
                *self.phase.lock().map_err(|_| CommandError::internal())? =
                    Phase::Ready(Arc::clone(&value));
                self.retry_completed.notify_all();
                Ok(value)
            }
            Err(error) => {
                let mut phase = self.phase.lock().map_err(|_| CommandError::internal())?;
                let waiters = match &*phase {
                    Phase::Retrying {
                        generation: active,
                        waiters,
                    } if *active == generation => *waiters,
                    _ => 0,
                };
                *phase = Phase::Failed {
                    error: error.clone(),
                    generation,
                    pending_waiters: waiters,
                };
                self.retry_completed.notify_all();
                Err(error)
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        self.phase
            .lock()
            .map(|phase| matches!(*phase, Phase::Ready(_)))
            .unwrap_or(false)
    }

    pub fn failure(&self) -> Option<CommandError> {
        self.phase.lock().ok().and_then(|phase| match &*phase {
            Phase::Failed { error, .. } => Some(error.clone()),
            Phase::Starting | Phase::Ready(_) | Phase::Retrying { .. } => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{BootstrapState, Phase};
    use crate::contracts::command::{CommandError, ErrorCode};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_waiters_share_one_failed_retry_result() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&attempts);
        let retry_started = Arc::new(Barrier::new(2));
        let release_retry = Arc::new(Barrier::new(2));
        let started = Arc::clone(&retry_started);
        let release = Arc::clone(&release_retry);
        let bootstrap = Arc::new(BootstrapState::<String>::failed(
            CommandError::persistence_unavailable(),
            move || {
                observed.fetch_add(1, Ordering::SeqCst);
                started.wait();
                release.wait();
                Err(CommandError::persistence_unavailable())
            },
        ));

        let leader_state = Arc::clone(&bootstrap);
        let leader = std::thread::spawn(move || leader_state.ready());
        retry_started.wait();
        let waiters = (0..7)
            .map(|_| {
                let bootstrap = Arc::clone(&bootstrap);
                std::thread::spawn(move || bootstrap.ready())
            })
            .collect::<Vec<_>>();
        loop {
            let registered = match &*bootstrap.phase.lock().unwrap() {
                Phase::Retrying { waiters, .. } => *waiters,
                _ => 0,
            };
            if registered == waiters.len() {
                break;
            }
            std::thread::yield_now();
        }
        release_retry.wait();

        assert_eq!(
            leader.join().unwrap().unwrap_err().code,
            ErrorCode::PersistenceUnavailable
        );
        for waiter in waiters {
            assert_eq!(
                waiter.join().unwrap().unwrap_err().code,
                ErrorCode::PersistenceUnavailable
            );
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
