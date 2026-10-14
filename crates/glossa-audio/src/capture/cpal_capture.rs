use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use camino::Utf8Path;
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat, SampleRate, StreamConfig,
};

use glossa_app::{ports::AudioCapture, AppError};
use glossa_core::{RecordSpec, SessionId};

use super::{active_recording::CpalActiveRecording, sample_convert};

type WriterHandle = thread::JoinHandle<Result<(), String>>;

const FIRST_INPUT_BUFFER_TIMEOUT: Duration = Duration::from_secs(2);
const LOW_LATENCY_BUFFER_FRAMES: [u32; 2] = [512, 1024];

/// Audio capture backed by the system default input device through CPAL.
#[derive(Clone, Default)]
pub struct CpalAudioCapture;

impl CpalAudioCapture {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl fmt::Debug for CpalAudioCapture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("CpalAudioCapture").finish()
    }
}

#[async_trait]
impl AudioCapture for CpalAudioCapture {
    async fn start(
        &self,
        session_id: SessionId,
        spec: RecordSpec,
        path: &Utf8Path,
    ) -> Result<Box<dyn glossa_app::ports::ActiveRecording>, AppError> {
        let startup_started_at = Instant::now();
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| AppError::message("no default input device is available"))?;
        let default_config = device
            .default_input_config()
            .map_err(|error| {
                tracing::error!(error = %error, "failed to read input config");
                AppError::message(
                    "Couldn't access your microphone. Please check its connection and your Windows microphone settings.",
                )
            })?;
        let fallback_config = default_config.config();
        let requested_config = StreamConfig {
            channels: spec.channels,
            sample_rate: SampleRate(spec.sample_rate_hz),
            buffer_size: cpal::BufferSize::Default,
        };

        let device_name = device
            .name()
            .unwrap_or_else(|_| "unknown input device".to_owned());
        // Build the stream before opening the WAV writer so the on-disk header matches
        // the format CPAL actually delivers (common on Windows: 44100 Hz stereo fallback
        // when 16000 Hz mono is unsupported). Samples only flow after `stream.play()`.
        let (writer_tx, writer_rx) = mpsc::sync_channel::<Vec<i16>>(8);
        let (first_input_tx, first_input_rx) = mpsc::sync_channel(1);
        let first_input_notifier = FirstInputBufferNotifier::new(first_input_tx);
        let stream_build_started_at = Instant::now();
        let ((stream, stream_config), used_fallback_config) = match build_preferred_stream(
            &device,
            default_config.sample_format(),
            &requested_config,
            writer_tx.clone(),
            first_input_notifier.clone(),
        ) {
            Ok(stream) => (stream, false),
            Err(error) => {
                tracing::info!(
                    %session_id,
                    device = %device_name,
                    requested_sample_rate_hz = requested_config.sample_rate.0,
                    requested_channels = requested_config.channels,
                    fallback_sample_rate_hz = fallback_config.sample_rate.0,
                    fallback_channels = fallback_config.channels,
                    error = %error,
                    "requested input configs failed, retrying with default input config"
                );
                (
                    build_preferred_stream(
                        &device,
                        default_config.sample_format(),
                        &fallback_config,
                        writer_tx.clone(),
                        first_input_notifier,
                    )
                    .map_err(AppError::message)?,
                    true,
                )
            }
        };
        let stream_build_ms = stream_build_started_at.elapsed().as_millis();

        let writer_started_at = Instant::now();
        let (writer_handle, writer_config) = start_writer(path, &stream_config, writer_rx);
        let writer_setup_ms = writer_started_at.elapsed().as_millis();

        let play_started_at = Instant::now();
        stream
            .play()
            .map_err(|error| AppError::message(format!("failed to start audio stream: {error}")))?;
        let stream_play_ms = play_started_at.elapsed().as_millis();
        // `cpal::Stream` is not Send, so the readiness wait cannot cross an await.
        let first_input_buffer_ms = match wait_for_first_input_buffer(first_input_rx) {
            Ok(ms) => ms,
            Err(error) => {
                let _ = stream.pause();
                drop(stream);
                drop(writer_tx);
                match writer_handle.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(writer_error)) => {
                        tracing::warn!(
                            error = %writer_error,
                            "audio writer failed while cleaning up capture startup"
                        );
                    }
                    Err(_) => {
                        tracing::warn!(
                            "audio writer thread panicked while cleaning up capture startup"
                        );
                    }
                }
                return Err(error);
            }
        };
        tracing::info!(
            %session_id,
            device = %device_name,
            requested_sample_rate_hz = spec.sample_rate_hz,
            requested_channels = spec.channels,
            writer_sample_rate_hz = writer_config.sample_rate.0,
            writer_channels = writer_config.channels,
            input_sample_rate_hz = stream_config.sample_rate.0,
            input_channels = stream_config.channels,
            input_buffer_size = %buffer_size_label(&stream_config.buffer_size),
            fallback_sample_rate_hz = fallback_config.sample_rate.0,
            fallback_channels = fallback_config.channels,
            used_fallback_config,
            writer_setup_ms,
            stream_build_ms,
            stream_play_ms,
            first_input_buffer_ms,
            total_startup_ms = startup_started_at.elapsed().as_millis(),
            "audio capture initialized"
        );

        Ok(Box::new(CpalActiveRecording {
            session_id,
            path: path.to_owned(),
            sample_rate_hz: writer_config.sample_rate.0,
            channels: writer_config.channels,
            started_at: Instant::now(),
            stream,
            tx: Some(writer_tx),
            writer_handle: Some(writer_handle),
        }))
    }
}

