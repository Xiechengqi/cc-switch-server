use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{mpsc, oneshot, Notify};
use tracing::{debug, warn};

use super::history::HistoryBuffer;
use super::options::TerminalRuntimeOptions;

pub(crate) enum SessionCommand {
    Input(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    Attach {
        client_id: u64,
        output_tx: mpsc::Sender<Vec<u8>>,
        replay_tx: oneshot::Sender<Vec<Vec<u8>>>,
    },
    Detach {
        client_id: u64,
    },
    Kill,
}

pub(crate) struct TerminalSession {
    pub owner: String,
    #[allow(dead_code)]
    pub created_at: Instant,
    pub generation: u64,
    cmd_tx: mpsc::UnboundedSender<SessionCommand>,
    pub dead: Arc<AtomicBool>,
    pub foreground_attached: Arc<AtomicBool>,
    pub last_detach_at: Arc<Mutex<Option<Instant>>>,
    idle_cancel: Arc<Notify>,
}

impl std::fmt::Debug for TerminalSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalSession")
            .field("owner", &self.owner)
            .field("generation", &self.generation)
            .field("dead", &self.dead.load(Ordering::Relaxed))
            .finish()
    }
}

impl TerminalSession {
    pub(crate) fn spawn(
        owner: String,
        generation: u64,
        options: TerminalRuntimeOptions,
    ) -> anyhow::Result<Arc<Self>> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let program = options
            .shell
            .first()
            .cloned()
            .unwrap_or_else(|| "sh".to_string());
        let mut cmd = CommandBuilder::new(&program);
        for arg in options.shell.iter().skip(1) {
            cmd.arg(arg);
        }
        cmd.cwd(&options.cwd);
        cmd.env("CC_SWITCH_WEB_TERMINAL", "1");
        cmd.env("TERM", "xterm-256color");

        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| anyhow::anyhow!("clone pty reader: {error}"))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|error| anyhow::anyhow!("take pty writer: {error}"))?;
        let master: Box<dyn MasterPty + Send> = pair.master;

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();
        let history = Arc::new(Mutex::new(HistoryBuffer::new(options.history_bytes)));
        let dead = Arc::new(AtomicBool::new(false));
        let foreground_attached = Arc::new(AtomicBool::new(false));
        let last_detach_at = Arc::new(Mutex::new(None));
        let idle_cancel = Arc::new(Notify::new());
        let live_queue_cap = options.live_queue_cap.max(1);
        let (output_bridge_tx, mut output_bridge_rx) = mpsc::channel::<Vec<u8>>(live_queue_cap);
        let read_buf_bytes = options.read_buf_bytes.max(1024);
        let permit_write = options.permit_write;
        let replay_chunk_bytes = options.replay_chunk_bytes;

        let dead_reader = Arc::clone(&dead);
        let history_reader = Arc::clone(&history);
        thread::Builder::new()
            .name("web-terminal-pty-read".into())
            .spawn(move || {
                let mut buf = vec![0_u8; read_buf_bytes];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = buf[..n].to_vec();
                            if let Ok(mut guard) = history_reader.lock() {
                                guard.append(&chunk);
                            }
                            let _ = output_bridge_tx.try_send(chunk);
                        }
                        Err(error) => {
                            debug!(error = %error, "web terminal pty read ended");
                            break;
                        }
                    }
                }
                dead_reader.store(true, Ordering::SeqCst);
            })?;

        let dead_for_loop = Arc::clone(&dead);
        let foreground_flag = Arc::clone(&foreground_attached);
        let last_detach = Arc::clone(&last_detach_at);
        let idle_cancel_for_loop = Arc::clone(&idle_cancel);
        tokio::spawn(async move {
            let mut foreground: Option<(u64, mpsc::Sender<Vec<u8>>)> = None;
            loop {
                tokio::select! {
                    biased;
                    cmd = cmd_rx.recv() => {
                        let Some(cmd) = cmd else { break; };
                        match cmd {
                            SessionCommand::Input(bytes) => {
                                if !permit_write || bytes.is_empty() {
                                    continue;
                                }
                                if let Err(error) = writer.write_all(&bytes) {
                                    warn!(error = %error, "web terminal pty write failed");
                                } else {
                                    let _ = writer.flush();
                                }
                            }
                            SessionCommand::Resize { cols, rows } => {
                                let cols = cols.max(2);
                                let rows = rows.max(1);
                                if let Err(error) = master.resize(PtySize {
                                    rows,
                                    cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                }) {
                                    warn!(error = %error, "web terminal resize failed");
                                }
                            }
                            SessionCommand::Attach { client_id, output_tx, replay_tx } => {
                                let replay = history
                                    .lock()
                                    .map(|guard| guard.snapshot_chunks(replay_chunk_bytes))
                                    .unwrap_or_default();
                                let _ = replay_tx.send(replay);
                                foreground = Some((client_id, output_tx));
                                foreground_flag.store(true, Ordering::SeqCst);
                                if let Ok(mut guard) = last_detach.lock() {
                                    *guard = None;
                                }
                            }
                            SessionCommand::Detach { client_id } => {
                                if foreground.as_ref().is_some_and(|(id, _)| *id == client_id) {
                                    foreground = None;
                                    foreground_flag.store(false, Ordering::SeqCst);
                                    if let Ok(mut guard) = last_detach.lock() {
                                        *guard = Some(Instant::now());
                                    }
                                    idle_cancel_for_loop.notify_waiters();
                                }
                            }
                            SessionCommand::Kill => {
                                let _ = child.kill();
                                dead_for_loop.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                    chunk = output_bridge_rx.recv() => {
                        let Some(chunk) = chunk else {
                            dead_for_loop.store(true, Ordering::SeqCst);
                            break;
                        };
                        if let Some((_, tx)) = foreground.as_ref() {
                            if tx.send(chunk).await.is_err() {
                                foreground = None;
                                foreground_flag.store(false, Ordering::SeqCst);
                                if let Ok(mut guard) = last_detach.lock() {
                                    *guard = Some(Instant::now());
                                }
                                idle_cancel_for_loop.notify_waiters();
                            }
                        }
                    }
                }

                match child.try_wait() {
                    Ok(Some(_)) => {
                        dead_for_loop.store(true, Ordering::SeqCst);
                        if foreground.is_none() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        dead_for_loop.store(true, Ordering::SeqCst);
                        if foreground.is_none() {
                            break;
                        }
                    }
                }
            }
            let _ = child.kill();
            dead_for_loop.store(true, Ordering::SeqCst);
            foreground_flag.store(false, Ordering::SeqCst);
        });

        Ok(Arc::new(Self {
            owner,
            created_at: Instant::now(),
            generation,
            cmd_tx,
            dead,
            foreground_attached,
            last_detach_at,
            idle_cancel,
        }))
    }

    pub(crate) fn request(&self, cmd: SessionCommand) -> Result<(), ()> {
        self.cmd_tx.send(cmd).map_err(|_| ())
    }

    pub(crate) fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    pub(crate) fn has_foreground(&self) -> bool {
        self.foreground_attached.load(Ordering::SeqCst)
    }

    pub(crate) fn cancel_idle_timer(&self) {
        self.idle_cancel.notify_waiters();
    }

    pub(crate) fn idle_notify(&self) -> Arc<Notify> {
        Arc::clone(&self.idle_cancel)
    }

    pub(crate) fn kill(&self) {
        let _ = self.request(SessionCommand::Kill);
    }
}

static CLIENT_SEQ: AtomicU64 = AtomicU64::new(1);
static GENERATION_SEQ: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_client_id() -> u64 {
    CLIENT_SEQ.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn next_generation() -> u64 {
    GENERATION_SEQ.fetch_add(1, Ordering::Relaxed)
}
