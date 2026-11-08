use portal_core::{
    config::{Config, Credentials, Network, DEFAULT_PROBE},
    engine::{Engine, Failure, LocalClock, State, Trigger},
    transport::{Request, RequestKind, Response, Transport, TransportError},
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const PORTAL: &str =
    "http://192.168.199.200/index.html?bras=hn&clientip=10.23.45.67&clientmac=aa:bb:cc:dd:ee:ff";
enum Reply {
    Http(u16, Option<&'static str>, &'static str),
    Error,
    Timeout,
    Hang,
}
struct Mock {
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<Request>>,
    resets: AtomicUsize,
}
impl Mock {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
            resets: AtomicUsize::new(0),
        }
    }
    fn empty() -> Self {
        Self::new(Vec::new())
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn append(&self, replies: Vec<Reply>) {
        self.replies.lock().unwrap().extend(replies);
    }
}
impl Transport for Mock {
    async fn execute(&self, request: Request) -> Result<Response, TransportError> {
        self.requests.lock().unwrap().push(request);
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request");
        match reply {
            Reply::Http(status, location, body) => Ok(Response {
                status,
                location: location.map(str::to_owned),
                body: Zeroizing::new(body.as_bytes().to_vec()),
            }),
            Reply::Error => Err(TransportError::Unavailable),
            Reply::Timeout => Err(TransportError::Timeout),
            Reply::Hang => std::future::pending().await,
        }
    }
    fn reset_session(&self) {
        self.resets.fetch_add(1, Ordering::SeqCst);
    }
}
fn online() -> Reply {
    Reply::Http(204, None, "")
}
fn portal() -> Reply {
    Reply::Http(302, Some(PORTAL), "")
}
fn ok() -> Reply {
    Reply::Http(200, None, r#"{"ret":0}"#)
}
fn network() -> Network {
    Network {
        generation: 1,
        ipv4: Some("10.23.45.67".parse().unwrap()),
        route_ready: true,
        is_wifi: true,
        ssid: Some("Campus".into()),
    }
}
fn credentials() -> Credentials {
    Credentials::new("test+user@net".into(), "synthetic-password".into()).unwrap()
}
fn script_key() -> Reply {
    Reply::Http(200, None, "Utf8.parse('5a3b9f207411a8ed')")
}
#[tokio::test(start_paused = true)]
async fn desktop_recovery_uses_cache_then_backup_with_one_shared_budget() {
    let mock = Mock::new(vec![
        Reply::Error,
        script_key(),
        ok(),
        Reply::Http(200, None, r#"{"ret":-1}"#),
        portal(),
        script_key(),
        ok(),
        ok(),
        ok(),
        online(),
    ]);
    let mut engine = Engine::new(Config {
        desktop_policy: true,
        ..Default::default()
    })
    .unwrap();
    let net = network();
    engine.restore_cache(
        portal_core::protocol::Portal::parse(PORTAL, net.ipv4.unwrap())
            .unwrap()
            .cache(),
    );
    warm(&mut engine, &mock, &net).await;
    assert!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .authenticated
    );
    assert_eq!(
        mock.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.url.contains("/api/login.php"))
            .count(),
        2
    );
}
#[tokio::test(start_paused = true)]
async fn desktop_recovery_stops_after_three_total_authentication_attempts() {
    let mock = Mock::new(vec![
        portal(),
        script_key(),
        ok(),
        Reply::Http(200, None, r#"{"ret":-1}"#),
        script_key(),
        ok(),
        Reply::Http(200, None, r#"{"ret":-1}"#),
        script_key(),
        ok(),
        Reply::Http(200, None, r#"{"ret":-1}"#),
    ]);
    let mut engine = Engine::new(Config {
        desktop_policy: true,
        ..Default::default()
    })
    .unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .state,
        State::Failed
    );
    assert_eq!(
        mock.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.url.contains("/api/login.php"))
            .count(),
        3
    );
}
#[tokio::test(start_paused = true)]
async fn desktop_confirmed_authentication_without_internet_requires_manual_action() {
    let mock = Mock::new(vec![
        portal(),
        script_key(),
        ok(),
        ok(),
        ok(),
        Reply::Error,
        Reply::Error,
        Reply::Error,
    ]);
    let mut engine = Engine::new(Config {
        desktop_policy: true,
        ..Default::default()
    })
    .unwrap();
    let net = network();
    let report = run(&mut engine, &mock, &net, Trigger::Login).await;
    assert_eq!(report.state, State::ManualRequired);
    assert!(!report.authenticated);
    assert_eq!(
        mock.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.url.contains("/api/login.php"))
            .count(),
        1
    );
}
#[tokio::test(start_paused = true)]
async fn desktop_logout_bootstraps_with_current_ip_and_school_preset() {
    let mock = Mock::new(vec![ok()]);
    let mut engine = Engine::new(Config {
        desktop_policy: true,
        ..Default::default()
    })
    .unwrap();
    let net = network();
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Logout).await.state,
        State::Paused
    );
    assert_eq!(
        mock.requests.lock().unwrap()[0].url,
        "http://192.168.199.200/api/logoff.php?bras=mlyx&clientip=10.23.45.67"
    );
    assert!(mock.requests.lock().unwrap()[0].body.is_none());
}

