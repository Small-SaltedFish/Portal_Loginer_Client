mod host;
mod ipc;
mod logs;
mod network;
mod runtime;
mod security;
mod settings;
mod startup;
mod storage;

use std::io;

pub const SERVICE_NAME: &str = "PortalLoginer";
pub const PIPE_NAME: &str = r"\\.\pipe\PortalLoginer.v1";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Windows operation failed: {0}")]
    Os(#[from] io::Error),
    #[error("service operation failed")]
    Service(#[from] windows_service::Error),
    #[error("invalid settings or local request")]
    Invalid,
    #[error("credential protection failed")]
    Credentials,
    #[error("access denied")]
    Access,
    #[error("local request timed out")]
    Timeout,
}

type Result<T> = std::result::Result<T, Error>;

pub use ipc::watch_events;
pub use ipc::Request as ClientRequest;
pub use settings::Settings;
pub fn current_user_sid() -> Result<String> {
    security::current_sid()
}
pub fn start_application() -> Result<()> {
    host::start_application()
}
pub async fn exit_application() -> Result<()> {
    if host::service_stopped()? {
        return Ok(());
    }
    let result = request(&ClientRequest::Shutdown).await;
    if let Ok(value) = &result {
        if value["ok"] != true {
            return Err(Error::Invalid);
        }
    }
    // A closing pipe may race the SCM transition to Stopped. The system state
    // below is the completion condition, including when the reply was lost.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    while !host::service_stopped()? {
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Ok(())
}

/// Shared client for the upcoming tray/settings processes. Credentials are
/// serialized only into a temporary zeroing buffer and sent to the SCM PID.
pub async fn request(value: &ClientRequest) -> Result<serde_json::Value> {
    let bytes = zeroize::Zeroizing::new(serde_json::to_vec(value).map_err(|_| Error::Invalid)?);
    ipc::client(&bytes).await
}

pub fn cli() -> Result<()> {
    host::cli()
}

mod uninstall;

mod updates;
pub use updates::View as UpdateView;
