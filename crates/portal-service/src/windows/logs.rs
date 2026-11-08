use super::{storage::Store, Error, Result};
use portal_core::{engine::State, transport::RequestKind};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
pub enum Event {
    Http {
        kind: RequestKind,
        elapsed_ms: u128,
        status: Option<u16>,
        error: Option<portal_core::transport::TransportError>,
    },
    Network {
        index: Option<u32>,
        ip: Option<std::net::Ipv4Addr>,
        dns: Vec<std::net::Ipv4Addr>,
    },
    State(State),
    OnlineCheck,
    Update(&'static str),
    Authenticated,
    ConfigSaved,
    ServiceStarted,
    BootPolicy {
        pause_restored: bool,
        expired_pause: bool,
    },
    NetworkChanged,
    CredentialUnavailable,
    StorageFailure,
    WatchFallback,
    Probe(portal_core::engine::ProbeAttempt),
    Failure(portal_core::engine::Failure),
}
struct Policy {
    maximum: u64,
    debug: bool,
    state: Option<State>,
    online: u64,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            maximum: 256 * 1024,
            debug: false,
            state: None,
            online: 0,
        }
    }
}
static POLICIES: OnceLock<Mutex<HashMap<PathBuf, Policy>>> = OnceLock::new();
fn policies() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Policy>> {
    POLICIES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
pub fn configure(store: &Store, total_maximum: u64, debug: bool) {
    let mut all = policies();
    let policy = all.entry(store.directory.clone()).or_default();
    policy.maximum = total_maximum.clamp(65536, 16777216) / 2;
    policy.debug = debug;
}
impl Policy {
    fn format(&mut self, event: Event) -> Option<String> {
        Some(match event {
            Event::State(State::Checking) => return None,
            Event::State(state) => {
                if self.state == Some(state) {
                    return None;
                }
                self.state = Some(state);
                if state != State::LoggedIn {
                    self.online = 0;
                }
                format!("state={state:?}")
            }
            Event::OnlineCheck => {
                self.online = self.online.saturating_add(1);
                if self.online % 10 != 0 {
                    return None;
                }
                format!(
                    "state=LoggedIn online_checks={} probe_http=204",
                    self.online
                )
            }
            Event::Authenticated => {
                self.online = 0;
                "authentication_succeeded".into()
            }
            Event::Http {
                kind,
                elapsed_ms,
                status,
                error,
            } => {
                if matches!(kind, RequestKind::Probe) && status == Some(204) && error.is_none() {
                    return None;
                }
                if !self.debug
                    && !matches!(kind, RequestKind::Probe)
                    && error.is_none()
                    && status.is_some_and(|v| v < 400)
                {
                    return None;
                }
                if error.is_some() || status.is_some_and(|v| v >= 400) {
                    self.online = 0;
                }
                format!(
                    "http kind={kind:?} elapsed_ms={elapsed_ms} status={status:?} error={error:?}"
                )
            }
            Event::Probe(attempt) => {
                if attempt.status == Some(204)
                    && attempt.error.is_none()
                    && !attempt.invalid_redirect
                {
                    return None;
                }
                self.online = 0;
                format!(
                    "probe={} http={:?} error={:?} invalid_redirect={}",
                    attempt.index, attempt.status, attempt.error, attempt.invalid_redirect
                )
            }
            Event::Network { index, ip, dns } => {
                if !self.debug {
                    return None;
                }
                format!("network interface={index:?} source={ip:?} dns={dns:?}")
            }
            Event::NetworkChanged => {
                self.online = 0;
                self.state = None;
                "network_changed".into()
            }
            Event::Failure(failure) => {
                self.online = 0;
                format!("failure={failure:?}")
            }
            Event::Update(code) => format!("update={code}"),
            Event::BootPolicy {
                pause_restored,
                expired_pause,
            } => {
                format!("boot_policy pause_restored={pause_restored} expired_pause={expired_pause}")
            }
            Event::ServiceStarted => format!(
                "service_started version={} pid={}",
                env!("CARGO_PKG_VERSION"),
                std::process::id()
            ),
            Event::ConfigSaved => "config_saved".into(),
            Event::CredentialUnavailable => {
                self.online = 0;
                "credential_unavailable".into()
            }
            Event::StorageFailure => {
                self.online = 0;
                "storage_failure".into()
            }
            Event::WatchFallback => "event_watch_fallback".into(),
        })
    }
}
pub fn record(store: &Store, event: Event) -> Result<()> {
    let mut all = policies();
    let policy = all.entry(store.directory.clone()).or_default();
    // Suppression precedes ALL filesystem access. Configuration is held in memory.
    let Some(code) = policy.format(event) else {
        return Ok(());
    };
    let path = store.directory.join("service.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() >= policy.maximum) {
        let old = store.directory.join("service.previous.log");
        if old.exists() {
            fs::remove_file(&old)?;
        }
        fs::rename(&path, old)?;
    }
    let mut file = OpenOptions::new().append(true).create(true).open(path)?;
    writeln!(
        file,
        "{} {code}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    )?;
    Ok(())
}
pub fn recent(store: &Store) -> Result<String> {
    let path = store.directory.join("service.log");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e.into()),
    };
    use std::os::windows::fs::MetadataExt;
    if !metadata.is_file()
        || metadata.file_attributes() & 0x400 != 0
        || metadata.len() > 16 * 1024 * 1024
    {
        return Err(Error::Access);
    }
    let mut file = File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(16 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(16 * 1024).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(if start == 0 {
        text.into_owned()
    } else {
        text.split_once('\n')
            .map(|(_, tail)| tail)
            .unwrap_or("")
            .to_owned()
    })
}
#[cfg(test)]
mod tests {
    use super::super::storage;
    use super::*;
    fn online() -> Event {
        Event::Probe(portal_core::engine::ProbeAttempt {
            index: 0,
            status: Some(204),
            error: None,
            invalid_redirect: false,
        })
    }
    #[test]
    fn log_rotation_and_tail_are_bounded() {
        let fixture = storage::TestStore::new();
        fs::write(
            fixture.store.directory.join("service.log"),
            vec![b'x'; 256 * 1024],
        )
        .unwrap();
        record(&fixture.store, Event::State(State::Paused)).unwrap();
        assert_eq!(
            fs::metadata(fixture.store.directory.join("service.previous.log"))
                .unwrap()
                .len(),
            256 * 1024
        );
        assert!(recent(&fixture.store).unwrap().len() < 16384);
    }
    #[test]
    fn normal_online_checks_only_write_every_ten_without_configuration_reads() {
        let fixture = storage::TestStore::new();
        let store = &fixture.store;
        configure(store, 524288, true);
        record(store, Event::State(State::LoggedIn)).unwrap();
        let before = recent(store).unwrap();
        for _ in 0..9 {
            record(store, Event::State(State::Checking)).unwrap();
            record(store, online()).unwrap();
            record(
                store,
                Event::Http {
                    kind: RequestKind::Probe,
                    elapsed_ms: 12,
                    status: Some(204),
                    error: None,
                },
            )
            .unwrap();
            record(store, Event::State(State::LoggedIn)).unwrap();
            record(store, Event::OnlineCheck).unwrap();
        }
        assert_eq!(before, recent(store).unwrap());
        assert!(!store.directory.join("settings.json").exists());
        record(store, Event::OnlineCheck).unwrap();
        assert!(recent(store).unwrap().contains("online_checks=10"));
    }
    #[test]
    fn anomaly_is_immediate_and_recovery_restarts_summary_counter() {
        let fixture = storage::TestStore::new();
        let store = &fixture.store;
        configure(store, 524288, false);
        for _ in 0..9 {
            record(store, Event::OnlineCheck).unwrap();
        }
        assert!(!store.directory.join("service.log").exists());
        record(
            store,
            Event::Http {
                kind: RequestKind::Probe,
                elapsed_ms: 6000,
                status: None,
                error: Some(portal_core::transport::TransportError::Timeout),
            },
        )
        .unwrap();
        assert!(recent(store).unwrap().contains("Timeout"));
        for _ in 0..9 {
            record(store, Event::OnlineCheck).unwrap();
        }
        assert!(!recent(store).unwrap().contains("online_checks="));
        record(store, Event::OnlineCheck).unwrap();
        assert!(recent(store).unwrap().contains("online_checks=10"));
    }
}
