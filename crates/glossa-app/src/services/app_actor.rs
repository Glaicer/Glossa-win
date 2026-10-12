use std::{sync::Arc, time::Duration, time::Instant};

use tokio::{sync::mpsc, task::AbortHandle};
use tracing::{error, info, warn};

use glossa_core::{
    AppCommand, AppConfig, AppState, CommandOrigin, PastingState, RecordSpec, SessionId,
};

use crate::{
    machine::{reduce, Action},
    ports::{
        ActiveRecording, AudioCapture, ClipboardWriter, CuePlayer, OverlayPort, PasteBackend,
        SilenceTrimmer, SttClient, TempStore, TextEnhancer, TrayPort, TrayState,
    },
    services::{
        command_router::AppHandle,
        recording_pipeline::{
            spawn_paste_task, spawn_processing_task, CycleOutcome, InternalEvent,
            PipelineDependencies,
        },
        status_store::StatusStore,
    },
    AppError,
};

/// Runtime dependencies owned by the application actor.
#[derive(Clone)]
pub struct AppDependencies {
    pub audio_capture: Arc<dyn AudioCapture>,
    pub trimmer: Arc<dyn SilenceTrimmer>,
    pub cue_player: Arc<dyn CuePlayer>,
    pub stt_client: Arc<dyn SttClient>,
    pub text_enhancer: Arc<dyn TextEnhancer>,
    pub clipboard: Arc<dyn ClipboardWriter>,
    pub paste: Arc<dyn PasteBackend>,
    pub tray: Arc<dyn TrayPort>,
    pub overlay: Arc<dyn OverlayPort>,
    pub temp_store: Arc<dyn TempStore>,
}

/// Main daemon actor that serializes all state transitions.
pub struct AppActor {
    config: Arc<AppConfig>,
    deps: AppDependencies,
    state: AppState,
    active_recording: Option<Box<dyn ActiveRecording>>,
    /// Cancels the pending auto-stop timer for the current recording, if any.
    max_duration_abort: Option<AbortHandle>,
    command_rx: mpsc::UnboundedReceiver<AppCommand>,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
    internal_rx: mpsc::UnboundedReceiver<InternalEvent>,
    status_store: StatusStore,
}

/// Reason why the actor stopped processing commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorExit {
    Restart,
    Shutdown,
}

impl AppActor {
    /// Creates a new actor plus the handle that external components can use.
    #[must_use]
    pub fn new(config: AppConfig, deps: AppDependencies) -> (Self, AppHandle) {
        let config = Arc::new(config);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let status_store = StatusStore::new(config.initial_status());
        let handle = AppHandle::new(command_tx, status_store.clone());
        let actor = Self {
            config,
            deps,
            state: AppState::Idle,
            active_recording: None,
            max_duration_abort: None,
            command_rx,
            internal_tx,
            internal_rx,
            status_store,
        };
        (actor, handle)
    }

    /// Runs the actor until shutdown is requested.
    pub async fn run(mut self) -> Result<ActorExit, AppError> {
        self.deps.temp_store.cleanup_stale_files().await?;
        self.set_tray_state(TrayState::Idle).await;
        self.publish_status();

        loop {
            tokio::select! {
                maybe_command = self.command_rx.recv() => {
                    let Some(command) = maybe_command else {
                        break;
                    };
                    if let Some(exit) = self.handle_command(command).await? {
                        return Ok(exit);
                    }
                }
                maybe_event = self.internal_rx.recv() => {
                    let Some(event) = maybe_event else {
                        break;
                    };
                    self.handle_internal_event(event).await?;
                }
            }
        }

        Ok(ActorExit::Shutdown)
    }

    async fn handle_command(&mut self, command: AppCommand) -> Result<Option<ActorExit>, AppError> {
        info!(state = ?self.state.kind(), command = ?command, "received command");
        let decision = reduce(&self.state, &command)?;
        self.state = decision.next_state;
        self.publish_status();

        for action in decision.actions {
            match action {
                Action::StartRecording { session_id } => self.start_recording(session_id).await?,
                Action::StopRecording { session_id } => self.stop_recording(session_id).await?,
                Action::AbortRecording { session_id } => self.abort_recording(session_id).await,
                Action::SetTrayIdle => self.set_tray_state(TrayState::Idle).await,
                Action::SetTrayRecording => self.set_tray_state(TrayState::Recording).await,
                Action::SetTrayProcessing => self.set_tray_state(TrayState::Processing).await,
                Action::ShowOverlay => self.show_overlay().await,
                Action::ShowProcessingOverlay => self.show_processing_overlay().await,
                Action::HideOverlay => self.hide_overlay().await,
                Action::PlayStartCue => {
                    if let Err(error) = self.deps.cue_player.play_start().await {
                        warn!(error = %error, "failed to play start cue");
                    }
                }
                Action::PlayStopCue => {
                    if let Err(error) = self.deps.cue_player.play_stop().await {
                        warn!(error = %error, "failed to play stop cue");
                    }
                }
                Action::Ignore { reason } => {
                    info!(reason, "ignored command");
                }
                Action::Restart => {
                    self.hide_overlay().await;
                    self.shutdown().await?;
                    return Ok(Some(ActorExit::Restart));
                }
                Action::Shutdown => {
                    self.hide_overlay().await;
                    self.shutdown().await?;
                    return Ok(Some(ActorExit::Shutdown));
                }
            }
        }

        Ok(None)
    }