fn start_writer(
    path: &Utf8Path,
    config: &StreamConfig,
    rx: mpsc::Receiver<Vec<i16>>,
) -> (WriterHandle, StreamConfig) {
    let wav_path = path.to_path_buf().into_std_path_buf();
    let wav_spec = hound::WavSpec {
        channels: config.channels,
        sample_rate: config.sample_rate.0,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let handle = thread::spawn(move || -> Result<(), String> {
        let mut writer = hound::WavWriter::create(wav_path, wav_spec)
            .map_err(|error| format!("failed to create wav writer: {error}"))?;
        while let Ok(chunk) = rx.recv() {
            for sample in chunk {
                writer
                    .write_sample(sample)
                    .map_err(|error| format!("failed to write wav sample: {error}"))?;
            }
        }
        writer
            .finalize()
            .map_err(|error| format!("failed to finalize wav writer: {error}"))?;
        Ok(())
    });
    (handle, config.clone())
}

fn build_preferred_stream(
    device: &cpal::Device,
    sample_format: SampleFormat,
    base_config: &StreamConfig,
    tx: SyncSender<Vec<i16>>,
    first_input_notifier: FirstInputBufferNotifier,
) -> Result<(cpal::Stream, StreamConfig), String> {
    let mut errors = Vec::new();

    for config in input_config_candidates(base_config) {
        match build_stream(
            device,
            sample_format,
            &config,
            tx.clone(),
            first_input_notifier.clone(),
        ) {
            Ok(stream) => return Ok((stream, config)),
            Err(error) => {
                tracing::debug!(
                    sample_rate_hz = config.sample_rate.0,
                    channels = config.channels,
                    buffer_size = %buffer_size_label(&config.buffer_size),
                    error = %error,
                    "input stream config candidate failed"
                );
                errors.push(format!(
                    "{} Hz/{} ch/{} buffer: {}",
                    config.sample_rate.0,
                    config.channels,
                    buffer_size_label(&config.buffer_size),
                    error
                ));
            }
        }
    }

    Err(format!(
        "failed to build input stream with preferred configs: {}",
        errors.join("; ")
    ))
}

fn build_stream(
    device: &cpal::Device,
    sample_format: SampleFormat,
    config: &StreamConfig,
    tx: SyncSender<Vec<i16>>,
    first_input_notifier: FirstInputBufferNotifier,
) -> Result<cpal::Stream, String> {
    let error_callback = |error| tracing::error!(error = %error, "audio stream error");

    match sample_format {
        SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| {
                    send_converted(
                        data.iter().copied().map(sample_convert::f32_to_i16),
                        &tx,
                        &first_input_notifier,
                    )
                },
                error_callback,
                None,
            )
            .map_err(|error| format!("failed to build f32 input stream: {error}")),
        SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    send_converted(data.iter().copied(), &tx, &first_input_notifier)
                },
                error_callback,
                None,
            )
            .map_err(|error| format!("failed to build i16 input stream: {error}")),
        SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| {
                    send_converted(
                        data.iter().copied().map(sample_convert::u16_to_i16),
                        &tx,
                        &first_input_notifier,
                    )
                },
                error_callback,
                None,
            )
            .map_err(|error| format!("failed to build u16 input stream: {error}")),
        _ => Err("unsupported input sample format".into()),
    }
}

fn input_config_candidates(base_config: &StreamConfig) -> Vec<StreamConfig> {
    let mut candidates = Vec::with_capacity(LOW_LATENCY_BUFFER_FRAMES.len() + 1);

    for buffer_frames in LOW_LATENCY_BUFFER_FRAMES {
        let mut config = base_config.clone();
        config.buffer_size = cpal::BufferSize::Fixed(buffer_frames);
        candidates.push(config);
    }

    let mut default_config = base_config.clone();
    default_config.buffer_size = cpal::BufferSize::Default;
    candidates.push(default_config);

    candidates
}

fn buffer_size_label(buffer_size: &cpal::BufferSize) -> String {
    match buffer_size {
        cpal::BufferSize::Default => "default".into(),
        cpal::BufferSize::Fixed(size) => size.to_string(),
    }
}

#[derive(Debug, Clone)]
struct FirstInputBufferNotifier {
    tx: SyncSender<()>,
    notified: Arc<AtomicBool>,
}

impl FirstInputBufferNotifier {
    fn new(tx: SyncSender<()>) -> Self {
        Self {
            tx,
            notified: Arc::new(AtomicBool::new(false)),
        }
    }