#[tokio::test(start_paused = true)]
async fn upstream_manual_login_runs_immediately_without_daemon_stabilization_or_key_fetch() {
    let mock = Mock::new(vec![portal(), ok(), ok(), ok(), online()]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let started = tokio::time::Instant::now();
    let report = run(&mut engine, &mock, &network(), Trigger::Login).await;
    assert!(report.authenticated);
    assert_eq!(tokio::time::Instant::now() - started, Duration::ZERO);
    assert_eq!(mock.count(), 5);
    assert!(mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|request| !matches!(request.kind, RequestKind::Script)));
}

#[tokio::test(start_paused = true)]
async fn upstream_ack_failure_with_confirmed_internet_is_success() {
    let mock = Mock::new(vec![
        portal(),
        ok(),
        ok(),
        Reply::Http(200, None, r#"{"ret":1}"#),
        online(),
    ]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let report = run(&mut engine, &mock, &network(), Trigger::Login).await;
    assert!(report.authenticated);
    assert!(report.failure.is_none());
}

#[tokio::test(start_paused = true)]
async fn pause_and_config_reload_preserve_matching_portal_cache_for_manual_relogin() {
    let mock = Mock::new(vec![portal(), ok(), ok(), ok(), online()]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    assert!(
        run(&mut engine, &mock, &net, Trigger::Login)
            .await
            .authenticated
    );
    let cache = engine.session_cache();
    engine
        .update_config(Config {
            paused: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .state,
        State::Paused
    );
    assert!(engine.session_cache() == cache);
    engine.update_config(Config::default()).unwrap();
    mock.append(vec![online(), ok(), online(), ok(), ok(), ok(), online()]);
    assert!(
        run(&mut engine, &mock, &net, Trigger::Relogin)
            .await
            .authenticated
    );
}
async fn warm(engine: &mut Engine, transport: &Mock, net: &Network) {
    let report = engine
        .step(
            transport,
            &credentials(),
            net,
            Trigger::Automatic,
            None,
            &CancellationToken::new(),
        )
        .await;
    assert_eq!(report.state, State::WaitNetwork);
    tokio::time::advance(Duration::from_secs(11)).await;
}
async fn run(
    engine: &mut Engine,
    transport: &Mock,
    net: &Network,
    trigger: Trigger,
) -> portal_core::engine::Report {
    engine
        .step(
            transport,
            &credentials(),
            net,
            trigger,
            None,
            &CancellationToken::new(),
        )
        .await
}

#[tokio::test(start_paused = true)]
async fn automatic_authentication_order_and_online_dedup() {
    let mock = Mock::new(vec![
        portal(),
        Reply::Http(200, None, "Utf8.parse('0123456789abcdef')"),
        ok(),
        ok(),
        ok(),
        online(),
        online(),
    ]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    let report = run(&mut engine, &mock, &net, Trigger::Automatic).await;
    assert_eq!(report.state, State::LoggedIn);
    assert!(report.authenticated);
    assert_eq!(report.changed_key.unwrap().expose(), "0123456789abcdef");
    let login_time = engine.last_authenticated();
    let next = run(&mut engine, &mock, &net, Trigger::Automatic).await;
    assert!(!next.authenticated);
    assert_eq!(engine.last_authenticated(), login_time);
    let requests = mock.requests.lock().unwrap();
    assert_eq!(requests.len(), 7);
    assert_eq!(requests[0].url, DEFAULT_PROBE);
    assert!(requests[1].url.ends_with("/assets/js/raas.js"));
    assert!(requests[2].url.contains("/index.html?"));
    assert!(!requests[2].post);
    assert!(requests[3].url.contains("/api/login.php?"));
    assert!(requests[3].post);
    assert!(requests[4].url.contains("/api/ack_auth.php?"));
    assert_eq!(requests[3].body.as_deref(), requests[4].body.as_deref());
    assert!(requests[3]
        .body
        .as_ref()
        .unwrap()
        .contains("user=test%2Buser%40net"));
    assert_eq!(
        requests[3].referer.as_deref(),
        Some("http://192.168.199.200/index.html?bras=hn")
    );
    assert!(requests[3].accept_json);
    assert!(!requests[4].accept_json);
}

#[tokio::test(start_paused = true)]
async fn manual_login_preserves_upstream_key_and_tolerates_session_get_failure() {
    let mock = Mock::new(vec![
        portal(),
        Reply::Error,
        Reply::Http(302, None, r#"{"ret":0}"#),
        ok(),
        online(),
    ]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    let report = run(&mut engine, &mock, &net, Trigger::Login).await;
    assert!(report.authenticated);
    assert!(report.changed_key.is_none());
    assert!(!mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| matches!(request.kind, RequestKind::Script)));
}

#[tokio::test(start_paused = true)]
async fn no_requests_while_wifi_mismatches_unknown_or_wired() {
    for (wifi, ssid) in [
        (true, Some("campus")),
        (true, None),
        (false, Some("Campus")),
    ] {
        let mock = Mock::empty();
        let config = Config {
            wifi_only: Some("Campus".into()),
            ..Config::default()
        };
        let mut engine = Engine::new(config).unwrap();
        let mut net = network();
        net.is_wifi = wifi;
        net.ssid = ssid.map(str::to_owned);
        assert_eq!(
            run(&mut engine, &mock, &net, Trigger::Relogin).await.state,
            State::WaitWifi
        );
        assert_eq!(mock.count(), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn missing_network_route_and_pause_never_send_requests() {
    let mock = Mock::empty();
    let mut engine = Engine::new(Config::default()).unwrap();
    let mut net = network();
    net.ipv4 = None;
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .state,
        State::Offline
    );
    net.ipv4 = network().ipv4;
    net.route_ready = false;
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .state,
        State::WaitNetwork
    );
    let config = Config {
        paused: true,
        ..Config::default()
    };
    engine.update_config(config).unwrap();
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .state,
        State::Paused
    );
    assert_eq!(mock.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn unknown_probe_uses_all_backups_and_does_not_authenticate() {
    let mock = Mock::new(vec![
        Reply::Error,
        Reply::Http(200, None, "html"),
        Reply::Http(503, None, ""),
    ]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    let report = run(&mut engine, &mock, &net, Trigger::Relogin).await;
    assert_eq!(report.state, State::ProbeUnknown);
    assert_eq!(report.failures, 0);
    assert_eq!(mock.count(), 3);
    assert_eq!(report.retry_after, Some(Duration::from_secs(30)));
    assert_eq!(report.probes.len(), 3);
    assert_eq!(report.probes[0].error, Some(TransportError::Unavailable));
    assert_eq!(report.probes[1].status, Some(200));
    assert_eq!(report.probes[2].status, Some(503));
}

#[tokio::test(start_paused = true)]
async fn invalid_redirect_is_reported_without_sending_credentials() {
    let mock = Mock::new(vec![portal(), portal(), portal()]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let mut net = network();
    net.ipv4 = Some("192.168.55.102".parse().unwrap());
    warm(&mut engine, &mock, &net).await;
    let report = run(&mut engine, &mock, &net, Trigger::Login).await;
    assert_eq!(report.state, State::ProbeUnknown);
    assert!(report.probes.iter().all(|attempt| attempt.invalid_redirect));
    assert_eq!(mock.count(), 3);
}

#[tokio::test(start_paused = true)]
async fn pending_confirmation_does_not_count_as_failure() {
    let mock = Mock::new(vec![
        portal(),
        ok(),
        ok(),
        ok(),
        Reply::Error,
        Reply::Error,
        Reply::Error,
    ]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    let report = run(&mut engine, &mock, &net, Trigger::Login).await;
    assert_eq!(report.state, State::Verifying);
    assert_eq!(report.failures, 0);
    assert!(!report.authenticated);
    assert_eq!(engine.last_authenticated(), None);
}

#[tokio::test(start_paused = true)]
async fn login_failure_stops_before_ack_and_backoff_caps_then_resets() {
    let mock = Mock::empty();
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    for seconds in [10, 20, 40, 80, 160, 300, 300] {
        mock.append(vec![
            portal(),
            ok(),
            Reply::Http(200, None, r#"{"ret":-1}"#),
        ]);
        let report = run(&mut engine, &mock, &net, Trigger::Login).await;
        assert_eq!(report.state, State::Failed);
        assert_eq!(report.retry_after, Some(Duration::from_secs(seconds)));
    }
    assert_eq!(mock.count(), 21);
    mock.append(vec![online()]);
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .failures,
        0
    );
}

#[tokio::test(start_paused = true)]
async fn cancellation_prevents_ack_and_discards_session() {
    let mock = Mock::new(vec![portal(), ok(), Reply::Hang]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        child.cancel();
    });
    let report = engine
        .step(&mock, &credentials(), &net, Trigger::Login, None, &cancel)
        .await;
    assert_eq!(report.failure, Some(Failure::Cancelled));
    assert!(!report.authenticated);
    assert_eq!(mock.count(), 3);
    assert_eq!(mock.resets.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn pre_cancelled_token_and_generation_change_prevent_requests() {
    let mock = Mock::empty();
    let mut engine = Engine::new(Config::default()).unwrap();
    let mut net = network();
    warm(&mut engine, &mock, &net).await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        engine
            .step(&mock, &credentials(), &net, Trigger::Login, None, &cancel)
            .await
            .failure,
        Some(Failure::Cancelled)
    );
    net.generation += 1;
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .state,
        State::WaitNetwork
    );
    assert_eq!(mock.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn hanging_transaction_has_total_deadline() {
    let mock = Mock::new(vec![Reply::Hang]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .failure,
        Some(Failure::TransactionTimeout)
    );
}

#[tokio::test(start_paused = true)]
async fn scheduled_logout_runs_once_and_preserves_manual_fallback_difference() {
    let mock = Mock::new(vec![portal(), ok(), ok(), ok(), online()]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    assert!(
        run(&mut engine, &mock, &net, Trigger::Login)
            .await
            .authenticated
    );
    mock.append(vec![online(), ok(), online(), online()]);
    let clock = Some(LocalClock {
        date: 20261108,
        hour: 4,
    });
    let report = engine
        .step(
            &mock,
            &credentials(),
            &net,
            Trigger::Automatic,
            clock,
            &CancellationToken::new(),
        )
        .await;
    assert!(!report.authenticated);
    assert_eq!(report.state, State::LoggedIn);
    let report = engine
        .step(
            &mock,
            &credentials(),
            &net,
            Trigger::Automatic,
            clock,
            &CancellationToken::new(),
        )
        .await;
    assert!(!report.authenticated);
    assert_eq!(
        mock.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.url.contains("logoff.php"))
            .count(),
        1
    );
    mock.append(vec![online(), ok(), online(), ok(), ok(), ok(), online()]);
    assert!(
        run(&mut engine, &mock, &net, Trigger::Relogin)
            .await
            .authenticated
    );
}

#[tokio::test(start_paused = true)]
async fn restored_cache_never_turns_unknown_probe_into_login() {
    let mock = Mock::new(vec![Reply::Error, Reply::Error, Reply::Error]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    let cache = portal_core::protocol::Portal::parse(PORTAL, net.ipv4.unwrap())
        .unwrap()
        .cache();
    assert!(engine.restore_cache(cache));
    warm(&mut engine, &mock, &net).await;
    assert!(engine.session_cache().is_some());
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .state,
        State::ProbeUnknown
    );
    assert_eq!(mock.count(), 3);
    let stale = Network {
        generation: 2,
        ipv4: Some("10.23.45.68".parse().unwrap()),
        ..net
    };
    assert_eq!(
        run(&mut engine, &mock, &stale, Trigger::Automatic)
            .await
            .state,
        State::WaitNetwork
    );
    assert!(engine.session_cache().is_none());
}

#[tokio::test(start_paused = true)]
async fn cancelling_scheduled_logout_does_not_repeat_same_day() {
    let mock = Mock::new(vec![portal(), ok(), ok(), ok(), online()]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    assert!(
        run(&mut engine, &mock, &net, Trigger::Login)
            .await
            .authenticated
    );
    mock.append(vec![online(), Reply::Hang]);
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        child.cancel();
    });
    let clock = Some(LocalClock {
        date: 20261108,
        hour: 4,
    });
    assert_eq!(
        engine
            .step(
                &mock,
                &credentials(),
                &net,
                Trigger::Automatic,
                clock,
                &cancel
            )
            .await
            .failure,
        Some(Failure::Cancelled)
    );
    mock.append(vec![online()]);
    assert_eq!(
        engine
            .step(
                &mock,
                &credentials(),
                &net,
                Trigger::Automatic,
                clock,
                &CancellationToken::new()
            )
            .await
            .state,
        State::LoggedIn
    );
    assert_eq!(
        mock.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.url.contains("logoff.php"))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn desktop_check_uses_backups_without_authenticating() {
    let mock = Mock::new(vec![Reply::Error, portal()]);
    let mut engine = Engine::new(Config {
        desktop_policy: true,
        ..Default::default()
    })
    .unwrap();
    let report = run(&mut engine, &mock, &network(), Trigger::Check).await;
    assert_eq!(report.state, State::AuthenticationRequired);
    assert_eq!(mock.count(), 2);
    assert!(!report.authenticated);
}
#[tokio::test(start_paused = true)]
async fn desktop_manual_login_never_validates_credentials_from_existing_internet() {
    let mock = Mock::new(vec![online()]);
    let mut engine = Engine::new(Config {
        desktop_policy: true,
        ..Default::default()
    })
    .unwrap();
    let report = run(&mut engine, &mock, &network(), Trigger::Login).await;
    assert_eq!(report.state, State::NeedsLogout);
    assert!(!report.authenticated);
    assert_eq!(mock.count(), 1);
}

#[tokio::test(start_paused = true)]
async fn saving_settings_does_not_restart_network_stabilization() {
    let mock = Mock::new(vec![online(), online()]);
    let mut engine = Engine::new(Config::default()).unwrap();
    let net = network();
    warm(&mut engine, &mock, &net).await;
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .state,
        State::LoggedIn
    );
    engine.update_config(Config::default()).unwrap();
    assert_eq!(
        run(&mut engine, &mock, &net, Trigger::Automatic)
            .await
            .state,
        State::LoggedIn
    );
}

#[tokio::test(start_paused = true)]
async fn desktop_login_continues_after_rejected_or_timed_out_logout() {
    for logout in [Reply::Http(200, None, r#"{"ret":-1}"#), Reply::Timeout] {
        let mock = Mock::new(vec![
            logout,
            portal(),
            script_key(),
            ok(),
            ok(),
            ok(),
            online(),
        ]);
        let mut engine = Engine::new(Config {
            desktop_policy: true,
            ..Default::default()
        })
        .unwrap();
        let report = run(&mut engine, &mock, &network(), Trigger::Relogin).await;
        assert_eq!(report.state, State::LoggedIn);
        assert!(report.authenticated);
        assert_eq!(report.failure, None);
        let requests = mock.requests.lock().unwrap();
        assert!(requests[0].url.contains("logoff.php"));
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.url.contains("login.php"))
                .count(),
            1
        );
    }
}
#[tokio::test(start_paused = true)]
async fn desktop_login_keeps_matching_cache_after_failed_logout_and_probe() {
    let mock = Mock::new(vec![
        Reply::Error,
        Reply::Error,
        script_key(),
        ok(),
        ok(),
        ok(),
        online(),
    ]);
    let mut engine = Engine::new(Config {
        desktop_policy: true,
        ..Default::default()
    })
    .unwrap();
    let net = network();
    engine.restore_cache(
        portal_core::protocol::Portal::parse(PORTAL, net.ipv4.unwrap())
            .unwrap()
            .cache(),
    );
    let report = run(&mut engine, &mock, &net, Trigger::Relogin).await;
    assert_eq!(report.state, State::LoggedIn);
    assert!(report.authenticated);
    assert_eq!(
        mock.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.url.contains("login.php"))
            .count(),
        1
    );
}
#[tokio::test(start_paused = true)]
async fn desktop_login_uses_cache_when_logout_did_not_interrupt_existing_internet() {
    let mock = Mock::new(vec![
        Reply::Http(200, None, r#"{"ret":-1}"#),
        online(),
        script_key(),
        ok(),
        ok(),
        ok(),
        online(),
    ]);
    let mut engine = Engine::new(Config {
        desktop_policy: true,
        ..Default::default()
    })
    .unwrap();
    let net = network();
    engine.restore_cache(
        portal_core::protocol::Portal::parse(PORTAL, net.ipv4.unwrap())
            .unwrap()
            .cache(),
    );
    assert!(
        run(&mut engine, &mock, &net, Trigger::Relogin)
            .await
            .authenticated
    );
}
#[tokio::test(start_paused = true)]
async fn desktop_login_reports_existing_internet_without_faking_new_authentication() {
    let mock = Mock::new(vec![Reply::Error, online()]);
    let mut engine = Engine::new(Config {
        desktop_policy: true,
        ..Default::default()
    })
    .unwrap();
    let report = run(&mut engine, &mock, &network(), Trigger::Relogin).await;
    assert_eq!(report.state, State::LoggedIn);
    assert!(!report.authenticated);
    assert_eq!(mock.count(), 2);
}
