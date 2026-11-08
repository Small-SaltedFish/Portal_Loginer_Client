use crate::{
    config::{Config, ConfigError, Credentials, Network, BACKUP_PROBES},
    crypto::{encrypt, Key},
    protocol::{self, Endpoint, LoginResult, Portal, Probe, SessionCache, KEY_PATHS},
    transport::{Request, RequestKind, Response, Transport},
};
use serde::Serialize;
use std::{
    net::Ipv4Addr,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum State {
    Idle,
    Checking,
    WaitNetwork,
    WaitWifi,
    Paused,
    Offline,
    AuthenticationRequired,
    LoggingOut,
    LoggingIn,
    Verifying,
    LoggedIn,
    ProbeUnknown,
    Failed,
    NeedsLogout,
    NetworkError,
    ManualRequired,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Automatic,
    Check,
    Login,
    Relogin,
    Logout,
    BootRelogin,
}

/// Calendar date encoded YYYYMMDD by the platform, plus local hour 0..23.
/// Elapsed time and all waits use Instant, never this wall-clock value.
#[derive(Clone, Copy)]
pub struct LocalClock {
    pub date: u32,
    pub hour: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Failure {
    LoginRejected,
    LoginTransport,
    InvalidLoginResponse,
    Encryption,
    Confirmation,
    Logout,
    Cancelled,
    TransactionTimeout,
    WrongTransport,
}

pub struct Report {
    pub state: State,
    /// None: wait for a network/config event; service supplies low-rate fallback.
    pub retry_after: Option<Duration>,
    pub failures: u32,
    pub authenticated: bool,
    pub failure: Option<Failure>,
    /// Persistence is a platform responsibility. This key is never status data.
    pub changed_key: Option<Key>,
    pub probes: Vec<ProbeAttempt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ProbeAttempt {
    pub index: usize,
    pub status: Option<u16>,
    pub error: Option<crate::transport::TransportError>,
    pub invalid_redirect: bool,
}

pub struct Engine {
    config: Config,
    state: watch::Sender<State>,
    generation: Option<u64>,
    address: Option<Ipv4Addr>,
    cached: Option<Portal>,
    restored_cache: Option<SessionCache>,
    stable_since: Option<Instant>,
    reset_session: bool,
    failures: u32,
    last_authenticated: Option<Instant>,
    last_relogin_date: Option<u32>,
    probes: Vec<ProbeAttempt>,
}

impl Engine {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        config.validate()?;
        let (state, _) = watch::channel(State::Idle);
        Ok(Self {
            config,
            state,
            generation: None,
            address: None,
            cached: None,
            restored_cache: None,
            stable_since: None,
            reset_session: false,
            failures: 0,
            last_authenticated: None,
            last_relogin_date: None,
            probes: Vec::new(),
        })
    }
    pub fn subscribe(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }
    pub fn state(&self) -> State {
        *self.state.borrow()
    }
    pub fn last_authenticated(&self) -> Option<Instant> {
        self.last_authenticated
    }
    pub fn session_cache(&self) -> Option<SessionCache> {
        self.cached.as_ref().map(Portal::cache)
    }
    /// Only before the first network snapshot. Restored data is validated later
    /// against that snapshot; it can never trigger automatic login by itself.
    pub fn restore_cache(&mut self, cache: SessionCache) -> bool {
        if self.generation.is_some() {
            return false;
        }
        self.restored_cache = Some(cache);
        true
    }
    pub fn update_config(&mut self, config: Config) -> Result<(), ConfigError> {
        config.validate()?;
        self.config = config;
        // Settings changes reset cookies, not the physical link stability timer.
        self.reset_session = true;
        Ok(())
    }
    fn report(
        &self,
        state: State,
        wait: Option<Duration>,
        failure: Option<Failure>,
        authenticated: bool,
        key: Option<Key>,
    ) -> Report {
        self.state.send_replace(state);
        Report {
            state,
            retry_after: wait,
            failures: self.failures,
            authenticated,
            failure,
            changed_key: key,
            probes: self.probes.clone(),
        }
    }

    /// The mutable borrow serializes transactions. The service must cancel the
    /// token on any network/config/pause event before applying a newer snapshot.
    pub async fn step<T: Transport>(
        &mut self,
        transport: &T,
        credentials: &Credentials,
        network: &Network,
        trigger: Trigger,
        clock: Option<LocalClock>,
        cancel: &CancellationToken,
    ) -> Report {
        let now = Instant::now();
        self.probes.clear();
        if self.reset_session {
            transport.reset_session();
            self.reset_session = false;
        }
        if self.generation != Some(network.generation) || self.address != network.ipv4 {
            transport.reset_session();
            self.cached = None;
            self.failures = 0;
            self.generation = Some(network.generation);
            self.address = network.ipv4;
            self.stable_since = network.ipv4.map(|_| now);
            if let Some(cache) = self.restored_cache.take() {
                self.cached = network.ipv4.and_then(|ip| cache.restore(ip).ok());
            }
        }
        if self.config.paused && trigger != Trigger::Check {
            return self.report(State::Paused, None, None, false, None);
        }
        if !network.wifi_matches(self.config.wifi_only.as_deref()) {
            self.cached = None;
            transport.reset_session();
            return self.report(State::WaitWifi, None, None, false, None);
        }
        let Some(ip) = network.ipv4 else {
            return self.report(
                State::Offline,
                Some(self.config.retry_interval),
                None,
                false,
                None,
            );
        };
        if transport.source_ipv4().is_some_and(|source| source != ip) {
            return self.report(
                State::WaitNetwork,
                None,
                Some(Failure::WrongTransport),
                false,
                None,
            );
        }
        if !network.route_ready {
            self.stable_since = Some(now);
            return self.report(
                State::WaitNetwork,
                Some(self.config.retry_interval),
                None,
                false,
                None,
            );
        }
        if matches!(trigger, Trigger::Automatic | Trigger::BootRelogin)
            && self
                .stable_since
                .is_some_and(|since| now.saturating_duration_since(since) < Duration::from_secs(10))
        {
            return self.report(
                State::WaitNetwork,
                Some(Duration::from_secs(3)),
                None,
                false,
                None,
            );
        }
        self.state.send_replace(State::Checking);
        let scheduled_attempt = AtomicBool::new(false);
        let future = work(
            transport,
            credentials,
            &self.config,
            WorkContext {
                ip,
                trigger,
                cached: self.cached.clone(),
                scheduled: self.scheduled_due(trigger, clock),
                state: &self.state,
                scheduled_attempt: &scheduled_attempt,
            },
        );
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(Failure::Cancelled),
            value = tokio::time::timeout(Duration::from_secs(120), future) => value.map_err(|_| Failure::TransactionTimeout),
        };
        if scheduled_attempt.load(Ordering::SeqCst) {
            self.last_relogin_date = clock.map(|time| time.date);
        }
        let work = match result {
            Ok(work) => work,
            Err(failure) => {
                self.cached = None;
                transport.reset_session();
                return self.report(
                    State::WaitNetwork,
                    Some(self.config.retry_interval),
                    Some(failure),
                    false,
                    None,
                );
            }
        };
        self.cached = work.cached;
        self.probes = work.probes;
        if let Some(key) = &work.key {
            self.config.key = key.clone();
        }
        match work.outcome {
            Outcome::NeedsLogout => self.report(State::NeedsLogout, None, None, false, work.key),
            Outcome::NetworkError => self.report(
                State::NetworkError,
                Some(self.config.check_interval),
                None,
                false,
                work.key,
            ),
            Outcome::ManualRequired => {
                self.report(State::ManualRequired, None, None, false, work.key)
            }
            Outcome::LoggedOut => self.report(State::Paused, None, None, false, work.key),
            Outcome::Online => {
                self.failures = 0;
                self.report(
                    State::LoggedIn,
                    Some(self.config.check_interval),
                    None,
                    false,
                    work.key,
                )
            }
            Outcome::Authenticated => {
                self.failures = 0;
                self.last_authenticated = Some(Instant::now());
                self.last_relogin_date = clock.map(|time| time.date).or(self.last_relogin_date);
                self.report(
                    State::LoggedIn,
                    Some(self.config.check_interval),
                    None,
                    true,
                    work.key,
                )
            }
            Outcome::Pending => self.report(
                State::Verifying,
                Some(self.config.retry_interval),
                None,
                false,
                work.key,
            ),
            Outcome::Failed(failure) => {
                self.failures = self.failures.saturating_add(1);
                let delay = self
                    .config
                    .retry_interval
                    .as_secs()
                    .saturating_mul(1u64 << self.failures.saturating_sub(1).min(9))
                    .min(300);
                self.report(
                    State::Failed,
                    Some(if self.config.desktop_policy {
                        self.config.check_interval
                    } else {
                        Duration::from_secs(delay)
                    }),
                    Some(failure),
                    false,
                    work.key,
                )
            }
            Outcome::Unknown => self.report(
                State::ProbeUnknown,
                Some(self.config.check_interval),
                None,
                false,
                work.key,
            ),
            Outcome::Captive => self.report(
                State::AuthenticationRequired,
                Some(self.config.check_interval),
                None,
                false,
                work.key,
            ),
        }
    }

    fn scheduled_due(&self, trigger: Trigger, clock: Option<LocalClock>) -> bool {
        trigger == Trigger::Automatic
            && clock.is_some_and(|time| {
                time.hour <= 23
                    && self.config.relogin_hour == Some(time.hour)
                    && self.last_relogin_date != Some(time.date)
            })
    }
}

enum Outcome {
    Online,
    Authenticated,
    Pending,
    Failed(Failure),
    Unknown,
    Captive,
    NeedsLogout,
    NetworkError,
    ManualRequired,
    LoggedOut,
}
struct Work {
    outcome: Outcome,
    cached: Option<Portal>,
    key: Option<Key>,
    probes: Vec<ProbeAttempt>,
}
struct WorkContext<'a> {
    ip: Ipv4Addr,
    trigger: Trigger,
    cached: Option<Portal>,
    scheduled: bool,
    state: &'a watch::Sender<State>,
    scheduled_attempt: &'a AtomicBool,
}

