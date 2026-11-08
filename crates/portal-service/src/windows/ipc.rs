use super::{
    logs,
    network::Snapshot,
    security,
    settings::{Settings, Stored},
    storage::Store,
    Error, Result, PIPE_NAME,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{os::windows::io::AsRawHandle, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions},
    sync::{mpsc, oneshot, Semaphore},
};
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, Zeroizing};

#[derive(Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status,
    Watch,
    GetSettings,
    Save {
        settings: Settings,
        password: String,
    },
    SelectInterface {
        index: Option<u32>,
    },
    Configure {
        settings: Settings,
        password: String,
    },
    CheckUpdates,
    InstallUpdate,
    UserActivity {
        busy: bool,
    },
    Check,
    Login,
    Relogin,
    Logout,
    Pause,
    Resume,
    Shutdown,
    Logs,
}
impl Drop for Request {
    fn drop(&mut self) {
        if let Self::Save { password, .. } | Self::Configure { password, .. } = self {
            password.zeroize();
        }
    }
}
impl Request {
    pub fn read_only(&self) -> bool {
        matches!(
            self,
            Self::Status | Self::Watch | Self::GetSettings | Self::Logs
        )
    }
}
pub struct Envelope {
    pub request: Request,
    pub reply: oneshot::Sender<Value>,
}

#[derive(Clone, Serialize)]
pub struct Status {
    pub instance_id: u64,
    pub version: &'static str,
    pub phase: String,
    pub failures: u32,
    pub authentication_id: u64,
    pub last_authentication_unix: Option<u64>,
    pub network: Option<Snapshot>,
    pub interfaces: Vec<Snapshot>,
    pub event_watch_healthy: bool,
    pub issue: Option<&'static str>,
    pub network_online: Option<bool>,
    pub update: super::updates::View,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            instance_id: rand::random(),
            version: env!("CARGO_PKG_VERSION"),
            phase: "STARTING".into(),
            failures: 0,
            authentication_id: 0,
            last_authentication_unix: None,
            network: None,
            interfaces: Vec::new(),
            event_watch_healthy: false,
            issue: None,
            network_online: None,
            update: super::updates::View::default(),
        }
    }
}

pub fn query(request: &Request, stored: &Stored, status: &Status, store: &Store) -> Value {
    match request {
        Request::Status | Request::Watch => event(stored, status),
        Request::GetSettings => {
            json!({"ok":true,"settings":stored.settings,"has_password":stored.credential_file.is_some(),"interfaces":super::network::interfaces().unwrap_or_default()})
        }
        Request::Logs => match logs::recent(store) {
            Ok(lines) => json!({"ok":true,"lines":lines}),
            Err(_) => failure("log_unavailable"),
        },
        _ => failure("invalid_request"),
    }
}
pub fn event(stored: &Stored, status: &Status) -> Value {
    json!({"ok":true,"status":status,"success_notifications":stored.settings.success_notifications,"network_error_notifications":stored.settings.network_error_notifications})
}
pub fn publish(sender: &tokio::sync::watch::Sender<Value>, stored: &Stored, status: &Status) {
    let next = event(stored, status);
    sender.send_if_modified(|old| {
        if *old != next {
            *old = next;
            true
        } else {
            false
        }
    });
}
pub fn failure(code: &'static str) -> Value {
    json!({"ok":false,"error":code})
}

fn create(name: &str, owner: &str, first: bool) -> Result<NamedPipeServer> {
    #[cfg(not(test))]
    let descriptor = security::Descriptor::pipe(owner)?;
    #[cfg(test)]
    let descriptor = if name.starts_with(r"\\.\pipe\PortalLoginer.runtime-test.") {
        // Test worker stands in for LocalSystem, which can create more server
        // instances. Production owner clients deliberately lack that right.
        security::validate_sid(owner)?;
        security::Descriptor::from_sddl(&format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{owner})"))?
    } else {
        security::Descriptor::pipe(owner)?
    };
    let mut attrs = descriptor.attributes();
    // CreateNamedPipe copies the descriptor during this synchronous call.
    Ok(unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .max_instances(16)
            .in_buffer_size(16384)
            .out_buffer_size(65536)
            .create_with_security_attributes_raw(
                name,
                (&mut attrs as *mut windows_sys::Win32::Security::SECURITY_ATTRIBUTES).cast(),
            )
    }?)
}