    async fn handle_internal_event(&mut self, event: InternalEvent) -> Result<(), AppError> {
        match event {
            InternalEvent::ProcessingReady { session_id, text } => {
                if !matches!(
                    self.state,
                    AppState::Processing(glossa_core::ProcessingState { session_id: current })
                        if current == session_id
                ) {
                    return Ok(());
                }

                self.state = AppState::Pasting(PastingState {
                    session_id,
                    text_len: text.len(),
                });
                self.publish_status();
                self.hide_overlay().await;
                spawn_paste_task(
                    self.internal_tx.clone(),
                    self.pipeline_dependencies(),
                    session_id,
                    text,
                );
            }
            InternalEvent::ProcessingFinished {
                session_id,
                outcome,
            } => {
                self.finish_cycle(session_id, outcome).await;
            }
            InternalEvent::PasteFinished {
                session_id,
                outcome,
            } => {
                self.finish_cycle(session_id, outcome).await;
            }
            InternalEvent::MaxDurationReached { session_id } => {
                if !matches!(
                    self.state,
                    AppState::Recording(glossa_core::RecordingState {
                        session_id: current
                    }) if current == session_id
                ) {
                    return Ok(());
                }

                info!(
                    %session_id,
                    max_duration_sec = self.config.audio.max_duration_sec,
                    "max recording duration reached; stopping"
                );
                let _ = self
                    .handle_command(AppCommand::StopRecording {
                        origin: CommandOrigin::Internal,
                    })
                    .await?;
            }
        }
        Ok(())
    }

    async fn abort_recording(&mut self, session_id: SessionId) {
        self.clear_max_duration_timer();
        let abort_error = match self.active_recording.take() {
            Some(recording) => recording.abort().await.err(),
            None => {
                warn!(%session_id, "cancel requested without an active recording");
                None
            }
        };
        let purge_error = self.deps.temp_store.purge_session(session_id).await.err();

        match (abort_error, purge_error) {
            (None, None) => {
                info!(%session_id, "recording cancelled");
            }
            (abort_error, purge_error) => {
                let error_message = match (abort_error, purge_error) {
                    (Some(abort_error), Some(purge_error)) => {
                        format!(
                            "failed to cancel recording for session {session_id}: \
                             abort failed: {abort_error}; purge failed: {purge_error}"
                        )
                    }
                    (Some(abort_error), None) => {
                        format!(
                            "failed to cancel recording for session {session_id}: {abort_error}"
                        )
                    }
                    (None, Some(purge_error)) => {
                        format!(
                            "failed to purge cancelled recording for session {session_id}: \
                             {purge_error}"
                        )
                    }
                    (None, None) => unreachable!("success case handled above"),
                };
                error!(%session_id, error = %error_message, "recording cancel failed");
                let _ = self.deps.tray.show_error(&error_message).await;
            }
        }
    }

    async fn start_recording(&mut self, session_id: SessionId) -> Result<(), AppError> {
        let startup_started_at = Instant::now();
        let path_started_at = Instant::now();
        let path = self
            .deps
            .temp_store
            .create_recording_path(session_id, self.config.audio.format)
            .await?;
        let path_prep_ms = path_started_at.elapsed().as_millis();
        let max_duration_sec = self.config.audio.max_duration_sec;
        let spec = RecordSpec {
            sample_rate_hz: self.config.audio.sample_rate_hz,
            channels: self.config.audio.channels,
            format: self.config.audio.format,
            max_duration_sec,
        };
        let capture_started_at = Instant::now();
        match self
            .deps
            .audio_capture
            .start(session_id, spec, path.as_path())
            .await
        {
            Ok(recording) => {
                self.active_recording = Some(recording);
                self.arm_max_duration_timer(session_id, max_duration_sec);
                info!(
                    %session_id,
                    path = %path,
                    max_duration_sec,
                    path_prep_ms,
                    capture_startup_ms = capture_started_at.elapsed().as_millis(),
                    total_startup_ms = startup_started_at.elapsed().as_millis(),
                    "recording started"
                );
            }
            Err(error) => {
                error!(
                    %session_id,
                    error = %error,
                    path_prep_ms,
                    capture_startup_ms = capture_started_at.elapsed().as_millis(),
                    total_startup_ms = startup_started_at.elapsed().as_millis(),
                    "failed to start recording"
                );
                self.state = AppState::Idle;
                self.publish_status();
                self.set_tray_state(TrayState::Idle).await;
                self.hide_overlay().await;
                let _ = self.deps.temp_store.cleanup_session(session_id).await;
                let _ = self.deps.tray.show_error(&error.to_string()).await;
            }
        }
        Ok(())
    }