async fn probe<T: Transport>(
    transport: &T,
    config: &Config,
    ip: Ipv4Addr,
    attempts: &mut Vec<ProbeAttempt>,
) -> Probe {
    for (index, url) in std::iter::once(config.probe_url.as_str())
        .chain(BACKUP_PROBES)
        .enumerate()
    {
        match transport
            .execute(Request::get(url.to_owned(), RequestKind::Probe))
            .await
        {
            Ok(response) => {
                let result =
                    protocol::classify_probe(response.status, response.location.as_deref(), ip);
                attempts.push(ProbeAttempt {
                    index,
                    status: Some(response.status),
                    error: None,
                    invalid_redirect: response.location.is_some() && result == Probe::Unknown,
                });
                if result != Probe::Unknown {
                    return result;
                }
            }
            Err(error) => attempts.push(ProbeAttempt {
                index,
                status: None,
                error: Some(error),
                invalid_redirect: false,
            }),
        }
    }
    Probe::Unknown
}

pub async fn inspect<T: Transport>(
    transport: &T,
    config: &Config,
    ip: Ipv4Addr,
) -> (Probe, Vec<ProbeAttempt>) {
    let mut attempts = Vec::new();
    let result = probe(transport, config, ip, &mut attempts).await;
    (result, attempts)
}

