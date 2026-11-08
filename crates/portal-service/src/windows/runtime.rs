use super::{
    ipc::{self, Envelope, Request, Status},
    logs::{self, Event},
    network::{self, Signal, Snapshot},
    settings::Stored,
    storage::Store,
    Error, Result,
};
use portal_core::{
    engine::{Engine, State, Trigger},
    transport::{DirectTransport, Request as HttpRequest, Response, Transport, TransportError},
};
use std::{
    net::Ipv4Addr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Adapter(Option<DirectTransport>, std::path::PathBuf);
impl Transport for Adapter {
    async fn execute(&self, request: HttpRequest) -> std::result::Result<Response, TransportError> {
        let kind = request.kind;
        let started = std::time::Instant::now();
        let result = match &self.0 {
            Some(client) => client.execute(request).await,
            None => Err(TransportError::Unavailable),
        };
        {
            let directory = &self.1;
            let _ = logs::record(
                &Store {
                    directory: directory.clone(),
                },
                Event::Http {
                    kind,
                    elapsed_ms: started.elapsed().as_millis(),
                    status: result.as_ref().ok().map(|r| r.status),
                    error: result.as_ref().err().copied(),
                },
            );
        }
        result
    }
    fn reset_session(&self) {
        if let Some(client) = &self.0 {
            client.reset_session();
        }
    }
    fn source_ipv4(&self) -> Option<Ipv4Addr> {
        self.0.as_ref().and_then(Transport::source_ipv4)
    }
}
pub fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(2)
        .enable_all()
        .build()?)
}