    fn notify(&self) {
        if !self.notified.swap(true, Ordering::AcqRel) {
            let _ = self.tx.try_send(());
        }
    }
}

fn wait_for_first_input_buffer(rx: mpsc::Receiver<()>) -> Result<u128, AppError> {
    let wait_started_at = Instant::now();
    match rx.recv_timeout(FIRST_INPUT_BUFFER_TIMEOUT) {
        Ok(()) => Ok(wait_started_at.elapsed().as_millis()),
        Err(RecvTimeoutError::Timeout) => Err(AppError::message(format!(
            "audio input stream did not produce samples within {} ms",
            FIRST_INPUT_BUFFER_TIMEOUT.as_millis()
        ))),
        Err(RecvTimeoutError::Disconnected) => Err(AppError::message(
            "audio input stream stopped before producing samples",
        )),
    }
}

fn send_converted<I>(
    samples: I,
    tx: &SyncSender<Vec<i16>>,
    first_input_notifier: &FirstInputBufferNotifier,
) where
    I: Iterator<Item = i16>,
{
    let mut peak = 0i16;
    let chunk: Vec<i16> = samples
        .inspect(|sample| peak = peak.max(sample.abs()))
        .collect();
    let has_samples = !chunk.is_empty();
    if has_samples {
        first_input_notifier.notify();
    }
    glossa_app::level::record_peak(f32::from(peak) / f32::from(i16::MAX));
    let _ = tx.send(chunk);
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread, time::Duration};

    use super::*;

    #[test]
    fn send_converted_should_signal_first_input_buffer_once() {
        let (audio_tx, audio_rx) = mpsc::sync_channel(2);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let notifier = FirstInputBufferNotifier::new(ready_tx);

        send_converted([1_i16, 2_i16].into_iter(), &audio_tx, &notifier);

        assert_eq!(
            audio_rx
                .recv_timeout(Duration::from_millis(50))
                .expect("audio chunk should be sent"),
            vec![1_i16, 2_i16]
        );
        assert!(
            ready_rx.recv_timeout(Duration::from_millis(50)).is_ok(),
            "first input buffer should signal readiness"
        );

        send_converted([3_i16].into_iter(), &audio_tx, &notifier);

        assert!(
            ready_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "subsequent input buffers should not signal readiness again"
        );
    }

    #[test]
    fn send_converted_should_signal_first_input_before_blocked_writer_send_completes() {
        let (audio_tx, audio_rx) = mpsc::sync_channel(0);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let notifier = FirstInputBufferNotifier::new(ready_tx);

        let handle = thread::spawn(move || {
            send_converted([1_i16].into_iter(), &audio_tx, &notifier);
        });

        let ready_result = ready_rx.recv_timeout(Duration::from_millis(50));
        assert_eq!(
            audio_rx
                .recv_timeout(Duration::from_millis(50))
                .expect("audio chunk should unblock the writer send"),
            vec![1_i16]
        );
        handle.join().expect("send thread should finish");

        assert!(
            ready_result.is_ok(),
            "first input readiness should be signaled when the callback receives samples"
        );
    }

    #[test]
    fn input_config_candidates_should_prefer_fixed_low_latency_buffers() {
        let base_config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(16_000),
            buffer_size: cpal::BufferSize::Default,
        };

        let candidates = input_config_candidates(&base_config);

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].buffer_size, cpal::BufferSize::Fixed(512));
        assert_eq!(candidates[1].buffer_size, cpal::BufferSize::Fixed(1024));
        assert_eq!(candidates[2].buffer_size, cpal::BufferSize::Default);
    }

    #[test]
    fn writer_header_should_match_actual_stream_config_on_fallback_rates() {
        // Regression: when CPAL falls back to 44100 Hz stereo, the WAV must not be
        // labeled as the requested 16000 Hz mono (that made STT hear garbage).
        let path_buf = std::env::temp_dir().join(format!(
            "glossa-fallback-writer-{}.wav",
            std::process::id()
        ));
        let path = Utf8Path::from_path(&path_buf).expect("utf8 path");
        let stream_config = StreamConfig {
            channels: 2,
            sample_rate: SampleRate(44_100),
            buffer_size: cpal::BufferSize::Default,
        };
        let (tx, rx) = mpsc::sync_channel(2);
        let (handle, writer_config) = start_writer(path, &stream_config, rx);

        assert_eq!(writer_config.sample_rate.0, 44_100);
        assert_eq!(writer_config.channels, 2);

        // One stereo frame at half scale.
        tx.send(vec![16_000_i16, -16_000_i16]).expect("send");
        drop(tx);
        handle.join().expect("join").expect("writer ok");

        let reader = hound::WavReader::open(path.as_std_path()).expect("open wav");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 44_100);
        assert_eq!(spec.channels, 2);
        let samples: Vec<i16> = reader
            .into_samples::<i16>()
            .map(|s| s.expect("sample"))
            .collect();
        assert_eq!(samples, vec![16_000, -16_000]);
        let _ = std::fs::remove_file(&path_buf);
    }
}