fn api_get(portal: &Portal) -> Request {
    let mut request = Request::get(portal.session_url(), RequestKind::Api);
    request.referer = Some(portal.referer());
    request
}
fn api_post(
    portal: &Portal,
    endpoint: Endpoint,
    body: Option<zeroize::Zeroizing<String>>,
) -> Request {
    let mut request = Request::get(
        portal.api_url(endpoint),
        if endpoint == Endpoint::Logoff {
            RequestKind::Logoff
        } else {
            RequestKind::Api
        },
    );
    request.post = true;
    if endpoint != Endpoint::Logoff {
        request.referer = Some(portal.referer());
    }
    request.accept_json = endpoint == Endpoint::Login;
    request.body = body;
    request
}
fn good(response: Result<Response, crate::transport::TransportError>) -> Option<Response> {
    response.ok().filter(Response::successful)
}

async fn work<T: Transport>(
    transport: &T,
    credentials: &Credentials,
    config: &Config,
    context: WorkContext<'_>,
) -> Work {
    if config.desktop_policy {
        return desktop_flow::run(transport, credentials, config, context).await;
    }
    let WorkContext {
        ip,
        trigger,
        mut cached,
        scheduled,
        state,
        scheduled_attempt,
    } = context;
    let mut probes = Vec::new();
    let mut result = probe(transport, config, ip, &mut probes).await;
    if let Probe::Portal(portal) = &result {
        cached = Some(portal.clone());
    }
    let mut report = Work {
        outcome: Outcome::Unknown,
        cached,
        key: None,
        probes,
    };
    if result == Probe::Unknown {
        return report;
    }
    if trigger == Trigger::Check {
        report.outcome = if result == Probe::Online {
            Outcome::Online
        } else {
            Outcome::Captive
        };
        return report;
    }
    let relogin = result == Probe::Online && (trigger == Trigger::Relogin || scheduled);
    if relogin {
        if scheduled {
            scheduled_attempt.store(true, Ordering::SeqCst);
        }
        if let Some(portal) = &report.cached {
            if portal.clientip() != ip {
                report.outcome = Outcome::Failed(Failure::Logout);
                return report;
            }
            let response = good(
                transport
                    .execute(api_post(portal, Endpoint::Logoff, None))
                    .await,
            );
            if !response.is_some_and(|r| protocol::ack_accepted(&r.body)) {
                report.outcome = if scheduled {
                    Outcome::Online
                } else {
                    Outcome::Failed(Failure::Logout)
                };
                return report;
            }
        }
        transport.reset_session();
        tokio::time::sleep(Duration::from_secs(2)).await;
        result = probe(transport, config, ip, &mut report.probes).await;
        if let Probe::Portal(portal) = &result {
            report.cached = Some(portal.clone());
        }
        // Only manual relogin may use matching cached parameters while still online.
        if result == Probe::Online && trigger != Trigger::Relogin {
            report.outcome = Outcome::Online;
            return report;
        }
        if result == Probe::Unknown {
            return report;
        }
    } else if result == Probe::Online {
        report.outcome = Outcome::Online;
        return report;
    }
    authenticate(transport, credentials, config, trigger, state, report, ip).await
}

