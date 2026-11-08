use super::*;
async fn single<T: Transport>(
    transport: &T,
    url: &str,
    ip: Ipv4Addr,
    index: usize,
    attempts: &mut Vec<ProbeAttempt>,
) -> Probe {
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
            result
        }
        Err(error) => {
            attempts.push(ProbeAttempt {
                index,
                status: None,
                error: Some(error),
                invalid_redirect: false,
            });
            Probe::Unknown
        }
    }
}
async fn logout<T: Transport>(
    transport: &T,
    config: &Config,
    ip: Ipv4Addr,
    cached: Option<&Portal>,
) -> bool {
    let preset = Portal::parse(
        &format!(
            "{}/index.html?bras={}&clientip={ip}",
            config.portal_host, config.bras
        ),
        ip,
    )
    .ok();
    let Some(portal) = cached.or(preset.as_ref()) else {
        return false;
    };
    good(
        transport
            .execute(api_post(portal, Endpoint::Logoff, None))
            .await,
    )
    .is_some_and(|r| protocol::ack_accepted(&r.body))
}
pub(super) async fn run<T: Transport>(
    transport: &T,
    credentials: &Credentials,
    config: &Config,
    context: WorkContext<'_>,
) -> Work {
    let WorkContext {
        ip,
        trigger,
        cached,
        scheduled,
        state,
        scheduled_attempt,
    } = context;
    let mut report = Work {
        outcome: Outcome::Unknown,
        cached,
        key: None,
        probes: Vec::new(),
    };
    if trigger == Trigger::Logout {
        state.send_replace(State::LoggingOut);
        report.outcome = if logout(transport, config, ip, report.cached.as_ref()).await {
            Outcome::LoggedOut
        } else {
            Outcome::Failed(Failure::Logout)
        };
        return report;
    }
    if matches!(trigger, Trigger::Relogin | Trigger::BootRelogin) || scheduled {
        if scheduled {
            scheduled_attempt.store(true, Ordering::SeqCst);
        }
        // Logout is best effort for Login: an already expired session or a
        // failed logout request must not prevent discovery and authentication.
        state.send_replace(State::LoggingOut);
        let _ = logout(transport, config, ip, report.cached.as_ref()).await;
        transport.reset_session();
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    state.send_replace(State::Checking);
    let primary = single(transport, &config.probe_url, ip, 0, &mut report.probes).await;
    let explicit_relogin = matches!(trigger, Trigger::Relogin | Trigger::BootRelogin);
    if primary == Probe::Online && !(explicit_relogin && report.cached.is_some()) {
        report.outcome = if trigger == Trigger::Login {
            Outcome::NeedsLogout
        } else {
            Outcome::Online
        };
        return report;
    }
    if trigger == Trigger::Check {
        report.outcome = if let Probe::Portal(portal) = primary {
            report.cached = Some(portal);
            Outcome::Captive
        } else {
            let mut found = Outcome::NetworkError;
            for (index, url) in BACKUP_PROBES.iter().enumerate() {
                match single(transport, url, ip, index + 1, &mut report.probes).await {
                    Probe::Portal(portal) => {
                        report.cached = Some(portal);
                        found = Outcome::Captive;
                        break;
                    }
                    Probe::Online => {
                        found = Outcome::Online;
                        break;
                    }
                    Probe::Unknown | Probe::NoAddress | Probe::NoRoute => {}
                }
            }
            found
        };
        return report;
    }
    let mut budget = 3u8;
    if let Probe::Portal(portal) = &primary {
        report.cached = Some(portal.clone());
    } else if report.cached.is_none() {
        report.outcome = Outcome::NetworkError;
    }
    if report.cached.is_some() {
        // Main probe failure intentionally allows one matching cached login.
        loop {
            budget -= 1;
            report = authenticate(transport, credentials, config, trigger, state, report, ip).await;
            if matches!(
                report.outcome,
                Outcome::Authenticated | Outcome::ManualRequired | Outcome::Pending
            ) {
                return report;
            }
            if primary == Probe::Unknown || budget == 0 {
                break;
            }
            tokio::time::sleep(config.retry_interval).await;
        }
    }
    if primary != Probe::Unknown {
        return report;
    }
    for (index, url) in BACKUP_PROBES.iter().enumerate() {
        let result = single(transport, url, ip, index + 1, &mut report.probes).await;
        if result == Probe::Online {
            report.outcome = Outcome::Online;
            return report;
        }
        if let Probe::Portal(portal) = result {
            report.cached = Some(portal);
            while budget > 0 {
                budget -= 1;
                report =
                    authenticate(transport, credentials, config, trigger, state, report, ip).await;
                if matches!(
                    report.outcome,
                    Outcome::Authenticated | Outcome::ManualRequired | Outcome::Pending
                ) {
                    return report;
                }
                if budget > 0 {
                    tokio::time::sleep(config.retry_interval).await;
                }
            }
            return report;
        }
    }
    report.outcome = Outcome::NetworkError;
    report
}