pub(super) async fn serve_at(
    owner: String,
    tx: mpsc::Sender<Envelope>,
    shutdown: CancellationToken,
    name: &str,
    signal: Arc<super::network::Signal>,
    state: tokio::sync::watch::Receiver<Value>,
) -> Result<()> {
    let mut listener = create(name, &owner, true)?;
    let permits = Arc::new(Semaphore::new(8));
    loop {
        tokio::select! {biased;_=shutdown.cancelled()=>return Ok(()),result=listener.connect()=>result?}
        let connected = listener;
        listener = create(name, &owner, false)?;
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            drop(connected);
            continue;
        };
        let owner = owner.clone();
        let tx = tx.clone();
        let stop = shutdown.clone();
        let signal = signal.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let _permit = permit;
            tokio::select! {biased;_=stop.cancelled()=>{},_=connection(connected,&owner,tx,signal,state,stop.clone())=>{}}
        });
    }
}
async fn connection(
    mut pipe: NamedPipeServer,
    owner: &str,
    tx: mpsc::Sender<Envelope>,
    signal: Arc<super::network::Signal>,
    mut state: tokio::sync::watch::Receiver<Value>,
    shutdown: CancellationToken,
) -> Result<()> {
    let bytes = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut pipe, 16384))
        .await
        .map_err(|_| Error::Timeout)??;
    security::authorize_pipe(pipe.as_raw_handle(), owner)?;
    let request: Request = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    if matches!(request, Request::Watch) {
        loop {
            let bytes =
                serde_json::to_vec(&*state.borrow_and_update()).map_err(|_| Error::Invalid)?;
            tokio::time::timeout(
                Duration::from_secs(5),
                write_frame(&mut pipe, &bytes, 65536),
            )
            .await
            .map_err(|_| Error::Timeout)??;
            loop {
                tokio::select! {
                    result=state.changed()=>{result.map_err(|_|Error::Invalid)?;break;},
                    result=pipe.readable()=>{result?;let mut byte=[0u8;1];match pipe.try_read(&mut byte){
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{},
                        _=>return Ok(()),
                    }}
                }
            }
        }
    }
    if matches!(
        request,
        Request::Save { .. } | Request::Pause | Request::Resume
    ) {
        signal.bump();
    }
    let (reply, receive) = oneshot::channel();
    tx.send(Envelope { request, reply })
        .await
        .map_err(|_| Error::Invalid)?;
    let response = tokio::time::timeout(Duration::from_secs(8), receive)
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Invalid)?;
    let data = Zeroizing::new(serde_json::to_vec(&response).map_err(|_| Error::Invalid)?);
    let written =
        tokio::time::timeout(Duration::from_secs(5), write_frame(&mut pipe, &data, 65536))
            .await
            .map_err(|_| Error::Timeout)?;
    if response["shutdown"] == true {
        shutdown.cancel();
    }
    written
}
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    let length = reader.read_u32_le().await? as usize;
    if length == 0 || length > limit {
        return Err(Error::Invalid);
    }
    let mut bytes = Zeroizing::new(vec![0; length]);
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
    limit: usize,
) -> Result<()> {
    if bytes.is_empty() || bytes.len() > limit {
        return Err(Error::Invalid);
    }
    writer.write_u32_le(bytes.len() as u32).await?;
    writer.write_all(bytes).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn client(bytes: &[u8]) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut pipe = open_verified().await?;
        write_frame(&mut pipe, bytes, 16384).await?;
        let response = read_frame(&mut pipe, 65536).await?;
        serde_json::from_slice(&response).map_err(|_| Error::Invalid)
    })
    .await
    .map_err(|_| Error::Timeout)?
}
async fn open_verified() -> Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    tokio::time::timeout(Duration::from_secs(10), async {
        super::host::service_ready()?;
        let pipe = loop {
            match ClientOptions::new().open(PIPE_NAME) {
                Ok(pipe) => break pipe,
                Err(error) if matches!(error.raw_os_error(), Some(231 | 2)) => {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                Err(error) => return Err(error.into()),
            }
        };
        // A squatted pipe must not receive credentials. Match the OS-owned SCM PID.
        let mut pid = 0;
        if unsafe {
            windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId(
                pipe.as_raw_handle(),
                &mut pid,
            )
        } == 0
        {
            return Err(Error::Access);
        }
        super::host::verify_server_pid(pid)?;
        Ok(pipe)
    })
    .await
    .map_err(|_| Error::Timeout)?
}
pub async fn watch_events(
    sender: tokio::sync::mpsc::Sender<Value>,
    stop: CancellationToken,
) -> Result<()> {
    let mut pipe =
        tokio::select! {biased;_=stop.cancelled()=>return Ok(()),result=open_verified()=>result?};
    write_frame(&mut pipe, b"{\"command\":\"watch\"}", 16384).await?;
    loop {
        let bytes = tokio::select! {biased;_=stop.cancelled()=>return Ok(()),result=read_frame(&mut pipe,65536)=>result?};
        let event = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
        tokio::select! {biased;_=stop.cancelled()=>return Ok(()),result=sender.send(event)=>result.map_err(|_|Error::Invalid)?};
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn oversized_frame_is_rejected_before_body_allocation() {
        let (mut a, mut b) = tokio::io::duplex(32);
        a.write_u32_le(16385).await.unwrap();
        assert!(read_frame(&mut b, 16384).await.is_err());
    }
    #[tokio::test]
    async fn named_pipe_uses_owner_identity_and_bounded_framing() {
        let name = format!(
            r"\\.\pipe\PortalLoginer.test.{}.{}",
            std::process::id(),
            rand::random::<u64>()
        );
        let owner = security::current_sid().unwrap();
        let server = create(&name, &owner, true).unwrap();
        let task = tokio::spawn(async move {
            let mut server = server;
            server.connect().await.unwrap();
            let bytes = read_frame(&mut server, 16384).await.unwrap();
            security::authorize_pipe(server.as_raw_handle(), &owner).unwrap();
            assert_eq!(&*bytes, b"{\"command\":\"status\"}");
            write_frame(&mut server, b"{\"ok\":true}", 65536)
                .await
                .unwrap();
        });
        let mut client = ClientOptions::new().open(&name).unwrap();
        write_frame(&mut client, b"{\"command\":\"status\"}", 16384)
            .await
            .unwrap();
        assert_eq!(
            &*read_frame(&mut client, 65536).await.unwrap(),
            b"{\"ok\":true}"
        );
        task.await.unwrap();
    }
    #[test]
    fn status_and_settings_response_do_not_contain_password_key_or_cache() {
        let mut stored = Stored::new(security::current_sid().unwrap());
        stored.key = "abcdefghijklmnop".into();
        let store = Store {
            directory: std::path::PathBuf::new(),
        };
        for command in [Request::Status, Request::GetSettings] {
            let response = query(&command, &stored, &Status::default(), &store).to_string();
            assert!(!response.contains("abcdefghijklmnop"));
            assert!(!response.contains("credential_file"));
            assert!(!response.contains("owner_sid"));
        }
    }
}