async fn authenticate<T: Transport>(
    transport: &T,
    credentials: &Credentials,
    config: &Config,
    trigger: Trigger,
    state: &watch::Sender<State>,
    mut report: Work,
    ip: Ipv4Addr,
) -> Work {
    let Some(portal) = &report.cached else {
        report.outcome = Outcome::Failed(Failure::Logout);
        return report;
    };
    // Match upstream: only the daemon discovers keys.
    if trigger == Trigger::Automatic || config.desktop_policy {
        for path in KEY_PATHS {
            if let Some(response) = good(
                transport
                    .execute(Request::get(
                        format!("{}{path}", portal.host()),
                        RequestKind::Script,
                    ))
                    .await,
            ) {
                if let Some(key) = std::str::from_utf8(&response.body)
                    .ok()
                    .and_then(protocol::detect_key)
                {
                    if key.expose() != config.key.expose() {
                        report.key = Some(key);
                    }
                    break;
                }
            }
        }
    }
    state.send_replace(State::LoggingIn);
    let encrypted = match encrypt(
        credentials.password(),
        report.key.as_ref().unwrap_or(&config.key),
    ) {
        Ok(value) => value,
        Err(_) => {
            report.outcome = Outcome::Failed(Failure::Encryption);
            return report;
        }
    };
    let _ = transport.execute(api_get(portal)).await;
    let body = protocol::form(credentials.user(), &encrypted);
    let response = good(
        transport
            .execute(api_post(portal, Endpoint::Login, Some(body.clone())))
            .await,
    );
    let failure = match response {
        None => Some(Failure::LoginTransport),
        Some(response) if protocol::response_ret(&response.body).is_none() => {
            Some(Failure::InvalidLoginResponse)
        }
        Some(response) if !protocol::login_accepted(&response.body) => Some(Failure::LoginRejected),
        Some(_) => None,
    };
    if let Some(failure) = failure {
        report.outcome = Outcome::Failed(failure);
        return report;
    }
    let ack = good(
        transport
            .execute(api_post(portal, Endpoint::Ack, Some(body)))
            .await,
    )
    .is_some_and(|r| protocol::ack_accepted(&r.body));
    state.send_replace(State::Verifying);
    let verified = probe(transport, config, ip, &mut report.probes).await;
    report.outcome = if config.desktop_policy && ack && verified != Probe::Online {
        Outcome::ManualRequired
    } else {
        match protocol::finish_login(ack, &verified) {
            LoginResult::Success => Outcome::Authenticated,
            LoginResult::Pending => Outcome::Pending,
            LoginResult::Failed => Outcome::Failed(Failure::Confirmation),
        }
    };
    if let Probe::Portal(portal) = verified {
        report.cached = Some(portal);
    }
    report
}

#[path = "desktop_flow.rs"]
mod desktop_flow;