    async fn stop_recording(&mut self, session_id: SessionId) -> Result<(), AppError> {
        self.clear_max_duration_timer();
        let Some(recording) = self.active_recording.take() else {
            warn!(%session_id, "stop requested without an active recording");
            self.state = AppState::Idle;
            self.publish_status();
            self.set_tray_state(TrayState::Idle).await;
            self.hide_overlay().await;
            return Ok(());
        };

        match recording.stop().await {
            Ok(audio) => {
                info!(%session_id, duration_ms = audio.duration_ms, path = %audio.path, "recording stopped");
                spawn_processing_task(
                    self.internal_tx.clone(),
                    self.pipeline_dependencies(),
                    audio,
                );
            }
            Err(error) => {
                error!(%session_id, error = %error, "failed to stop recording");
                self.finish_cycle(session_id, CycleOutcome::Failed(error.to_string()))
                    .await;
            }
        }
        Ok(())
    }

    fn arm_max_duration_timer(&mut self, session_id: SessionId, max_duration_sec: u32) {
        self.clear_max_duration_timer();
        let tx = self.internal_tx.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(max_duration_sec))).await;
            let _ = tx.send(InternalEvent::MaxDurationReached { session_id });
        });
        self.max_duration_abort = Some(handle.abort_handle());
    }

    fn clear_max_duration_timer(&mut self) {
        if let Some(handle) = self.max_duration_abort.take() {
            handle.abort();
        }
    }

    async fn finish_cycle(&mut self, session_id: SessionId, outcome: CycleOutcome) {
        match &outcome {
            CycleOutcome::Completed => info!(%session_id, "cycle completed"),
            CycleOutcome::CompletedWithWarning(reason) => {
                warn!(%session_id, reason, "cycle completed with warning");
            }
            CycleOutcome::Failed(reason) => {
                error!(%session_id, reason, "cycle failed");
                let _ = self.deps.tray.show_error(reason).await;
            }
        }

        self.state = AppState::Idle;
        self.publish_status();
        self.set_tray_state(TrayState::Idle).await;
        self.hide_overlay().await;
    }

    async fn shutdown(&mut self) -> Result<(), AppError> {
        self.clear_max_duration_timer();
        if let Some(recording) = self.active_recording.take() {
            let _ = recording.abort().await;
        }
        self.state = AppState::ShuttingDown;
        self.publish_status();
        self.set_tray_state(TrayState::Idle).await;
        Ok(())
    }

    fn publish_status(&self) {
        let mut status = self.config.initial_status();
        status.state = self.state.kind();
        self.status_store.update(status);
    }

    async fn set_tray_state(&self, state: TrayState) {
        if let Err(error) = self.deps.tray.set_state(state).await {
            warn!(error = %error, "failed to update tray state");
        }
    }

    async fn show_overlay(&self) {
        if let Err(error) = self.deps.overlay.show_recording().await {
            warn!(error = %error, "failed to show recording overlay");
        }
    }

    async fn show_processing_overlay(&self) {
        if let Err(error) = self.deps.overlay.show_processing().await {
            warn!(error = %error, "failed to show processing overlay");
        }
    }

    async fn hide_overlay(&self) {
        if let Err(error) = self.deps.overlay.hide().await {
            warn!(error = %error, "failed to hide overlay");
        }
    }

    fn pipeline_dependencies(&self) -> PipelineDependencies {
        PipelineDependencies {
            config: Arc::clone(&self.config),
            trimmer: Arc::clone(&self.deps.trimmer),
            stt_client: Arc::clone(&self.deps.stt_client),
            text_enhancer: Arc::clone(&self.deps.text_enhancer),
            clipboard: Arc::clone(&self.deps.clipboard),
            paste: Arc::clone(&self.deps.paste),
            temp_store: Arc::clone(&self.deps.temp_store),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use camino::{Utf8Path, Utf8PathBuf};

    use glossa_core::{AppStateKind, AudioFormat, CapturedAudio, CommandOrigin};

    use super::*;
    use crate::ports::{
        AudioCapture, ClipboardWriter, CuePlayer, NoopTextEnhancer, PasteBackend, SilenceTrimmer,
        SttClient, TempStore, TrayPort,
    };

    #[derive(Debug)]
    struct FakeAudioCapture {
        started: Arc<Mutex<bool>>,
    }

    #[derive(Debug)]
    struct OrderedAudioCapture {
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    #[derive(Debug)]
    struct TrackingAudioCapture {
        lifecycle: Arc<RecordingLifecycle>,
    }

    struct FakeRecording;

    #[derive(Debug, Default)]
    struct RecordingLifecycle {
        stop_calls: Mutex<Vec<SessionId>>,
        abort_calls: Mutex<Vec<SessionId>>,
    }

    struct TrackingRecording {
        session_id: SessionId,
        lifecycle: Arc<RecordingLifecycle>,
    }

    #[async_trait]
    impl AudioCapture for FakeAudioCapture {
        async fn start(
            &self,
            session_id: SessionId,
            spec: RecordSpec,
            path: &Utf8Path,
        ) -> Result<Box<dyn ActiveRecording>, AppError> {
            let _ = (session_id, spec, path);
            *self.started.lock().expect("mutex should not be poisoned") = true;
            Ok(Box::new(FakeRecording))
        }
    }

    #[async_trait]
    impl AudioCapture for OrderedAudioCapture {
        async fn start(
            &self,
            session_id: SessionId,
            spec: RecordSpec,
            path: &Utf8Path,
        ) -> Result<Box<dyn ActiveRecording>, AppError> {
            let _ = (session_id, spec, path);
            self.events
                .lock()
                .expect("mutex should not be poisoned")
                .push("start-recording");
            Ok(Box::new(FakeRecording))
        }
    }

    #[async_trait]
    impl AudioCapture for TrackingAudioCapture {
        async fn start(
            &self,
            session_id: SessionId,
            spec: RecordSpec,
            path: &Utf8Path,
        ) -> Result<Box<dyn ActiveRecording>, AppError> {
            let _ = (spec, path);
            Ok(Box::new(TrackingRecording {
                session_id,
                lifecycle: Arc::clone(&self.lifecycle),
            }))
        }
    }

    #[async_trait(?Send)]
    impl ActiveRecording for FakeRecording {
        async fn stop(self: Box<Self>) -> Result<CapturedAudio, AppError> {
            Ok(CapturedAudio {
                session_id: SessionId::new(),
                path: Utf8PathBuf::from("C:\\glossa\\test.wav"),
                duration_ms: 500,
                sample_rate_hz: 16_000,
                channels: 1,
            })
        }

        async fn abort(self: Box<Self>) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[async_trait(?Send)]
    impl ActiveRecording for TrackingRecording {
        async fn stop(self: Box<Self>) -> Result<CapturedAudio, AppError> {
            self.lifecycle
                .stop_calls
                .lock()
                .expect("mutex should not be poisoned")
                .push(self.session_id);
            Ok(CapturedAudio {
                session_id: self.session_id,
                path: Utf8PathBuf::from("C:\\glossa\\test.wav"),
                duration_ms: 500,
                sample_rate_hz: 16_000,
                channels: 1,
            })
        }

        async fn abort(self: Box<Self>) -> Result<(), AppError> {
            self.lifecycle
                .abort_calls
                .lock()
                .expect("mutex should not be poisoned")
                .push(self.session_id);
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct FakeTrimmer;

    #[async_trait]
    impl SilenceTrimmer for FakeTrimmer {
        async fn trim(&self, input: &CapturedAudio) -> Result<CapturedAudio, AppError> {
            Ok(input.clone())
        }
    }

    #[derive(Debug, Default)]
    struct FakeCuePlayer;

    #[derive(Debug)]
    struct OrderedCuePlayer {
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    #[derive(Debug)]
    struct TrackingCuePlayer {
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl CuePlayer for FakeCuePlayer {
        async fn play_start(&self) -> Result<(), AppError> {
            Ok(())
        }

        async fn play_stop(&self) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[async_trait]
    impl CuePlayer for OrderedCuePlayer {
        async fn play_start(&self) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("mutex should not be poisoned")
                .push("play-start-cue");
            Ok(())
        }

        async fn play_stop(&self) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[async_trait]
    impl CuePlayer for TrackingCuePlayer {
        async fn play_start(&self) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("mutex should not be poisoned")
                .push("play-start-cue");
            Ok(())
        }

        async fn play_stop(&self) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("mutex should not be poisoned")
                .push("play-stop-cue");
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct FakeSttClient;

    #[derive(Debug, Default)]
    struct TrackingSttClient {
        calls: Mutex<usize>,
    }

    #[async_trait]
    impl SttClient for FakeSttClient {
        fn provider_name(&self) -> &'static str {
            "fake"
        }

        async fn transcribe(&self, _audio: &CapturedAudio) -> Result<String, AppError> {
            Ok("hello".into())
        }
    }

    #[async_trait]
    impl SttClient for TrackingSttClient {
        fn provider_name(&self) -> &'static str {
            "tracking"
        }

        async fn transcribe(&self, _audio: &CapturedAudio) -> Result<String, AppError> {
            let mut calls = self.calls.lock().expect("mutex should not be poisoned");
            *calls += 1;
            Ok("hello".into())
        }
    }

    #[derive(Debug, Default)]
    struct FakeClipboard;

    #[async_trait]
    impl ClipboardWriter for FakeClipboard {
        async fn set_text(&self, _text: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct FakePaste;

    #[async_trait]
    impl PasteBackend for FakePaste {
        async fn paste(&self, _mode: glossa_core::PasteMode) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct FakeTempStore;

    #[derive(Debug, Default)]
    struct TrackingTempStore {
        cleaned_sessions: Mutex<Vec<SessionId>>,
        purged_sessions: Mutex<Vec<SessionId>>,
    }

    #[derive(Debug, Default)]
    struct TrackingTray {
        states: Mutex<Vec<TrayState>>,
        errors: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl TempStore for FakeTempStore {
        async fn create_recording_path(
            &self,
            session_id: SessionId,
            format: AudioFormat,
        ) -> Result<Utf8PathBuf, AppError> {
            Ok(Utf8PathBuf::from(format!(
                r"C:\glossa\{session_id}.{}",
                format.extension()
            )))
        }

        async fn cleanup_session(&self, _session_id: SessionId) -> Result<(), AppError> {
            Ok(())
        }

        async fn purge_session(&self, _session_id: SessionId) -> Result<(), AppError> {
            Ok(())
        }

        async fn cleanup_stale_files(&self) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[async_trait]
    impl TempStore for TrackingTempStore {
        async fn create_recording_path(
            &self,
            session_id: SessionId,
            format: AudioFormat,
        ) -> Result<Utf8PathBuf, AppError> {
            Ok(Utf8PathBuf::from(format!(
                r"C:\glossa\{session_id}.{}",
                format.extension()
            )))
        }

        async fn cleanup_session(&self, session_id: SessionId) -> Result<(), AppError> {
            self.cleaned_sessions
                .lock()
                .expect("mutex should not be poisoned")
                .push(session_id);
            Ok(())
        }

        async fn purge_session(&self, session_id: SessionId) -> Result<(), AppError> {
            self.purged_sessions
                .lock()
                .expect("mutex should not be poisoned")
                .push(session_id);
            Ok(())
        }

        async fn cleanup_stale_files(&self) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[async_trait]
    impl TrayPort for TrackingTray {
        async fn set_state(&self, state: TrayState) -> Result<(), AppError> {
            self.states
                .lock()
                .expect("mutex should not be poisoned")
                .push(state);
            Ok(())
        }

        async fn set_shortcut_description(
            &self,
            _description: Option<&str>,
        ) -> Result<(), AppError> {
            Ok(())
        }

        async fn show_error(&self, message: &str) -> Result<(), AppError> {
            self.errors
                .lock()
                .expect("mutex should not be poisoned")
                .push(message.to_owned());
            Ok(())
        }
    }

    #[tokio::test]
    async fn actor_should_expose_initial_status() {
        let deps = AppDependencies {
            audio_capture: Arc::new(FakeAudioCapture {
                started: Arc::new(Mutex::new(false)),
            }),
            trimmer: Arc::new(FakeTrimmer),
            cue_player: Arc::new(FakeCuePlayer),
            stt_client: Arc::new(FakeSttClient),
            text_enhancer: Arc::new(NoopTextEnhancer),
            clipboard: Arc::new(FakeClipboard),
            paste: Arc::new(FakePaste),
            tray: Arc::new(crate::ports::NullTrayPort),
            overlay: Arc::new(crate::ports::NullOverlayPort),
            temp_store: Arc::new(FakeTempStore),
        };
        let (_actor, handle) = AppActor::new(AppConfig::default(), deps);
        assert_eq!(handle.status().state, AppStateKind::Idle);
    }

    #[tokio::test]
    async fn actor_should_start_recording_before_playing_start_cue() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let deps = AppDependencies {
            audio_capture: Arc::new(OrderedAudioCapture {
                events: Arc::clone(&events),
            }),
            trimmer: Arc::new(FakeTrimmer),
            cue_player: Arc::new(OrderedCuePlayer {
                events: Arc::clone(&events),
            }),
            stt_client: Arc::new(FakeSttClient),
            text_enhancer: Arc::new(NoopTextEnhancer),
            clipboard: Arc::new(FakeClipboard),
            paste: Arc::new(FakePaste),
            tray: Arc::new(crate::ports::NullTrayPort),
            overlay: Arc::new(crate::ports::NullOverlayPort),
            temp_store: Arc::new(FakeTempStore),
        };
        let (mut actor, _handle) = AppActor::new(AppConfig::default(), deps);

        let exit = actor
            .handle_command(AppCommand::ToggleRecording {
                origin: CommandOrigin::CliControl,
            })
            .await
            .expect("command should succeed");

        assert_eq!(exit, None);
        assert_eq!(
            *events.lock().expect("mutex should not be poisoned"),
            vec!["start-recording", "play-start-cue"]
        );
    }

    #[tokio::test]
    async fn cancel_should_abort_recording_force_purge_audio_and_return_to_idle() {
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let cue_events = Arc::new(Mutex::new(Vec::new()));
        let temp_store = Arc::new(TrackingTempStore::default());
        let tray = Arc::new(TrackingTray::default());
        let stt_client = Arc::new(TrackingSttClient::default());
        let deps = AppDependencies {
            audio_capture: Arc::new(TrackingAudioCapture {
                lifecycle: Arc::clone(&lifecycle),
            }),
            trimmer: Arc::new(FakeTrimmer),
            cue_player: Arc::new(TrackingCuePlayer {
                events: Arc::clone(&cue_events),
            }),
            stt_client: stt_client.clone(),
            text_enhancer: Arc::new(NoopTextEnhancer),
            clipboard: Arc::new(FakeClipboard),
            paste: Arc::new(FakePaste),
            tray: tray.clone(),
            overlay: Arc::new(crate::ports::NullOverlayPort),
            temp_store: temp_store.clone(),
        };
        let (mut actor, handle) = AppActor::new(AppConfig::default(), deps);

        actor
            .handle_command(AppCommand::ToggleRecording {
                origin: CommandOrigin::CliControl,
            })
            .await
            .expect("start should succeed");

        let session_id = match actor.state {
            AppState::Recording(glossa_core::RecordingState { session_id }) => session_id,
            ref state => panic!("expected recording state, got {state:?}"),
        };

        let exit = actor
            .handle_command(AppCommand::CancelRecording {
                origin: CommandOrigin::EscapeKey,
            })
            .await
            .expect("cancel should succeed");

        assert_eq!(exit, None);
        assert_eq!(handle.status().state, AppStateKind::Idle);
        assert_eq!(
            *lifecycle
                .stop_calls
                .lock()
                .expect("mutex should not be poisoned"),
            Vec::<SessionId>::new()
        );
        assert_eq!(
            *lifecycle
                .abort_calls
                .lock()
                .expect("mutex should not be poisoned"),
            vec![session_id]
        );
        assert_eq!(
            *stt_client
                .calls
                .lock()
                .expect("mutex should not be poisoned"),
            0
        );
        assert_eq!(
            *temp_store
                .cleaned_sessions
                .lock()
                .expect("mutex should not be poisoned"),
            Vec::<SessionId>::new()
        );
        assert_eq!(
            *temp_store
                .purged_sessions
                .lock()
                .expect("mutex should not be poisoned"),
            vec![session_id]
        );
        assert_eq!(
            *cue_events.lock().expect("mutex should not be poisoned"),
            vec!["play-start-cue", "play-stop-cue"]
        );
        assert_eq!(
            *tray.states.lock().expect("mutex should not be poisoned"),
            vec![TrayState::Recording, TrayState::Idle]
        );
        assert!(
            tray.errors
                .lock()
                .expect("mutex should not be poisoned")
                .is_empty(),
            "cancel should not show tray errors"
        );
    }

    #[tokio::test]
    async fn max_duration_reached_should_stop_active_recording() {
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let cue_events = Arc::new(Mutex::new(Vec::new()));
        let deps = AppDependencies {
            audio_capture: Arc::new(TrackingAudioCapture {
                lifecycle: Arc::clone(&lifecycle),
            }),
            trimmer: Arc::new(FakeTrimmer),
            cue_player: Arc::new(TrackingCuePlayer {
                events: Arc::clone(&cue_events),
            }),
            stt_client: Arc::new(FakeSttClient),
            text_enhancer: Arc::new(NoopTextEnhancer),
            clipboard: Arc::new(FakeClipboard),
            paste: Arc::new(FakePaste),
            tray: Arc::new(crate::ports::NullTrayPort),
            overlay: Arc::new(crate::ports::NullOverlayPort),
            temp_store: Arc::new(FakeTempStore),
        };
        let (mut actor, handle) = AppActor::new(AppConfig::default(), deps);

        actor
            .handle_command(AppCommand::ToggleRecording {
                origin: CommandOrigin::CliControl,
            })
            .await
            .expect("start should succeed");

        let session_id = match actor.state {
            AppState::Recording(glossa_core::RecordingState { session_id }) => session_id,
            ref state => panic!("expected recording state, got {state:?}"),
        };

        actor
            .handle_internal_event(InternalEvent::MaxDurationReached { session_id })
            .await
            .expect("max duration handling should succeed");

        assert_eq!(handle.status().state, AppStateKind::Processing);
        assert_eq!(
            *lifecycle
                .stop_calls
                .lock()
                .expect("mutex should not be poisoned"),
            vec![session_id]
        );
        assert!(
            lifecycle
                .abort_calls
                .lock()
                .expect("mutex should not be poisoned")
                .is_empty(),
            "max duration should stop, not abort"
        );
        assert_eq!(
            *cue_events.lock().expect("mutex should not be poisoned"),
            vec!["play-start-cue", "play-stop-cue"]
        );
        assert!(
            actor.max_duration_abort.is_none(),
            "auto-stop timer should be cleared after stop"
        );
    }

    #[tokio::test]
    async fn stale_max_duration_event_should_be_ignored() {
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let deps = AppDependencies {
            audio_capture: Arc::new(TrackingAudioCapture {
                lifecycle: Arc::clone(&lifecycle),
            }),
            trimmer: Arc::new(FakeTrimmer),
            cue_player: Arc::new(FakeCuePlayer),
            stt_client: Arc::new(FakeSttClient),
            text_enhancer: Arc::new(NoopTextEnhancer),
            clipboard: Arc::new(FakeClipboard),
            paste: Arc::new(FakePaste),
            tray: Arc::new(crate::ports::NullTrayPort),
            overlay: Arc::new(crate::ports::NullOverlayPort),
            temp_store: Arc::new(FakeTempStore),
        };
        let (mut actor, handle) = AppActor::new(AppConfig::default(), deps);

        actor
            .handle_command(AppCommand::ToggleRecording {
                origin: CommandOrigin::CliControl,
            })
            .await
            .expect("start should succeed");

        let stale_session = SessionId::new();
        actor
            .handle_internal_event(InternalEvent::MaxDurationReached {
                session_id: stale_session,
            })
            .await
            .expect("stale max duration should be ignored");

        assert_eq!(handle.status().state, AppStateKind::Recording);
        assert!(
            lifecycle
                .stop_calls
                .lock()
                .expect("mutex should not be poisoned")
                .is_empty(),
            "stale max duration must not stop the current recording"
        );
    }

    #[tokio::test]
    async fn manual_stop_should_clear_max_duration_timer() {
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let deps = AppDependencies {
            audio_capture: Arc::new(TrackingAudioCapture {
                lifecycle: Arc::clone(&lifecycle),
            }),
            trimmer: Arc::new(FakeTrimmer),
            cue_player: Arc::new(FakeCuePlayer),
            stt_client: Arc::new(FakeSttClient),
            text_enhancer: Arc::new(NoopTextEnhancer),
            clipboard: Arc::new(FakeClipboard),
            paste: Arc::new(FakePaste),
            tray: Arc::new(crate::ports::NullTrayPort),
            overlay: Arc::new(crate::ports::NullOverlayPort),
            temp_store: Arc::new(FakeTempStore),
        };
        let (mut actor, _handle) = AppActor::new(AppConfig::default(), deps);

        actor
            .handle_command(AppCommand::ToggleRecording {
                origin: CommandOrigin::CliControl,
            })
            .await
            .expect("start should succeed");
        assert!(
            actor.max_duration_abort.is_some(),
            "timer should be armed while recording"
        );

        actor
            .handle_command(AppCommand::ToggleRecording {
                origin: CommandOrigin::CliControl,
            })
            .await
            .expect("stop should succeed");
        assert!(
            actor.max_duration_abort.is_none(),
            "timer should be cleared after manual stop"
        );
        assert_eq!(
            lifecycle
                .stop_calls
                .lock()
                .expect("mutex should not be poisoned")
                .len(),
            1
        );
    }

    #[derive(Debug, Default)]
    struct RecordingOverlay {
        events: Mutex<Vec<&'static str>>,
    }

    #[async_trait]
    impl crate::ports::OverlayPort for RecordingOverlay {
        async fn show_recording(&self) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("mutex should not be poisoned")
                .push("show");
            Ok(())
        }

        async fn show_processing(&self) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("mutex should not be poisoned")
                .push("processing");
            Ok(())
        }

        async fn hide(&self) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("mutex should not be poisoned")
                .push("hide");
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FailingAudioCapture;

    #[async_trait]
    impl AudioCapture for FailingAudioCapture {
        async fn start(
            &self,
            _session_id: SessionId,
            _spec: RecordSpec,
            _path: &Utf8Path,
        ) -> Result<Box<dyn ActiveRecording>, AppError> {
            Err(AppError::message("capture is unavailable"))
        }
    }

    #[tokio::test]
    async fn overlay_should_follow_the_recording_cycle() {
        let overlay = Arc::new(RecordingOverlay::default());
        let deps = AppDependencies {
            audio_capture: Arc::new(TrackingAudioCapture {
                lifecycle: Arc::new(RecordingLifecycle::default()),
            }),
            trimmer: Arc::new(FakeTrimmer),
            cue_player: Arc::new(FakeCuePlayer),
            stt_client: Arc::new(FakeSttClient),
            text_enhancer: Arc::new(NoopTextEnhancer),
            clipboard: Arc::new(FakeClipboard),
            paste: Arc::new(FakePaste),
            tray: Arc::new(crate::ports::NullTrayPort),
            overlay: overlay.clone(),
            temp_store: Arc::new(FakeTempStore),
        };
        let (mut actor, _handle) = AppActor::new(AppConfig::default(), deps);

        actor
            .handle_command(AppCommand::ToggleRecording {
                origin: CommandOrigin::CliControl,
            })
            .await
            .expect("start should succeed");
        actor
            .handle_command(AppCommand::ToggleRecording {
                origin: CommandOrigin::CliControl,
            })
            .await
            .expect("stop should succeed");

        assert_eq!(
            *overlay.events.lock().expect("mutex should not be poisoned"),
            vec!["show", "processing"]
        );

        let event = actor.internal_rx.recv().await.expect("processing event");
        assert!(matches!(event, InternalEvent::ProcessingReady { .. }));
        actor
            .handle_internal_event(event)
            .await
            .expect("processing should finish");
        assert!(matches!(actor.state, AppState::Pasting(_)));
        assert_eq!(
            *overlay.events.lock().expect("mutex should not be poisoned"),
            vec!["show", "processing", "hide"]
        );
    }

    #[tokio::test]
    async fn processing_overlay_should_hide_on_failure_or_empty_result() {
        for outcome in [
            CycleOutcome::Failed("server request failed".into()),
            CycleOutcome::Completed,
        ] {
            let overlay = Arc::new(RecordingOverlay::default());
            let deps = AppDependencies {
                audio_capture: Arc::new(FakeAudioCapture {
                    started: Arc::new(Mutex::new(false)),
                }),
                trimmer: Arc::new(FakeTrimmer),
                cue_player: Arc::new(FakeCuePlayer),
                stt_client: Arc::new(FakeSttClient),
                text_enhancer: Arc::new(NoopTextEnhancer),
                clipboard: Arc::new(FakeClipboard),
                paste: Arc::new(FakePaste),
                tray: Arc::new(crate::ports::NullTrayPort),
                overlay: overlay.clone(),
                temp_store: Arc::new(FakeTempStore),
            };
            let (mut actor, _handle) = AppActor::new(AppConfig::default(), deps);
            let session_id = SessionId::new();
            actor.state = AppState::Processing(glossa_core::ProcessingState { session_id });
            actor.show_processing_overlay().await;
            actor
                .handle_internal_event(InternalEvent::ProcessingFinished {
                    session_id,
                    outcome,
                })
                .await
                .expect("finish processing");
            assert_eq!(actor.state, AppState::Idle);
            assert_eq!(
                *overlay.events.lock().expect("overlay events"),
                vec!["processing", "hide"]
            );
        }
    }

    #[tokio::test]
    async fn overlay_should_hide_on_capture_failure_cancel_restart_and_shutdown() {
        for (fail_start, command) in [
            (
                true,
                AppCommand::Shutdown {
                    origin: CommandOrigin::CliControl,
                },
            ),
            (
                false,
                AppCommand::CancelRecording {
                    origin: CommandOrigin::HotkeyShortcut,
                },
            ),
            (
                false,
                AppCommand::Restart {
                    origin: CommandOrigin::TrayMenu,
                },
            ),
            (
                false,
                AppCommand::Shutdown {
                    origin: CommandOrigin::CliControl,
                },
            ),
        ] {
            let overlay = Arc::new(RecordingOverlay::default());
            let deps = AppDependencies {
                audio_capture: if fail_start {
                    Arc::new(FailingAudioCapture)
                } else {
                    Arc::new(TrackingAudioCapture {
                        lifecycle: Arc::new(RecordingLifecycle::default()),
                    })
                },
                trimmer: Arc::new(FakeTrimmer),
                cue_player: Arc::new(FakeCuePlayer),
                stt_client: Arc::new(FakeSttClient),
                text_enhancer: Arc::new(NoopTextEnhancer),
                clipboard: Arc::new(FakeClipboard),
                paste: Arc::new(FakePaste),
                tray: Arc::new(crate::ports::NullTrayPort),
                overlay: overlay.clone(),
                temp_store: Arc::new(FakeTempStore),
            };
            let (actor, handle) = AppActor::new(AppConfig::default(), deps);
            let origin = match &command {
                AppCommand::Shutdown { origin } | AppCommand::CancelRecording { origin } => *origin,
                AppCommand::Restart { origin } => *origin,
                _ => unreachable!("test commands carry an origin"),
            };
            handle
                .send(AppCommand::ToggleRecording { origin })
                .expect("start");
            let hides_before_shutdown =
                fail_start || matches!(command, AppCommand::CancelRecording { .. });
            handle.send(command).expect("end recording");
            handle
                .send(AppCommand::Shutdown {
                    origin: CommandOrigin::CliControl,
                })
                .expect("shutdown");
            actor.run().await.expect("actor exit");
            let events = overlay.events.lock().expect("overlay events");
            let expected = if hides_before_shutdown {
                vec!["show", "hide", "hide"]
            } else {
                vec!["show", "hide"]
            };
            assert_eq!(*events, expected);
        }
    }
}
