use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::oneshot;
use tracing::warn;

use glossa_app::level::{level_percent, record_peak};
use glossa_app::{ports::OverlayPort, AppError};
use glossa_core::AppConfig;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(250);

enum OverlayCommand {
    ShowRecording,
    ShowProcessing,
    Hide,
}

/// Best-effort recording and processing indicator isolated from the daemon's backend threads.
pub struct BestEffortOverlayPort {
    worker: Option<OverlayWorker>,
}

struct OverlayWorker {
    command_tx: mpsc::Sender<(OverlayCommand, oneshot::Sender<()>)>,
    thread_handle: JoinHandle<()>,
}

impl BestEffortOverlayPort {
    #[must_use]
    pub fn new(config: &AppConfig) -> Self {
        let worker = if config.ui.overlay {
            start_worker()
        } else {
            None
        };
        Self::with_worker(worker)
    }

    fn with_worker(worker: Option<OverlayWorker>) -> Self {
        Self { worker }
    }

    async fn send_command(&self, command: OverlayCommand) {
        let Some(worker) = &self.worker else {
            return;
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        if worker.command_tx.send((command, reply_tx)).is_err() {
            warn!("recording overlay worker is unavailable");
            return;
        }
        let _ = reply_rx.await;
    }
}

impl Drop for BestEffortOverlayPort {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            drop(worker.command_tx);
            let _ = worker.thread_handle.join();
        }
    }
}

#[async_trait]
impl OverlayPort for BestEffortOverlayPort {
    async fn show_recording(&self) -> Result<(), AppError> {
        // The worker reads the process-global peak, so reset it here;
        // otherwise the previous cycle's final level replays into a new recording.
        record_peak(0.0);
        self.send_command(OverlayCommand::ShowRecording).await;
        Ok(())
    }

    async fn show_processing(&self) -> Result<(), AppError> {
        self.send_command(OverlayCommand::ShowProcessing).await;
        Ok(())
    }

    async fn hide(&self) -> Result<(), AppError> {
        self.send_command(OverlayCommand::Hide).await;
        Ok(())
    }
}

#[cfg(windows)]
fn start_worker() -> Option<OverlayWorker> {
    let (command_tx, command_rx) = mpsc::channel();
    match thread::Builder::new()
        .name("glossa-overlay".into())
        .spawn(move || run_pill_worker(command_rx))
    {
        Ok(thread_handle) => Some(OverlayWorker {
            command_tx,
            thread_handle,
        }),
        Err(error) => {
            warn!(error = %error, "failed to spawn recording overlay worker");
            None
        }
    }
}

#[cfg(not(windows))]
fn start_worker() -> Option<OverlayWorker> {
    warn!("recording overlay requires Windows; the indicator stays disabled");
    None
}

