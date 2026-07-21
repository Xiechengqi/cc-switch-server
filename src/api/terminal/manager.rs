use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::info;

use super::options::TerminalRuntimeOptions;
use super::session::{next_generation, TerminalSession};

#[derive(Clone, Default)]
pub struct OpsTerminalManager {
    session: Arc<Mutex<Option<Arc<TerminalSession>>>>,
}

impl std::fmt::Debug for OpsTerminalManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OpsTerminalManager")
    }
}

#[derive(Debug)]
pub(crate) enum AttachError {
    Busy { owner: String },
    Spawn(String),
}

impl OpsTerminalManager {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) async fn end_session(&self) {
        let session = {
            let mut guard = self.session.lock().await;
            guard.take()
        };
        if let Some(session) = session {
            session.cancel_idle_timer();
            session.kill();
            info!(owner = %session.owner, "web terminal session ended");
        }
    }

    pub(crate) async fn current_owner(&self) -> Option<String> {
        let guard = self.session.lock().await;
        guard.as_ref().and_then(|session| {
            if session.is_dead() {
                None
            } else {
                Some(session.owner.clone())
            }
        })
    }

    pub(crate) async fn attach_or_create(
        &self,
        owner: &str,
        config_dir: &Path,
    ) -> Result<Arc<TerminalSession>, AttachError> {
        self.reap_expired(config_dir).await;
        let options = TerminalRuntimeOptions::resolve(config_dir);

        let mut guard = self.session.lock().await;
        if let Some(existing) = guard.as_ref() {
            if existing.is_dead() {
                existing.kill();
                *guard = None;
            } else if existing.owner != owner {
                return Err(AttachError::Busy {
                    owner: existing.owner.clone(),
                });
            } else {
                existing.cancel_idle_timer();
                return Ok(Arc::clone(existing));
            }
        }

        let generation = next_generation();
        let idle = options.idle_detach;
        let max_lifetime = options.max_lifetime;
        let session = TerminalSession::spawn(owner.to_string(), generation, options)
            .map_err(|error| AttachError::Spawn(error.to_string()))?;
        self.spawn_lifecycle_watchers(Arc::clone(&session), idle, max_lifetime);
        *guard = Some(Arc::clone(&session));
        info!(owner = %owner, generation, "web terminal session created");
        Ok(session)
    }

    fn spawn_lifecycle_watchers(
        &self,
        session: Arc<TerminalSession>,
        idle: Duration,
        max_lifetime: Duration,
    ) {
        let generation = session.generation;
        let idle_notify = session.idle_notify();
        let slot_idle = Arc::clone(&self.session);
        let slot_life = Arc::clone(&self.session);
        let session_idle = Arc::clone(&session);
        let session_life = Arc::clone(&session);

        tokio::spawn(async move {
            loop {
                if session_idle.is_dead() {
                    clear_if_generation(&slot_idle, generation).await;
                    return;
                }
                if session_idle.has_foreground() {
                    idle_notify.notified().await;
                    continue;
                }
                tokio::select! {
                    _ = idle_notify.notified() => {}
                    _ = tokio::time::sleep(idle) => {
                        if !session_idle.has_foreground() && !session_idle.is_dead() {
                            info!(generation, "web terminal idle detach timeout");
                            session_idle.kill();
                            clear_if_generation(&slot_idle, generation).await;
                            return;
                        }
                    }
                }
            }
        });

        tokio::spawn(async move {
            tokio::time::sleep(max_lifetime).await;
            if !session_life.is_dead() {
                info!(generation, "web terminal max lifetime reached");
                session_life.kill();
            }
            clear_if_generation(&slot_life, generation).await;
        });
    }

    async fn reap_expired(&self, config_dir: &Path) {
        let options = TerminalRuntimeOptions::resolve(config_dir);
        let mut guard = self.session.lock().await;
        let Some(session) = guard.as_ref() else {
            return;
        };
        if session.is_dead() {
            *guard = None;
            return;
        }
        let expired_idle = session
            .last_detach_at
            .lock()
            .ok()
            .and_then(|guard| *guard)
            .is_some_and(|at| at.elapsed() >= options.idle_detach && !session.has_foreground());
        if expired_idle {
            session.kill();
            *guard = None;
        }
    }
}

