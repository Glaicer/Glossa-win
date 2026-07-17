use std::net::SocketAddr;

use camino::Utf8PathBuf;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};
use tracing::{error, info};

use glossa_app::{AppError, AppHandle};
use glossa_core::{AppCommand, CommandOrigin};

use super::{IpcRequest, IpcResponse};

/// Loopback TCP IPC server for daemon control and status inspection.
#[derive(Debug, Clone)]
pub struct IpcServer {
    endpoint: Utf8PathBuf,
    handle: AppHandle,
}

impl IpcServer {
    #[must_use]
    pub fn new(endpoint: Utf8PathBuf, handle: AppHandle) -> Self {
        Self { endpoint, handle }
    }

    pub async fn run(self) -> Result<(), AppError> {
        let addr = parse_endpoint(&self.endpoint)?;
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|error| AppError::io("failed to bind daemon IPC endpoint", error))?;
        info!(endpoint = %self.endpoint, "ipc server listening");

        loop {
            let (stream, _) = listener
                .accept()
                .await
                .map_err(|error| AppError::io("failed to accept ipc client", error))?;
            let handle = self.handle.clone();
            tokio::spawn(async move {
                if let Err(error) = handle_client(stream, handle).await {
                    error!(error = %error, "ipc client handling failed");
                }
            });
        }
    }
}

pub(crate) fn parse_endpoint(endpoint: &Utf8PathBuf) -> Result<SocketAddr, AppError> {
    endpoint
        .as_str()
        .parse::<SocketAddr>()
        .map_err(|error| AppError::message(format!("invalid IPC endpoint `{endpoint}`: {error}")))
}

async fn handle_client(stream: TcpStream, handle: AppHandle) -> Result<(), AppError> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|error| AppError::io("failed to read ipc request", error))?;

    let request: IpcRequest = serde_json::from_str(&line)
        .map_err(|error| AppError::message(format!("failed to decode ipc request: {error}")))?;
    let response = match request {
        IpcRequest::Toggle => {
            handle.send(AppCommand::ToggleRecording {
                origin: CommandOrigin::CliControl,
            })?;
            IpcResponse::Ok
        }
        IpcRequest::Status => IpcResponse::Status {
            status: handle.status(),
        },
        IpcRequest::Shutdown => {
            handle.send(AppCommand::Shutdown {
                origin: CommandOrigin::CliControl,
            })?;
            IpcResponse::Ok
        }
    };

    let payload = serde_json::to_vec(&response)
        .map_err(|error| AppError::message(format!("failed to encode ipc response: {error}")))?;
    let mut stream = reader.into_inner();
    stream
        .write_all(&payload)
        .await
        .map_err(|error| AppError::io("failed to write ipc response", error))?;
    stream
        .write_all(b"\n")
        .await
        .map_err(|error| AppError::io("failed to terminate ipc response", error))?;
    Ok(())
}