/// Drives the indicator window: one worker thread owns the window, renders a
/// frame per poll interval, and destroys the window on hide or disconnect.
#[cfg(windows)]
fn run_pill_worker(command_rx: mpsc::Receiver<(OverlayCommand, oneshot::Sender<()>)>) {
    use super::pill::{GdiPlusGuard, PillMode, PillWindow};

    let _gdiplus = match GdiPlusGuard::new() {
        Ok(guard) => guard,
        Err(error) => {
            warn!(error = %error, "recording overlay stays disabled");
            return;
        }
    };
    let mut pill: Option<PillWindow> = None;
    let mut recording = false;
    loop {
        let poll_interval = if pill.is_some() {
            EVENT_POLL_INTERVAL
        } else {
            IDLE_POLL_INTERVAL
        };
        match command_rx.recv_timeout(poll_interval) {
            Ok((
                mode @ (OverlayCommand::ShowRecording | OverlayCommand::ShowProcessing),
                reply,
            )) => {
                recording = matches!(mode, OverlayCommand::ShowRecording);
                let mode = if recording {
                    PillMode::Recording
                } else {
                    PillMode::Processing
                };
                match pill.as_mut() {
                    Some(window) => window.set_mode(mode),
                    None => match PillWindow::new(mode) {
                        Ok(window) => pill = Some(window),
                        Err(error) => warn!(error = %error, "failed to show the recording overlay"),
                    },
                }
                let _ = reply.send(());
            }
            Ok((OverlayCommand::Hide, reply)) => {
                pill = None;
                recording = false;
                let _ = reply.send(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Some(window) = pill.as_mut() {
            if recording {
                window.set_target(level_percent());
            }
            window.render_frame();
            if !window.is_alive() {
                warn!("recording overlay window was destroyed; dictation continues");
                pill = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::Mutex;

    use glossa_app::level::{level_percent, record_peak};
    use glossa_core::AppConfig;

    use super::{BestEffortOverlayPort, OverlayCommand};
    use glossa_app::ports::OverlayPort;

    type RecordedCommands = Mutex<Vec<&'static str>>;

    /// Spawns a worker stand-in that records commands and acknowledges them,
    /// standing in for the real window thread which needs a desktop session.
    fn fake_worker(events: std::sync::Arc<RecordedCommands>) -> super::OverlayWorker {
        use tokio::sync::oneshot;

        let (command_tx, command_rx) = mpsc::channel::<(OverlayCommand, oneshot::Sender<()>)>();
        let thread_handle = std::thread::spawn(move || {
            while let Ok((command, reply)) = command_rx.recv() {
                match command {
                    OverlayCommand::ShowRecording => events
                        .lock()
                        .expect("mutex should not be poisoned")
                        .push("show"),
                    OverlayCommand::ShowProcessing => events
                        .lock()
                        .expect("mutex should not be poisoned")
                        .push("processing"),
                    OverlayCommand::Hide => events
                        .lock()
                        .expect("mutex should not be poisoned")
                        .push("hide"),
                }
                let _ = reply.send(());
            }
        });
        super::OverlayWorker {
            command_tx,
            thread_handle,
        }
    }

    #[tokio::test]
    async fn disabled_overlay_should_not_start_a_worker() {
        let mut config = AppConfig::default();
        config.ui.overlay = false;
        let overlay = BestEffortOverlayPort::new(&config);

        assert!(
            overlay.worker.is_none(),
            "disabled overlay started a worker"
        );
        overlay.show_recording().await.expect("disabled show");
        overlay
            .show_processing()
            .await
            .expect("disabled processing");
        overlay.hide().await.expect("disabled hide");
    }

    #[tokio::test]
    async fn recording_commands_should_reach_the_worker_and_hide_should_stop_it() {
        let events = std::sync::Arc::new(RecordedCommands::default());
        let overlay = BestEffortOverlayPort::with_worker(Some(fake_worker(events.clone())));

        overlay.show_recording().await.expect("show");
        overlay.show_processing().await.expect("processing");
        overlay.hide().await.expect("hide");
        drop(overlay);

        assert_eq!(
            *events.lock().expect("mutex should not be poisoned"),
            vec!["show", "processing", "hide"]
        );
    }

    #[tokio::test]
    async fn showing_the_recording_overlay_should_reset_the_peak() {
        let events = std::sync::Arc::new(RecordedCommands::default());
        let overlay = BestEffortOverlayPort::with_worker(Some(fake_worker(events)));

        record_peak(1.0);
        assert_eq!(level_percent(), 100);
        overlay.show_recording().await.expect("show");

        assert_eq!(level_percent(), 0, "stale level must not replay");
    }

    #[tokio::test]
    async fn missing_worker_should_not_fail_recording_controls() {
        let overlay = BestEffortOverlayPort::with_worker(None);
        for _ in 0..2 {
            overlay
                .show_recording()
                .await
                .expect("missing worker must not fail show");
            overlay
                .show_processing()
                .await
                .expect("missing worker must not fail processing");
            overlay
                .hide()
                .await
                .expect("missing worker must not fail hide");
        }
    }
}