async fn clear_if_generation(slot: &Mutex<Option<Arc<TerminalSession>>>, generation: u64) {
    let mut guard = slot.lock().await;
    if guard
        .as_ref()
        .is_some_and(|session| session.generation == generation)
    {
        if let Some(session) = guard.take() {
            session.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::terminal::session::{next_client_id, SessionCommand};
    use std::time::Duration;
    use tokio::sync::{mpsc, oneshot};
    use tokio::time::timeout;

    #[tokio::test]
    async fn attach_detach_keeps_session_and_replays_output() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-terminal-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CC_SWITCH_TERMINAL_SHELL", "/bin/sh");
        std::env::set_var("CC_SWITCH_TERMINAL_CWD", dir.display().to_string());
        std::env::set_var("CC_SWITCH_TERMINAL_IDLE_DETACH_SECS", "3600");

        let manager = OpsTerminalManager::new();
        let session = manager
            .attach_or_create("ops@example.com", &dir)
            .await
            .expect("spawn terminal");

        let client_a = next_client_id();
        let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(32);
        let (replay_tx, replay_rx) = oneshot::channel();
        session
            .request(SessionCommand::Attach {
                client_id: client_a,
                output_tx,
                replay_tx,
            })
            .unwrap();
        let _ = replay_rx.await.unwrap();

        session
            .request(SessionCommand::Input(
                b"printf 'hello-web-terminal'\n".to_vec(),
            ))
            .unwrap();

        let mut saw_hello = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(chunk)) = timeout(Duration::from_millis(200), output_rx.recv()).await {
                if String::from_utf8_lossy(&chunk).contains("hello-web-terminal") {
                    saw_hello = true;
                    break;
                }
            }
        }
        assert!(saw_hello, "expected live output from shell");

        session
            .request(SessionCommand::Detach {
                client_id: client_a,
            })
            .unwrap();

        // Session should remain alive after detach.
        assert!(!session.is_dead());
        let again = manager
            .attach_or_create("ops@example.com", &dir)
            .await
            .expect("reattach");
        assert_eq!(again.generation, session.generation);

        let client_b = next_client_id();
        let (output_tx_b, _output_rx_b) = mpsc::channel::<Vec<u8>>(8);
        let (replay_tx_b, replay_rx_b) = oneshot::channel();
        again
            .request(SessionCommand::Attach {
                client_id: client_b,
                output_tx: output_tx_b,
                replay_tx: replay_tx_b,
            })
            .unwrap();
        let replay = replay_rx_b.await.unwrap();
        let flat: Vec<u8> = replay.into_iter().flatten().collect();
        assert!(
            String::from_utf8_lossy(&flat).contains("hello-web-terminal"),
            "expected replayed history after reattach"
        );

        manager.end_session().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn second_owner_is_rejected_while_session_alive() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-terminal-busy-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CC_SWITCH_TERMINAL_SHELL", "/bin/sh");
        std::env::set_var("CC_SWITCH_TERMINAL_CWD", dir.display().to_string());
        std::env::set_var("CC_SWITCH_TERMINAL_IDLE_DETACH_SECS", "3600");

        let manager = OpsTerminalManager::new();
        manager
            .attach_or_create("alice@example.com", &dir)
            .await
            .unwrap();
        let err = manager
            .attach_or_create("bob@example.com", &dir)
            .await
            .expect_err("busy");
        assert!(matches!(err, AttachError::Busy { .. }));
        manager.end_session().await;
        let _ = std::fs::remove_dir_all(dir);
    }
}
