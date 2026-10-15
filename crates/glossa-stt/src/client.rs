use std::sync::Arc;

use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::StatusCode;
use tokio::fs;
use tracing::debug;

use glossa_app::{ports::SttClient, AppError};
use glossa_core::{AudioFormat, CapturedAudio, ProviderConfig, ProviderKind};

use crate::dto::TranscriptionResponse;

/// Shared OpenAI-style multipart transcription client.
#[derive(Debug, Clone)]
pub struct HttpSttClient {
    provider_name: &'static str,
    endpoint: String,
    model: String,
    api_key: String,
    client: reqwest::Client,
}

impl HttpSttClient {
    #[must_use]
    pub fn new(
        provider_name: &'static str,
        endpoint: String,
        model: String,
        api_key: String,
    ) -> Self {
        Self {
            provider_name,
            endpoint,
            model,
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl SttClient for HttpSttClient {
    fn provider_name(&self) -> &'static str {
        self.provider_name
    }

    async fn transcribe(&self, audio: &CapturedAudio) -> Result<String, AppError> {
        debug!(
            provider = self.provider_name,
            path = %audio.path,
            duration_ms = audio.duration_ms,
            "uploading captured audio for transcription"
        );
        let bytes = fs::read(audio.path.as_std_path())
            .await
            .map_err(|error| AppError::io("failed to read captured audio", error))?;
        let filename = audio.path.file_name().unwrap_or("capture.wav").to_string();
        let mime = audio
            .path
            .extension()
            .and_then(|extension| AudioFormat::from_extension(&extension.to_ascii_lowercase()))
            .map_or("application/octet-stream", AudioFormat::mime_type);
        let file_part = Part::bytes(bytes)
            .file_name(filename)
            .mime_str(mime)
            .map_err(|error| {
                AppError::message(format!("failed to encode upload mime type: {error}"))
            })?;
        let form = Form::new()
            .text("model", self.model.clone())
            .part("file", file_part);

        let mut request = self.client.post(&self.endpoint);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }

        let response =
            request.multipart(form).send().await.map_err(|error| {
                AppError::message(format!("transcription request failed: {error}"))
            })?;

        let status = response.status();
        if status != StatusCode::OK {
            return Err(AppError::message(http_status_error_message(status)));
        }

        let parsed: TranscriptionResponse = response.json().await.map_err(|error| {
            AppError::message(format!("failed to decode transcription response: {error}"))
        })?;
        Ok(parsed.text)
    }
}

fn http_status_error_message(status: StatusCode) -> String {
    let code = status.as_u16();
    let (label, description) = match code {
        206 => Some((
            "Partial Content",
            "the provider returned only a partial response for the transcription request",
        )),
        400 => Some((
            "Bad Request",
            "the request was invalid or malformed",
        )),
        401 => Some((
            "Unauthorized",
            "authentication failed or the API key is invalid",
        )),
        403 => Some((
            "Forbidden",
            "the request is not allowed, possibly due to permissions or region restrictions",
        )),
        404 => Some((
            "Not Found",
            "the requested resource, model, or endpoint could not be found",
        )),
        409 => Some((
            "Conflict",
            "the request conflicted with the provider's current state; retry the request",
        )),
        413 => Some((
            "Request Entity Too Large",
            "the audio payload is too large for the provider to accept",
        )),
        422 => Some((
            "Unprocessable Entity",
            "the request was well-formed but could not be processed because the input was semantically invalid",
        )),
        424 => Some((
            "Failed Dependency",
            "an upstream or dependent service failed while processing the request",
        )),
        429 => Some((
            "Too Many Requests",
            "rate limit or quota exceeded",
        )),
        498 => Some((
            "Flex Tier Capacity Exceeded",
            "the provider does not currently have enough capacity to process the request",
        )),
        499 => Some((
            "Request Cancelled",
            "the request was cancelled before the provider could finish processing it",
        )),
        500 => Some((
            "Internal Server Error",
            "the provider encountered an internal error",
        )),
        502 => Some((
            "Bad Gateway",
            "the provider received an invalid upstream response",
        )),
        503 => Some((
            "Service Unavailable",
            "the provider is temporarily unavailable or overloaded",
        )),
        504 => Some((
            "Gateway Timeout",
            "the provider timed out while waiting for an upstream response",
        )),
        _ => None,
    }
    .unwrap_or_else(|| (status.canonical_reason().unwrap_or("Unknown Status"), ""));

    if description.is_empty() {
        format!("transcription request failed with status {code} {label}")
    } else {
        format!("transcription request failed with status {code} {label}: {description}")
    }
}

#[must_use]
pub fn build_http_client(
    config: &ProviderConfig,
    api_key: String,
    provider_name: &'static str,
) -> Arc<dyn SttClient> {
    let base_url = config
        .base_url
        .clone()
        .unwrap_or_else(|| match config.kind {
            ProviderKind::Groq => "https://api.groq.com/openai/v1".into(),
            ProviderKind::OpenAi => "https://api.openai.com/v1".into(),
            ProviderKind::OpenAiCompatible => unreachable!("validated elsewhere"),
        });
    let endpoint = format!("{}/audio/transcriptions", base_url.trim_end_matches('/'));
    Arc::new(HttpSttClient::new(
        provider_name,
        endpoint,
        config.model.clone(),
        api_key,
    ))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use glossa_core::{
        AudioFormat, CapturedAudio, ProviderConfig, ProviderKind, SecretSource, SessionId,
    };

    use reqwest::StatusCode;

    use super::http_status_error_message;

    #[test]
    fn known_status_should_render_code_and_description() {
        assert_eq!(
            http_status_error_message(StatusCode::TOO_MANY_REQUESTS),
            "transcription request failed with status 429 Too Many Requests: rate limit or quota exceeded"
        );
    }

    #[test]
    fn custom_status_should_render_code_and_description() {
        let status = StatusCode::from_u16(498).expect("status should be valid");
        assert_eq!(
            http_status_error_message(status),
            "transcription request failed with status 498 Flex Tier Capacity Exceeded: the provider does not currently have enough capacity to process the request"
        );
    }

    #[test]
    fn documented_statuses_should_use_expected_endpoint_agnostic_messages() {
        let cases = [
            (
                206,
                "transcription request failed with status 206 Partial Content: the provider returned only a partial response for the transcription request",
            ),
            (
                400,
                "transcription request failed with status 400 Bad Request: the request was invalid or malformed",
            ),
            (
                401,
                "transcription request failed with status 401 Unauthorized: authentication failed or the API key is invalid",
            ),
            (
                403,
                "transcription request failed with status 403 Forbidden: the request is not allowed, possibly due to permissions or region restrictions",
            ),
            (
                404,
                "transcription request failed with status 404 Not Found: the requested resource, model, or endpoint could not be found",
            ),
            (
                409,
                "transcription request failed with status 409 Conflict: the request conflicted with the provider's current state; retry the request",
            ),
            (
                413,
                "transcription request failed with status 413 Request Entity Too Large: the audio payload is too large for the provider to accept",
            ),
            (
                422,
                "transcription request failed with status 422 Unprocessable Entity: the request was well-formed but could not be processed because the input was semantically invalid",
            ),
            (
                424,
                "transcription request failed with status 424 Failed Dependency: an upstream or dependent service failed while processing the request",
            ),
            (
                429,
                "transcription request failed with status 429 Too Many Requests: rate limit or quota exceeded",
            ),
            (
                499,
                "transcription request failed with status 499 Request Cancelled: the request was cancelled before the provider could finish processing it",
            ),
            (
                500,
                "transcription request failed with status 500 Internal Server Error: the provider encountered an internal error",
            ),
            (
                502,
                "transcription request failed with status 502 Bad Gateway: the provider received an invalid upstream response",
            ),
            (
                503,
                "transcription request failed with status 503 Service Unavailable: the provider is temporarily unavailable or overloaded",
            ),
            (
                504,
                "transcription request failed with status 504 Gateway Timeout: the provider timed out while waiting for an upstream response",
            ),
        ];

        for (code, expected) in cases {
            let status = StatusCode::from_u16(code).expect("status should be valid");
            assert_eq!(http_status_error_message(status), expected);
        }
    }

    #[test]
    fn unknown_status_should_fall_back_to_reason_phrase() {
        assert_eq!(
            http_status_error_message(StatusCode::IM_A_TEAPOT),
            "transcription request failed with status 418 I'm a teapot"
        );
    }

    #[tokio::test]
    async fn wav_should_upload_with_wav_mime() {
        check_upload(AudioFormat::Wav, "audio/wav").await;
    }

    #[tokio::test]
    async fn compressed_formats_should_upload_with_correct_filename_and_mime() {
        for (format, mime) in [
            (AudioFormat::Mp3, "audio/mpeg"),
            (AudioFormat::M4a, "audio/mp4"),
            (AudioFormat::Flac, "audio/flac"),
            (AudioFormat::Ogg, "audio/ogg"),
            (AudioFormat::Aac, "audio/aac"),
        ] {
            check_upload(format, mime).await;
        }
    }

    #[tokio::test]
    async fn uppercase_file_extension_should_keep_audio_mime() {
        check_upload_with_extension("MP3", "audio/mpeg").await;
    }

    async fn check_upload(format: AudioFormat, mime: &str) {
        check_upload_with_extension(format.extension(), mime).await;
    }

    async fn check_upload_with_extension(extension: &str, mime: &str) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("local address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).await.expect("read request");
                assert!(count > 0, "request ended before its body arrived");
                request.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&request);
                if let Some(header_end) = text.find("\r\n\r\n") {
                    let length = text[..header_end]
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("content length"))
                        })
                        .expect("content-length header");
                    if request.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
            let body = r#"{"text":"hello"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("respond");
            String::from_utf8(request).expect("request is utf8")
        });

        let session_id = SessionId::new();
        let path = std::env::temp_dir().join(format!("glossa-stt-{session_id}.{extension}"));
        tokio::fs::write(&path, b"RIFF-test-audio")
            .await
            .expect("write audio");
        let audio = CapturedAudio {
            session_id,
            path: path.to_str().expect("utf8 path").into(),
            duration_ms: 1000,
            sample_rate_hz: 16000,
            channels: 1,
        };
        let config = ProviderConfig {
            kind: ProviderKind::Groq,
            base_url: Some(format!("http://{address}/api/v1/")),
            model: "whisper-large-v3".into(),
            api_key: SecretSource::Empty,
        };
        let client = super::build_http_client(&config, "test-key".into(), "groq");
        assert_eq!(client.provider_name(), "groq");
        let result = client.transcribe(&audio).await;
        tokio::fs::remove_file(path).await.expect("remove audio");
        assert_eq!(result.expect("transcribe"), "hello");

        let request = server.await.expect("server task");
        assert!(request.starts_with("POST /api/v1/audio/transcriptions HTTP/1.1\r\n"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-key\r\n"));
        assert!(request.contains("multipart/form-data; boundary="));
        assert!(request.contains("name=\"model\"\r\n\r\nwhisper-large-v3"));
        assert!(request.contains("name=\"file\"; filename=\"glossa-stt-"));
        assert!(request.contains(&format!(
            "filename=\"glossa-stt-{session_id}.{extension}\""
        )));
        assert!(request.contains(&format!("Content-Type: {mime}")));
        assert!(request.contains("RIFF-test-audio"));
    }
}