pub async fn run(store: Store, shutdown: CancellationToken, signal: Arc<Signal>) -> Result<()> {
    run_at(store, shutdown, signal, super::PIPE_NAME.to_owned()).await
}
async fn run_at(
    store: Store,
    shutdown: CancellationToken,
    signal: Arc<Signal>,
    pipe_name: String,
) -> Result<()> {
    let mut stored = store.load()?;
    logs::configure(
        &store,
        stored.settings.log_max_bytes,
        stored.settings.debug_log,
    );
    let _ = logs::record(&store, Event::ServiceStarted);
    let mut engine = Engine::new(stored.settings.core(&stored.key)?).map_err(|_| Error::Invalid)?;
    if let Some(cache) = stored.cache.clone() {
        engine.restore_cache(cache);
    }
    let update_control = super::updates::Control::new(stored.settings.automatic_updates);
    let mut update_events = update_control.state.subscribe();
    let (update_ready_tx, mut update_ready_rx) = mpsc::channel(1);
    #[cfg(not(test))]
    let _update_worker = tokio::spawn(super::updates::worker(
        Store {
            directory: store.directory.clone(),
        },
        update_control.clone(),
        update_ready_tx,
        shutdown.clone(),
    ));
    #[cfg(test)]
    drop(update_ready_tx);
    let mut prepared_update: Option<super::updates::Ready> = None;
    let mut update_ready_open = true;
    let mut stages = engine.subscribe();
    let mut events = network::Events::register(signal.clone());
    let mut status = Status {
        event_watch_healthy: events.healthy,
        ..Status::default()
    };
    if !events.healthy {
        let _ = logs::record(&store, Event::WatchFallback);
    }
    let (tx, mut commands) = mpsc::channel(32);
    let ipc_shutdown = shutdown.clone();
    let ipc_stop = shutdown.clone();
    let owner = stored.owner_sid.clone();
    let (state_sender, state_receiver) = tokio::sync::watch::channel(ipc::event(&stored, &status));
    let ipc_signal = signal.clone();
    let server = tokio::spawn(async move {
        let result =
            ipc::serve_at(owner, tx, ipc_stop, &pipe_name, ipc_signal, state_receiver).await;
        ipc_shutdown.cancel();
        result
    });
    #[cfg(not(test))]
    {
        let _startup_task = tokio::task::spawn_blocking(move || {
            if super::startup::refresh_saved().is_err() {
                if let Ok(store) = Store::system() {
                    let _ = logs::record(&store, Event::StartupTaskFailed);
                }
            }
        });
    }
    let mut transport = Adapter(None, store.directory.clone());
    let mut previous: Option<Snapshot> = None;
    let mut generation = 0;
    let mut physical_generation = 0;
    let mut due = tokio::time::Instant::now();
    let mut dirty = true;
    let mut pending: Option<Envelope> = None;
    let boot = super::security::boot_id()?;
    let had_pause = stored.paused_boot.is_some();
    normalize_boot_pause(&mut stored, &boot);
    store.save(&stored)?;
    let _ = logs::record(
        &store,
        Event::BootPolicy {
            pause_restored: paused_this_boot(&stored, &boot),
            expired_pause: had_pause && stored.paused_boot.is_none(),
        },
    );
    let mut next_trigger = if stored.settings.force_boot_login
        && stored.credential_file.is_some()
        && stored.last_boot.as_ref() != Some(&boot)
        && !paused_this_boot(&stored, &boot)
    {
        Trigger::BootRelogin
    } else {
        Trigger::Automatic
    };
    let mut session_paused = paused_this_boot(&stored, &boot);
    let mut configuration: Option<(Stored, zeroize::Zeroizing<String>)> = None;
    let mut fallback =
        tokio::time::interval(Duration::from_secs(if events.healthy { 300 } else { 60 }));
    fallback.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    fallback.tick().await;
    loop {
        update_control.automatic.store(
            stored.settings.automatic_updates,
            std::sync::atomic::Ordering::SeqCst,
        );
        update_control.online.store(
            status.network_online == Some(true),
            std::sync::atomic::Ordering::SeqCst,
        );
        logs::configure(
            &store,
            stored.settings.log_max_bytes,
            stored.settings.debug_log,
        );
        if shutdown.is_cancelled() {
            break;
        }
        if let Some(envelope) = pending.take() {
            if matches!(
                envelope.request,
                Request::CheckUpdates | Request::InstallUpdate | Request::UserActivity { .. }
            ) {
                match &envelope.request {
                    Request::UserActivity { busy } => update_control.activity(*busy),
                    Request::InstallUpdate => update_control.request(true),
                    _ => update_control.request(false),
                };
                let _ = envelope.reply.send(serde_json::json!({"ok":true}));
                continue;
            }
            if let Request::Configure { settings, password } = &envelope.request {
                let prepared = (|| -> Result<_> {
                    settings.core(&stored.key)?;
                    let password = if password.is_empty() {
                        if settings.user != stored.settings.user {
                            return Err(Error::Invalid);
                        }
                        let bytes = store.password(&stored)?.ok_or(Error::Credentials)?;
                        String::from_utf8(bytes.to_vec()).map_err(|_| Error::Credentials)?
                    } else {
                        password.clone()
                    };
                    super::settings::credentials(&settings.user, password.as_bytes())?;
                    Ok(password)
                })();
                match prepared {
                    Ok(password) => {
                        let previous = stored.clone();
                        stored.settings = settings.clone();
                        stored.settings.paused = false;
                        engine
                            .update_config(stored.settings.core(&stored.key)?)
                            .map_err(|_| Error::Invalid)?;
                        configuration = Some((previous, zeroize::Zeroizing::new(password)));
                        session_paused = false;
                        next_trigger = Trigger::Login;
                        due = tokio::time::Instant::now();
                        dirty = true;
                        status.phase = "CHECKING".into();
                        let _ = envelope.reply.send(serde_json::json!({"ok":true}));
                    }
                    Err(_) => {
                        let _ = envelope.reply.send(ipc::failure("invalid_settings"));
                    }
                }
                continue;
            }

            if matches!(envelope.request, Request::Logout) {
                session_paused = true;
                pause_for_boot(&store, &mut stored, &boot)?;
            }
            if matches!(envelope.request, Request::Relogin | Request::Login) {
                stored.paused_boot = None;
                store.save(&stored)?;
                session_paused = false;
            }
            apply(
                envelope,
                &mut stored,
                &mut engine,
                &store,
                &mut status,
                &mut next_trigger,
            );
            dirty = true;
            due = tokio::time::Instant::now();
            ipc::publish(&state_sender, &stored, &status);
            if status.phase == "EXITING" {
                // No further authentication after an exit request. Let the IPC
                // connection send its acknowledgement before closing all pipes.
                let _ = tokio::time::timeout(Duration::from_secs(6), shutdown.cancelled()).await;
                shutdown.cancel();
                break;
            }
        }
        if dirty {
            // Bounded debounce: callbacks already cancelled the in-flight round.
            tokio::select! {biased;_=shutdown.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(400))=>{}}
            let required = stored.settings.wifi_only.clone();
            let selected_index = stored.settings.interface_index;
            let snapshot_generation = signal.generation();
            let (snapshot, interfaces) = tokio::task::spawn_blocking(move || {
                network::snapshot_and_interfaces(required.as_deref(), selected_index)
            })
            .await
            .map_err(|_| Error::Invalid)??;
            // Publish the completed snapshot even during a burst of route events.
            // A changed epoch schedules another scan; it must not starve startup status.
            generation = snapshot_generation;
            if previous.as_ref() != Some(&snapshot) {
                status.network_online = None;
                if previous
                    .as_ref()
                    .is_none_or(|old| !old.same_link(&snapshot))
                {
                    physical_generation += 1;
                    let _ = logs::record(&store, Event::NetworkChanged);
                }
                transport = Adapter(
                    snapshot
                        .ipv4
                        .and_then(|ip| DirectTransport::new(ip, &snapshot.dns).ok()),
                    store.directory.clone(),
                );
                if stored.settings.debug_log {
                    let _ = logs::record(
                        &store,
                        Event::Network {
                            index: snapshot.interface_index,
                            ip: snapshot.ipv4,
                            dns: snapshot.dns.clone(),
                        },
                    );
                }
            }
            status.interfaces = interfaces;
            status.network = Some(snapshot.clone());
            previous = Some(snapshot);
            dirty = generation != signal.generation();
            if dirty {
                status.phase = "WAIT_NETWORK".into();
            }
            ipc::publish(&state_sender, &stored, &status);
        }
        if session_paused
            && next_trigger == Trigger::Automatic
            && tokio::time::Instant::now() >= due
        {
            status.phase = "PAUSED".into();
            due = tokio::time::Instant::now() + Duration::from_secs(60);
        }
        if tokio::time::Instant::now() >= due && generation == signal.generation() {
            let password = if let Some((_, password)) = &configuration {
                Ok(Some(zeroize::Zeroizing::new(password.as_bytes().to_vec())))
            } else {
                store.password(&stored)
            };
            let mut configuration_result = None;
            let credentials = password
                .as_ref()
                .ok()
                .and_then(|value| value.as_ref())
                .and_then(|password| {
                    super::settings::credentials(&stored.settings.user, password).ok()
                })
                .or_else(|| {
                    (next_trigger == Trigger::Logout).then(|| {
                        portal_core::config::Credentials::new("unused".into(), "unused".into())
                            .expect("nonempty")
                    })
                });
            let authenticated_round = credentials.is_some();
            if let Some(credentials) = credentials {
                let token = signal.prepare();
                let snapshot = previous
                    .as_ref()
                    .expect("initial snapshot")
                    .network(physical_generation);
                let trigger = next_trigger;
                let step = engine.step(
                    &transport,
                    &credentials,
                    &snapshot,
                    trigger,
                    Some(network::local_clock()),
                    &token,
                );
                tokio::pin!(step);
                loop {
                    tokio::select! {
                        _=shutdown.cancelled()=>{token.cancel();let _=step.await;break;},
                        _=signal.notify.notified()=>{token.cancel();let _=step.await;dirty=true;due=tokio::time::Instant::now();status.phase="WAIT_NETWORK".into();break;},
                        Some(envelope)=commands.recv()=>{
                            if envelope.request.read_only(){let _=envelope.reply.send(ipc::query(&envelope.request,&stored,&status,&store));}
                            else if matches!(envelope.request,Request::CheckUpdates|Request::InstallUpdate|Request::UserActivity{..}){match &envelope.request {Request::UserActivity{busy}=>update_control.activity(*busy),Request::InstallUpdate=>update_control.request(true),_=>update_control.request(false)};let _=envelope.reply.send(serde_json::json!({"ok":true}));}
                            else if matches!(envelope.request,Request::Check|Request::Relogin){let _=envelope.reply.send(ipc::failure("busy"));}
                            else {token.cancel();let _=step.await;pending=Some(envelope);break;}
                        },
                        _=update_events.changed()=>{status.update=update_events.borrow_and_update().clone();ipc::publish(&state_sender,&stored,&status);},
                        _=stages.changed()=>{let state=*stages.borrow_and_update();update_phase(&mut status,state,&store);ipc::publish(&state_sender,&stored,&status);},
                        report=&mut step=>{
                            if signal.generation()!=generation {dirty=true;due=tokio::time::Instant::now();status.phase="WAIT_NETWORK".into();break;}
                            update_phase(&mut status,report.state,&store);status.failures=report.failures;
                            if report.state==State::ManualRequired || (trigger==Trigger::Logout && report.state==State::Paused){session_paused=true;stored.paused_boot=Some(boot.clone());if configuration.is_none(){let _=store.save(&stored);}}
                            if !report.probes.is_empty(){status.network_online=probe_online(&report.probes);}
                            if !matches!(report.state, State::WaitNetwork | State::Offline | State::WaitWifi) {if trigger==Trigger::BootRelogin {stored.last_boot=Some(boot.clone());let _=store.save(&stored);} next_trigger=Trigger::Automatic; }
                            for attempt in &report.probes { let _=logs::record(&store,Event::Probe(*attempt)); }
                            if report.state==State::LoggedIn && !report.authenticated && !report.probes.is_empty() && report.probes.iter().all(|p|p.status==Some(204)&&p.error.is_none()&&!p.invalid_redirect) {let _=logs::record(&store,Event::OnlineCheck);}
                            if let Some(failure)=report.failure {let _=logs::record(&store,Event::Failure(failure));}
                            status.issue=report.failure.map(|failure| match failure {
                                portal_core::engine::Failure::LoginRejected=>"login_rejected",
                                portal_core::engine::Failure::LoginTransport=>"login_transport_failed",
                                portal_core::engine::Failure::InvalidLoginResponse=>"login_invalid_response",
                                portal_core::engine::Failure::Confirmation=>"confirmation_failed",
                                _=>"authentication_or_transport_failure",
                            });
                            if report.state == State::ProbeUnknown {
                                status.issue=Some(if report.probes.iter().any(|p|p.invalid_redirect) {"invalid_portal_redirect"}
                                else if report.probes.iter().all(|p|p.status.is_none()) {"probe_unreachable"} else {"probe_unexpected_response"});
                            }
                            if configuration.is_some() {configuration_result=Some(report.authenticated);}
                            if report.authenticated && configuration.is_none() {let _=logs::record(&store,Event::Authenticated);status.authentication_id=status.authentication_id.saturating_add(1);
                                status.last_authentication_unix=Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs());}
                            let mut changed=false;
                            if let Some(key)=report.changed_key {stored.key=key.expose().to_owned();changed=true;}
                            if changed && configuration.is_none() && store.save(&stored).is_err(){status.issue=Some("storage_failure");let _=logs::record(&store,Event::StorageFailure);}
                            if trigger==Trigger::Logout && report.state==State::Paused && stored.credential_file.is_none(){next_trigger=Trigger::Check;}
                            due=tokio::time::Instant::now()+if next_trigger==Trigger::Check {Duration::from_secs(2)}else{report.retry_after.unwrap_or(Duration::from_secs(60))};break;
                        }
                    }
                }
                // The step future has released the mutable engine borrow here.
            } else {
                if status.phase != "NEEDS_CONFIGURATION" {
                    status.phase = "NEEDS_CONFIGURATION".into();
                    if password.is_err() {
                        let _ = logs::record(&store, Event::CredentialUnavailable);
                    }
                }
                status.issue = if password.is_err() {
                    Some("credential_unavailable")
                } else {
                    None
                };
                due =
                    tokio::time::Instant::now() + stored.settings.core(&stored.key)?.check_interval;
                let snapshot = previous.as_ref().expect("initial snapshot");
                let network = snapshot.network(physical_generation);
                if !network.route_ready {
                    status.phase = "OFFLINE".into();
                } else if !network.wifi_matches(stored.settings.wifi_only.as_deref()) {
                    status.phase = "WAIT_WIFI".into();
                }
                if (!stored.settings.paused || next_trigger == Trigger::Check)
                    && network.route_ready
                    && network.wifi_matches(stored.settings.wifi_only.as_deref())
                {
                    if let Some(ip) = snapshot.ipv4 {
                        let config = stored.settings.core(&stored.key)?;
                        let inspection = portal_core::engine::inspect(&transport, &config, ip);
                        tokio::pin!(inspection);
                        let inspected = loop {
                            tokio::select! {biased;
                                _=shutdown.cancelled()=>break None,
                                _=signal.notify.notified()=>{dirty=true;break None},
                                Some(command)=commands.recv()=>{if command.request.read_only(){let _=command.reply.send(ipc::query(&command.request,&stored,&status,&store));}else if matches!(command.request,Request::CheckUpdates|Request::InstallUpdate|Request::UserActivity{..}){match &command.request {Request::UserActivity{busy}=>update_control.activity(*busy),Request::InstallUpdate=>update_control.request(true),_=>update_control.request(false)};let _=command.reply.send(serde_json::json!({"ok":true}));}else{pending=Some(command);break None;}},
                                result=&mut inspection=>break Some(result),
                            }
                        };
                        if let Some((probe, attempts)) = inspected {
                            status.network_online = if probe == portal_core::protocol::Probe::Online
                            {
                                Some(true)
                            } else if matches!(probe, portal_core::protocol::Probe::Portal(_)) {
                                Some(false)
                            } else {
                                None
                            };
                            next_trigger = Trigger::Automatic;
                            for attempt in attempts {
                                let _ = logs::record(&store, Event::Probe(attempt));
                            }
                            if let portal_core::protocol::Probe::Portal(ref portal) = probe {
                                stored.cache = Some(portal.cache());
                                engine.restore_cache(portal.cache());
                                let _ = store.save(&stored);
                            }
                            status.phase = if probe == portal_core::protocol::Probe::Online {
                                let endpoint =
                                    format!("{}/index.html", stored.settings.logout_host);
                                match transport
                                    .execute(HttpRequest::get(
                                        endpoint,
                                        portal_core::transport::RequestKind::Script,
                                    ))
                                    .await
                                {
                                    Ok(r)
                                        if (200..400).contains(&r.status)
                                            && r.body.windows(7).any(|w| w == b"raas.js") =>
                                    {
                                        "NEEDS_LOGOUT"
                                    }
                                    _ => {
                                        status.issue = Some("portal_unavailable");
                                        "MANUAL_REQUIRED"
                                    }
                                }
                            } else if matches!(probe, portal_core::protocol::Probe::Portal(_)) {
                                "AUTHENTICATION_REQUIRED"
                            } else {
                                "PROBE_UNKNOWN"
                            }
                            .into();
                        }
                    }
                }
            }
            if configuration.is_some() && configuration_result.is_none() {
                configuration_result = Some(false);
                status.phase = "VERIFICATION_CANCELLED".into();
                status.issue = Some("verification_interrupted");
            }
            if let Some(success) = configuration_result {
                if let Some((previous, password)) = configuration.take() {
                    if success {
                        stored.cache = engine.session_cache();
                        stored.credentials_verified = true;
                        stored.paused_boot = None;
                        stored.last_boot = Some(boot.clone());
                        if store.change_password(&mut stored, &password).is_ok() {
                            let _ = logs::record(&store, Event::Authenticated);
                            status.authentication_id = status.authentication_id.saturating_add(1);
                            #[cfg(not(test))]
                            if super::host::set_autostart(stored.settings.autostart).is_err() {
                                status.issue = Some("autostart_failure");
                            }
                            let _ = logs::record(&store, Event::ConfigSaved);
                        } else {
                            stored = previous;
                            status.phase = "FAILED".into();
                            status.issue = Some("storage_failure");
                            session_paused = true;
                        }
                    } else {
                        stored = previous;
                        stored.paused_boot = Some(boot.clone());
                        let _ = store.save(&stored);
                        session_paused = true;
                    }
                    engine
                        .update_config(stored.settings.core(&stored.key)?)
                        .map_err(|_| Error::Invalid)?;
                }
            }
            let cache = engine.session_cache();
            let quitting = shutdown.is_cancelled()
                || pending
                    .as_ref()
                    .is_some_and(|command| matches!(command.request, Request::Shutdown));
            if authenticated_round
                && configuration.is_none()
                && !session_paused
                && !quitting
                && cache != stored.cache
            {
                stored.cache = cache;
                if store.save(&stored).is_err() {
                    status.issue = Some("storage_failure");
                }
            }
            if dirty || pending.is_some() {
                ipc::publish(&state_sender, &stored, &status);
                continue;
            }
        }
        if let Ok(ready) = update_ready_rx.try_recv() {
            prepared_update = Some(ready);
        }
        if let Some(ready) = &prepared_update {
            let allowed = ready.manual
                || stored.settings.automatic_updates
                || update_control
                    .force_staged
                    .load(std::sync::atomic::Ordering::SeqCst);
            if allowed
                && configuration.is_none()
                && super::uninstall::installed_directory()
                    .ok()
                    .flatten()
                    .is_some_and(|dir| super::updates::can_install(&update_control, &dir))
            {
                update_control.show("installing", "正在静默更新，完成后托盘会恢复");
                status.phase = "UPDATING".into();
                status.update.phase = "installing".into();
                status.update.message = "正在静默更新，完成后托盘会恢复".into();
                ipc::publish(&state_sender, &stored, &status);
                if super::updates::launch(ready).is_ok() {
                    prepared_update = None;
                    update_control
                        .force_staged
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                    tokio::select! {_=shutdown.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(25))=>{update_control.show("failed","更新进程未能接管，稍后重试");update_control.staged.store(false,std::sync::atomic::Ordering::SeqCst);update_phase(&mut status,engine.state(),&store);}}
                } else {
                    prepared_update = None;
                    update_control
                        .staged
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                    update_control.show("failed", "无法启动更新进程，请稍后重试");
                    update_phase(&mut status, engine.state(), &store);
                }
            } else {
                update_control.show("deferred", "更新已就绪，等待设置关闭或全屏应用结束");
            }
        }
        status.update = update_events.borrow_and_update().clone();
        ipc::publish(&state_sender, &stored, &status);
        tokio::select! {biased;
            _=update_events.changed()=>{status.update=update_events.borrow_and_update().clone();},
            ready=update_ready_rx.recv(),if prepared_update.is_none()&&update_ready_open=>{if let Some(ready)=ready {prepared_update=Some(ready);}else{update_ready_open=false;}},
            _=shutdown.cancelled()=>break,
            _=signal.notify.notified()=>{dirty=true;due=tokio::time::Instant::now();},
            _=signal.schedule.notified()=>{due=tokio::time::Instant::now();},
            Some(envelope)=commands.recv()=>{
                if envelope.request.read_only(){let _=envelope.reply.send(ipc::query(&envelope.request,&stored,&status,&store));}
                else {pending=Some(envelope);}
            },
            _=fallback.tick()=>{
                if !events.healthy {
                    events = network::Events::register(signal.clone());
                    status.event_watch_healthy = events.healthy;
                }
                let required=stored.settings.wifi_only.clone();let selected_index=stored.settings.interface_index;
                let (current, interfaces)=tokio::task::spawn_blocking(move||network::snapshot_and_interfaces(required.as_deref(),selected_index)).await.map_err(|_|Error::Invalid)??;
                status.interfaces = interfaces;
                if previous.as_ref()!=Some(&current){signal.bump();dirty=true;due=tokio::time::Instant::now();}
            },
            _=tokio::time::sleep_until(due)=>{}
        }
    }
    shutdown.cancel();
    server.await.map_err(|_| Error::Invalid)??;
    Ok(())
}
fn normalize_boot_pause(stored: &mut Stored, boot: &str) {
    // Legacy environment-only markers cannot identify a boot reliably.
    // Keep a pause only when the complete, versioned boot identity matches.
    if stored.paused_boot.as_deref() != Some(boot) {
        stored.paused_boot = None;
    }
}
fn paused_this_boot(stored: &Stored, boot: &str) -> bool {
    stored.paused_boot.as_deref() == Some(boot)
}
fn pause_for_boot(store: &Store, stored: &mut Stored, boot: &str) -> Result<()> {
    stored.settings.paused = false;
    stored.paused_boot = Some(boot.to_owned());
    store.save(stored)
}
fn update_phase(status: &mut Status, state: State, store: &Store) {
    let value = serde_json::to_value(state)
        .expect("state enum")
        .as_str()
        .expect("state text")
        .to_owned();
    if status.phase != value {
        status.phase = value;
        let _ = logs::record(store, Event::State(state));
    }
}
fn probe_online(probes: &[portal_core::engine::ProbeAttempt]) -> Option<bool> {
    let probe = probes.last()?;
    match probe.status {
        Some(204) => Some(true),
        Some(301 | 302 | 303 | 307 | 308) if !probe.invalid_redirect => Some(false),
        _ => None,
    }
}
fn apply(
    envelope: Envelope,
    stored: &mut Stored,
    engine: &mut Engine,
    store: &Store,
    status: &mut Status,
    next: &mut Trigger,
) {
    let result = (|| -> Result<()> {
        match &envelope.request {
            Request::SelectInterface { index } => {
                stored.settings.interface_index = *index;
                store.save(stored)?;
                engine
                    .update_config(stored.settings.core(&stored.key)?)
                    .map_err(|_| Error::Invalid)?;
            }
            Request::Shutdown => {
                status.phase = "EXITING".into();
            }
            Request::Save { settings, password } => {
                settings.core(&stored.key)?;
                if !password.is_empty() || settings.user != stored.settings.user {
                    return Err(Error::Invalid);
                }
                let mut candidate = Stored {
                    schema: stored.schema,
                    owner_sid: stored.owner_sid.clone(),
                    settings: settings.clone(),
                    credential_file: stored.credential_file.clone(),
                    key: stored.key.clone(),
                    cache: engine.session_cache().or_else(|| stored.cache.clone()),
                    last_boot: stored.last_boot.clone(),
                    paused_boot: stored.paused_boot.clone(),
                    credentials_verified: stored.credentials_verified
                        && password.is_empty()
                        && settings.user == stored.settings.user,
                };
                store.change_password(&mut candidate, password)?;
                *stored = candidate;
                #[cfg(not(test))]
                super::host::set_autostart(stored.settings.autostart)?;
                engine
                    .update_config(stored.settings.core(&stored.key)?)
                    .map_err(|_| Error::Invalid)?;
                let _ = logs::record(store, Event::ConfigSaved);
            }
            Request::Pause | Request::Resume => {
                let old = stored.settings.paused;
                stored.settings.paused = matches!(envelope.request, Request::Pause);
                if let Err(error) = store.save(stored) {
                    stored.settings.paused = old;
                    return Err(error);
                }
                engine
                    .update_config(stored.settings.core(&stored.key)?)
                    .map_err(|_| Error::Invalid)?;
            }
            Request::Logout => {
                stored.settings.paused = false;
                store.save(stored)?;
                engine
                    .update_config(stored.settings.core(&stored.key)?)
                    .map_err(|_| Error::Invalid)?;
                *next = Trigger::Logout;
            }
            Request::Check | Request::Login | Request::Relogin => {
                if (matches!(envelope.request, Request::Login | Request::Relogin)
                    && stored.credential_file.is_none())
                    || (stored.settings.paused
                        && !matches!(
                            envelope.request,
                            Request::Login | Request::Relogin | Request::Check
                        ))
                {
                    return Err(Error::Invalid);
                }
                if matches!(envelope.request, Request::Login | Request::Relogin)
                    && stored.settings.paused
                {
                    let mut candidate = stored.clone();
                    candidate.settings.paused = false;
                    store.save(&candidate)?;
                    *stored = candidate;
                    engine
                        .update_config(stored.settings.core(&stored.key)?)
                        .map_err(|_| Error::Invalid)?;
                }
                *next = if matches!(envelope.request, Request::Check) {
                    Trigger::Check
                } else if matches!(envelope.request, Request::Login) {
                    Trigger::Login
                } else {
                    Trigger::Relogin
                };
            }
            _ => return Err(Error::Invalid),
        }
        status.issue = None;
        Ok(())
    })();
    let response = if result.is_ok() {
        serde_json::json!({"ok":true,"shutdown":status.phase=="EXITING"})
    } else {
        ipc::failure("operation_failed")
    };
    let _ = envelope.reply.send(response);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use tokio::net::windows::named_pipe::ClientOptions;
    #[tokio::test]
    async fn application_exit_acknowledges_before_closing_ipc_and_terminates_service_loop() {
        let fixture = super::super::storage::TestStore::new();
        let mut stored = Stored::new(super::super::security::current_sid().unwrap());
        stored.settings.paused = true;
        fixture.store.save(&stored).unwrap();
        let name = format!(
            r"\\.\pipe\PortalLoginer.runtime-test.{}.{}",
            std::process::id(),
            rand::random::<u64>()
        );
        let stop = CancellationToken::new();
        let service = run_at(
            Store {
                directory: fixture.store.directory.clone(),
            },
            stop,
            Signal::new(),
            name.clone(),
        );
        let driver = async {
            let result = request(&name, json!({"command":"shutdown"})).await;
            assert_eq!(result["ok"], true);
            assert_eq!(result["shutdown"], true);
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(service, driver)
        })
        .await
        .unwrap();
        result.unwrap();
        assert!(fixture.store.load().unwrap().settings.paused);
        assert_eq!(fixture.store.load().unwrap().credential_file, None);
    }
    #[test]
    fn new_os_boot_clears_logout_pause_even_when_environment_guid_is_reused() {
        let mut stored = Stored::new(super::super::security::current_sid().unwrap());
        stored.paused_boot = Some("v2-same-environment-00000010".into());
        normalize_boot_pause(&mut stored, "v2-same-environment-00000010");
        assert!(paused_this_boot(&stored, "v2-same-environment-00000010"));
        normalize_boot_pause(&mut stored, "v2-same-environment-00000011");
        assert!(stored.paused_boot.is_none());
        stored.paused_boot = Some("legacy-environment".into());
        normalize_boot_pause(&mut stored, "v2-same-environment-00000011");
        assert!(stored.paused_boot.is_none());
        assert!(!stored.settings.paused);
    }
    #[test]
    fn manual_login_resumes_pause_and_check_is_accepted_while_network_stabilizes() {
        let fixture = super::super::storage::TestStore::new();
        let mut stored = Stored::new(super::super::security::current_sid().unwrap());
        stored.settings.user = "synthetic-user".into();
        stored.settings.paused = true;
        stored.credentials_verified = true;
        fixture
            .store
            .change_password(&mut stored, "synthetic-password")
            .unwrap();
        let mut engine = Engine::new(stored.settings.core(&stored.key).unwrap()).unwrap();
        let mut status = Status {
            phase: "PAUSED".into(),
            ..Default::default()
        };
        let mut next = Trigger::Automatic;
        let (reply, receive) = tokio::sync::oneshot::channel();
        apply(
            Envelope {
                request: Request::Login,
                reply,
            },
            &mut stored,
            &mut engine,
            &fixture.store,
            &mut status,
            &mut next,
        );
        assert_eq!(receive.blocking_recv().unwrap()["ok"], true);
        assert!(!fixture.store.load().unwrap().settings.paused);
        assert!(next == Trigger::Login);
        status.phase = "WAIT_NETWORK".into();
        let (reply, receive) = tokio::sync::oneshot::channel();
        apply(
            Envelope {
                request: Request::Check,
                reply,
            },
            &mut stored,
            &mut engine,
            &fixture.store,
            &mut status,
            &mut next,
        );
        assert_eq!(receive.blocking_recv().unwrap()["ok"], true);
        assert!(next == Trigger::Check);
    }
    async fn request(name: &str, value: Value) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut pipe = loop {
            match ClientOptions::new().open(name) {
                Ok(pipe) => break pipe,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
                Err(error) => panic!("test pipe unavailable: {error}"),
            }
        };
        let bytes = serde_json::to_vec(&value).unwrap();
        ipc::write_frame(&mut pipe, &bytes, 16384)
            .await
            .unwrap_or_else(|e| panic!("test command={} failed: {e}", value["command"]));
        let response =
            tokio::time::timeout(Duration::from_secs(5), ipc::read_frame(&mut pipe, 65536))
                .await
                .unwrap()
                .unwrap();
        serde_json::from_slice(&response).unwrap()
    }
    #[tokio::test]
    async fn service_loop_handles_configuration_and_pause_through_real_ipc_without_authentication()
    {
        let fixture = super::super::storage::TestStore::new();
        let mut stored = Stored::new(super::super::security::current_sid().unwrap());
        stored.settings.user = "synthetic-user".into();
        stored.settings.paused = true;
        fixture
            .store
            .change_password(&mut stored, "synthetic-password")
            .unwrap();
        let name = format!(
            r"\\.\pipe\PortalLoginer.runtime-test.{}.{}",
            std::process::id(),
            rand::random::<u64>()
        );
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let store = Store {
            directory: fixture.store.directory.clone(),
        };
        let service = run_at(store, shutdown, Signal::new(), name.clone());
        let driver = async {
            assert_eq!(
                request(&name, json!({"command":"get_settings"})).await["has_password"],
                true
            );
            // A closed quiet subscription must release its connection permit.
            for _ in 0..12 {
                let mut pipe = ClientOptions::new().open(&name).unwrap();
                ipc::write_frame(&mut pipe, b"{\"command\":\"watch\"}", 16384)
                    .await
                    .unwrap();
                let frame = ipc::read_frame(&mut pipe, 65536).await.unwrap();
                let event: Value = serde_json::from_slice(&frame).unwrap();
                assert_eq!(event["ok"], true);
                assert!(event["status"]["instance_id"].is_u64());
                drop(pipe);
                tokio::task::yield_now().await;
            }
            let mut settings = super::super::settings::Settings {
                user: "synthetic-user".into(),
                paused: true,
                ..Default::default()
            };
            assert_eq!(
                request(
                    &name,
                    json!({"command":"save","settings":settings,"password":""})
                )
                .await["ok"],
                true
            );
            let reply = request(&name, json!({"command":"get_settings"})).await;
            assert_eq!(reply["has_password"], true);
            assert!(!reply.to_string().contains("synthetic-password"));
            settings.retry_seconds = 0;
            assert_eq!(
                request(
                    &name,
                    json!({"command":"save","settings":settings,"password":""})
                )
                .await["ok"],
                false
            );
            assert_eq!(
                request(&name, json!({"command":"get_settings"})).await["settings"]
                    ["retry_seconds"],
                10
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let status = request(&name, json!({"command":"status"})).await;
                if status["status"]["phase"] == "PAUSED" {
                    assert_eq!(status["status"]["authentication_id"], 0);
                    break;
                }
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            stop.cancel();
        };
        let (result, ()) = tokio::join!(service, driver);
        result.unwrap();
        assert!(fixture.store.load().unwrap().settings.paused);
    }
    #[tokio::test]
    async fn unverified_candidate_cannot_replace_saved_credentials_or_bypass_save() {
        let fixture = super::super::storage::TestStore::new();
        let mut stored = Stored::new(super::super::security::current_sid().unwrap());
        stored.settings.paused = true;
        fixture.store.save(&stored).unwrap();
        let name = format!(
            r"\\.\pipe\PortalLoginer.runtime-test.candidate.{}.{}",
            std::process::id(),
            rand::random::<u64>()
        );
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let service = run_at(
            Store {
                directory: fixture.store.directory.clone(),
            },
            shutdown,
            Signal::new(),
            name.clone(),
        );
        let driver = async {
            assert_eq!(
                request(&name, json!({"command":"get_settings"})).await["has_password"],
                false
            );
            let candidate = super::super::settings::Settings {
                user: "unverified-user".into(),
                wifi_only: Some("Synthetic-Unconnected-WiFi".into()),
                ..Default::default()
            };
            assert_eq!(
                request(
                    &name,
                    json!({"command":"save","settings":candidate,"password":"synthetic-password"})
                )
                .await["ok"],
                false
            );
            assert_eq!(request(&name,json!({"command":"configure","settings":candidate,"password":"synthetic-password"})).await["ok"],true);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let value = request(&name, json!({"command":"get_settings"})).await;
                if value["settings"]["user"] == "" {
                    assert_eq!(value["has_password"], false);
                    break;
                }
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            stop.cancel();
        };
        let (result, ()) = tokio::join!(service, driver);
        result.unwrap();
        let restored = fixture.store.load().unwrap();
        assert_eq!(restored.settings.user, "");
        assert!(restored.credential_file.is_none());
        assert_eq!(restored.settings.wifi_only, None);
    }
    #[test]
    fn logout_preserves_saved_credentials_and_pause_expires_next_boot() {
        let fixture = super::super::storage::TestStore::new();
        let mut stored = Stored::new(super::super::security::current_sid().unwrap());
        stored.settings.user = "synthetic-user".into();
        stored.credentials_verified = true;
        fixture
            .store
            .change_password(&mut stored, "synthetic-password")
            .unwrap();
        let reference = stored.credential_file.clone();
        pause_for_boot(&fixture.store, &mut stored, "current-boot").unwrap();
        let mut engine = Engine::new(stored.settings.core(&stored.key).unwrap()).unwrap();
        let mut status = Status::default();
        let mut next = Trigger::Automatic;
        let (reply, receive) = tokio::sync::oneshot::channel();
        apply(
            Envelope {
                request: Request::Logout,
                reply,
            },
            &mut stored,
            &mut engine,
            &fixture.store,
            &mut status,
            &mut next,
        );
        assert_eq!(receive.blocking_recv().unwrap()["ok"], true);
        assert!(next == Trigger::Logout);
        let reloaded = fixture.store.load().unwrap();
        assert_eq!(reloaded.settings.user, "synthetic-user");
        assert_eq!(reloaded.credential_file, reference);
        assert!(reloaded.credentials_verified);
        assert_eq!(
            &**fixture.store.password(&reloaded).unwrap().as_ref().unwrap(),
            b"synthetic-password"
        );
        assert!(paused_this_boot(&reloaded, "current-boot"));
        assert!(!paused_this_boot(&reloaded, "next-boot"));
        assert!(!reloaded.settings.paused);
    }
}
