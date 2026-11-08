use super::{
    ipc, network, runtime, security,
    storage::{self, Store},
    Error, Result, SERVICE_NAME,
};
use std::{
    ffi::OsString,
    io::Read,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use windows_service::{
    define_windows_service,
    service::*,
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};
use zeroize::Zeroizing;

pub fn cli() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 3 && args[0] == "verify-update-package" && args[1] == "--directory" {
        let dir = std::path::PathBuf::from(&args[2]);
        let raw = storage::read_limited(&dir.join("update-manifest.json"), 65536)?;
        let sig = storage::read_limited(&dir.join("update-manifest.sig"), 64)?;
        let update = portal_update::verify_manifest(
            &raw,
            &sig,
            portal_update::UPDATE_PUBLIC_KEY,
            "0.0.0",
            portal_update::Platform::Windows,
            portal_update::Architecture::X86_64,
        )
        .map_err(|_| Error::Invalid)?
        .ok_or(Error::Invalid)?;
        portal_update::download::verify_file(&update, &dir.join(&update.asset().name))
            .map_err(|_| Error::Invalid)?;
        println!(
            "{}",
            serde_json::json!({"verified":true,"version":update.version().to_string(),"bytes":update.asset().size})
        );
        return Ok(());
    }
    if args.len() == 5 && args[0] == "watch-update" && args[1] == "--job" && args[3] == "--pid" {
        return super::updates::watch(
            &std::path::PathBuf::from(&args[2]),
            args[4]
                .to_str()
                .and_then(|v| v.parse().ok())
                .ok_or(Error::Invalid)?,
        );
    }
    if args.len() == 3 && args[1] == "--job" {
        if args[0] == "apply-update" {
            return super::updates::apply(&std::path::PathBuf::from(&args[2]), false);
        }
        if args[0] == "recover-update" {
            return super::updates::apply(&std::path::PathBuf::from(&args[2]), true);
        }
    }
    if args.first().and_then(|v| v.to_str()) == Some("prepare-install")
        && args.len() == 3
        && args[1] == "--directory"
    {
        security::require_admin()?;
        let directory = std::path::PathBuf::from(&args[2]);
        if !directory.is_absolute() || directory.components().count() < 3 {
            return Err(Error::Invalid);
        }
        super::uninstall::validate_install_target(&directory)?;
        super::uninstall::ensure_files_idle(&directory, false)?;
        security::validate_install_ancestors(&directory)?;
        security::secure_install_directory(&directory)?;
        return stop_existing();
    }
    if args.first().and_then(|v| v.to_str()) == Some("uninstall-clean")
        && args.len() == 3
        && args[1] == "--directory"
    {
        return uninstall_clean(&std::path::PathBuf::from(&args[2]));
    }
    if args.first().and_then(|value| value.to_str()) == Some("diagnose-wifi") && args.len() == 2 {
        let ssid = args[1].to_str().ok_or(Error::Invalid)?;
        return runtime::runtime()?.block_on(async {
            use portal_core::{config::{DEFAULT_PROBE,BACKUP_PROBES},transport::{DirectTransport,Request,RequestKind,Transport}};
            let snapshot=network::snapshot(Some(ssid))?;
            if !snapshot.is_wifi || snapshot.ssid.as_deref()!=Some(ssid){return Err(Error::Invalid);}
            println!("{}",serde_json::json!({"event":"selected_wifi","network":snapshot,"authentication_requests":false}));
            let ip=snapshot.ipv4.ok_or(Error::Invalid)?;
            let transport=DirectTransport::new(ip,&snapshot.dns).map_err(|_|Error::Invalid)?;
            for (index,url) in std::iter::once(DEFAULT_PROBE).chain(BACKUP_PROBES).enumerate() {
                let started=Instant::now();
                let result=transport.execute(Request::get(url.to_owned(),RequestKind::Probe)).await;
                let value=match result {
                    Ok(response)=>serde_json::json!({"event":"probe","index":index,"elapsed_ms":started.elapsed().as_millis(),"http":response.status,"redirect":response.location.is_some(),"valid_portal":matches!(portal_core::protocol::classify_probe(response.status,response.location.as_deref(),ip),portal_core::protocol::Probe::Portal(_))}),
                    Err(error)=>serde_json::json!({"event":"probe","index":index,"elapsed_ms":started.elapsed().as_millis(),"error":error.to_string()}),
                };
                println!("{value}");
            }
            Ok(())
        });
    }
    if args.first().and_then(|value| value.to_str()) == Some("setup") {
        let owner = if args.len() == 3 && args[1] == "--owner-sid" {
            args[2].to_str().ok_or(Error::Invalid)?.to_owned()
        } else if args.len() == 1 {
            security::current_sid()?
        } else {
            return Err(Error::Invalid);
        };
        security::validate_sid(&owner)?;
        security::require_admin()?;
        match service(ServiceAccess::QUERY_STATUS) {
            Ok(existing) => {
                let _ = existing;
                let service = service(ServiceAccess::CHANGE_CONFIG)?;
                service.change_config(&application_service_info(std::env::current_exe()?))?;
            }
            Err(Error::Service(windows_service::Error::Winapi(error)))
                if error.raw_os_error() == Some(1060) =>
            {
                install_for_at(
                    owner,
                    Some(
                        std::env::current_exe()?
                            .parent()
                            .ok_or(Error::Invalid)?
                            .to_path_buf(),
                    ),
                )?
            }
            Err(error) => return Err(error),
        }
        let owner = Store::system()?.load()?.owner_sid;
        super::uninstall::remember_owner(&owner)?;
        let service =
            service(ServiceAccess::START | ServiceAccess::QUERY_STATUS | ServiceAccess::WRITE_DAC)?;
        security::set_service_permissions(service.raw_handle() as _, &owner)?;
        set_autostart(Store::system()?.load()?.settings.autostart)?;
        if service.query_status()?.current_state != ServiceState::Running {
            service.start::<&str>(&[])?;
        }
        return Ok(());
    }
    if args.len() != 1 {
        print_help();
        return Err(Error::Invalid);
    }
    match args[0].to_str().ok_or(Error::Invalid)? {
        "service" => {
            service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
            Ok(())
        }
        "install" => install(),
        "uninstall" => uninstall(),
        "uninstall-clean" => uninstall_clean(
            &super::uninstall::installed_directory()?.unwrap_or(security::install_directory()?),
        ),
        "prepare-install" => {
            security::require_admin()?;
            security::secure_install_directory(&security::install_directory()?)?;
            stop_existing()
        }
        "start" => {
            security::require_admin()?;
            service(ServiceAccess::START)?.start::<&str>(&[])?;
            Ok(())
        }
        "stop" => {
            security::require_admin()?;
            service(ServiceAccess::STOP)?.stop()?;
            Ok(())
        }
        "diagnose-install" => {
            let registered = super::uninstall::installed_directory()?;
            let directory = registered.clone().unwrap_or(security::install_directory()?);
            let idle = super::uninstall::ensure_files_idle(&directory, false);
            let file_error = match &idle {
                Err(Error::Os(e)) => e.raw_os_error(),
                _ => None,
            };
            let idle = match &idle {
                Ok(()) => Some(true),
                Err(Error::Os(e)) if matches!(e.raw_os_error(), Some(32 | 33)) => Some(false),
                _ => None,
            };
            println!(
                "{}",
                serde_json::json!({"directory":directory,"registered":registered.is_some(),"can_repair":super::uninstall::validate_install_target(&directory).is_ok(),"ui_files_idle":idle,"file_check_error":file_error,"protected_ancestors":security::validate_install_ancestors(&directory).is_ok(),"remaining_program_files":(["portal-service.exe","portal-settings.exe","portal-tray.exe"].into_iter().filter(|name|directory.join(name).exists()).collect::<Vec<_>>()) })
            );
            Ok(())
        }
        "diagnose" => {
            let snapshot = network::snapshot(None)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&snapshot).map_err(|_| Error::Invalid)?
            );
            Ok(())
        }
        "diagnose-probes" => runtime::runtime()?.block_on(async {
            use portal_core::{
                config::{BACKUP_PROBES, DEFAULT_PROBE},
                transport::{DirectTransport, Request, RequestKind, Transport},
            };
            let snapshot = network::snapshot(None)?;
            let ip = snapshot.ipv4.ok_or(Error::Invalid)?;
            let transport = DirectTransport::new(ip, &snapshot.dns).map_err(|_| Error::Invalid)?;
            for url in std::iter::once(DEFAULT_PROBE).chain(BACKUP_PROBES) {
                let result = transport
                    .execute(Request::get(url.to_owned(), RequestKind::Probe))
                    .await;
                match result {
                    Ok(response) => println!(
                        "{} HTTP={} redirect={} valid_portal={}",
                        url,
                        response.status,
                        response.location.is_some(),
                        matches!(
                            portal_core::protocol::classify_probe(
                                response.status,
                                response.location.as_deref(),
                                ip
                            ),
                            portal_core::protocol::Probe::Portal(_)
                        )
                    ),
                    Err(error) => println!("{} error={error}", url),
                }
            }
            Ok(())
        }),
        "configure" => {
            let mut bytes = Zeroizing::new(Vec::new());
            std::io::stdin().take(16385).read_to_end(&mut bytes)?;
            if bytes.len() > 16384 {
                return Err(Error::Invalid);
            }
            let request: ipc::Request =
                serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
            if !matches!(request, ipc::Request::Save { .. }) {
                return Err(Error::Invalid);
            }
            send(&bytes)
        }
        "status" => send(b"{\"command\":\"status\"}"),
        "settings" => send(b"{\"command\":\"get_settings\"}"),
        "check" => send(b"{\"command\":\"check\"}"),
        "login" => send(b"{\"command\":\"login\"}"),
        "relogin" => send(b"{\"command\":\"relogin\"}"),
        "pause" => send(b"{\"command\":\"pause\"}"),
        "resume" => send(b"{\"command\":\"resume\"}"),
        "logs" => send(b"{\"command\":\"logs\"}"),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        _ => Err(Error::Invalid),
    }
}
fn send(bytes: &[u8]) -> Result<()> {
    let response = runtime::runtime()?.block_on(ipc::client(bytes))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(|_| Error::Invalid)?
    );
    if response["ok"] != true {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn print_help() {
    println!("Portal Loginer Windows 开发版\ninstall / uninstall / start / stop\nstatus / settings / configure（从标准输入读取 JSON，不接受密码参数）\ncheck / relogin / pause / resume / logs\ndiagnose（只读取网络信息，不发送认证请求）");
}
fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
    Ok(ServiceManager::local_computer(None::<&str>, access)?)
}
fn service(access: ServiceAccess) -> Result<Service> {
    Ok(manager(ServiceManagerAccess::CONNECT)?.open_service(SERVICE_NAME, access)?)
}

pub fn verify_server_pid(pid: u32) -> Result<()> {
    let status = service(ServiceAccess::QUERY_STATUS)?.query_status()?;
    if status.current_state == ServiceState::Running && status.process_id == Some(pid) {
        Ok(())
    } else {
        Err(Error::Access)
    }
}
pub(super) fn service_ready() -> Result<()> {
    if service(ServiceAccess::QUERY_STATUS)?
        .query_status()?
        .current_state
        == ServiceState::Running
    {
        Ok(())
    } else {
        Err(Error::Invalid)
    }
}
pub(super) fn service_stopped() -> Result<bool> {
    match service(ServiceAccess::QUERY_STATUS) {
        Ok(service) => Ok(service.query_status()?.current_state == ServiceState::Stopped),
        Err(Error::Service(windows_service::Error::Winapi(error)))
            if error.raw_os_error() == Some(1060) =>
        {
            Ok(true)
        }
        Err(error) => Err(error),
    }
}
pub(super) fn set_autostart(enabled: bool) -> Result<()> {
    let mut info = application_service_info(std::env::current_exe()?);
    info.start_type = if enabled {
        ServiceStartType::AutoStart
    } else {
        ServiceStartType::OnDemand
    };
    service(ServiceAccess::CHANGE_CONFIG)?.change_config(&info)?;
    Ok(())
}
pub(super) fn start_application() -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let state = service(ServiceAccess::QUERY_STATUS)?
            .query_status()?
            .current_state;
        match state {
            ServiceState::Running => return Ok(()),
            ServiceState::Stopped => {
                let result = service(ServiceAccess::START)?.start::<&str>(&[]);
                if let Err(windows_service::Error::Winapi(error)) = &result {
                    if error.raw_os_error() != Some(1056) {
                        result?;
                    }
                } else {
                    result?;
                }
            }
            ServiceState::StartPending | ServiceState::StopPending => {}
            _ => return Err(Error::Invalid),
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn install() -> Result<()> {
    install_for(security::current_sid()?)
}
fn install_for(owner: String) -> Result<()> {
    install_for_at(owner, None)
}
fn application_service_info(target: std::path::PathBuf) -> ServiceInfo {
    ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: "Portal Loginer 校园网认证".into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: target,
        launch_arguments: vec!["service".into()],
        dependencies: Vec::new(),
        account_name: None,
        account_password: None,
    }
}
fn install_for_at(owner: String, directory: Option<std::path::PathBuf>) -> Result<()> {
    security::require_admin()?;
    let manager = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
    match manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(_) => return Err(Error::Invalid),
        Err(windows_service::Error::Winapi(error)) if error.raw_os_error() == Some(1060) => {}
        Err(error) => return Err(error.into()),
    }
    let directory = directory.unwrap_or(security::install_directory()?);
    security::secure_install_directory(&directory)?;
    let target = directory.join("portal-service.exe");
    if std::env::current_exe()?.canonicalize()? != target.canonicalize().unwrap_or(target.clone()) {
        // Legacy development CLI only. The normal installer supplies all files.
        let source = std::env::current_exe()?
            .parent()
            .ok_or(Error::Invalid)?
            .to_path_buf();
        for name in [
            "portal-service.exe",
            "portal-settings.exe",
            "portal-tray.exe",
        ] {
            let bytes = std::fs::read(source.join(name))?;
            storage::atomic_write(&directory, name, &bytes)?;
        }
    }
    let store = Store::system()?;
    store.initialize(owner)?;
    let owner = store.load()?.owner_sid;
    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: "Portal Loginer 校园网认证".into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: target,
        launch_arguments: vec!["service".into()],
        dependencies: Vec::new(),
        account_name: None,
        account_password: None,
    };
    let service = manager.create_service(
        &info,
        ServiceAccess::CHANGE_CONFIG
            | ServiceAccess::QUERY_STATUS
            | ServiceAccess::START
            | ServiceAccess::WRITE_DAC
            | ServiceAccess::DELETE,
    )?;
    let setup = (|| -> Result<()> {
        security::set_service_permissions(service.raw_handle() as _, &owner)?;
        service.set_description("校园 Portal 自动认证；用户登录前等待物理网络就绪。")?;
        service.update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86400)),
            reboot_msg: None,
            command: None,
            actions: Some(
                [30, 60, 300]
                    .into_iter()
                    .map(|seconds| ServiceAction {
                        action_type: ServiceActionType::Restart,
                        delay: Duration::from_secs(seconds),
                    })
                    .collect(),
            ),
        })?;
        service.set_failure_actions_on_non_crash_failures(true)?;
        service.set_delayed_auto_start(false)?;
        Ok(())
    })();
    if let Err(error) = setup {
        let _ = service.delete();
        return Err(error);
    }
    println!("后台服务已安装，尚未启动；配置与凭据目录已受保护。");
    Ok(())
}
fn missing_service(error: &windows_service::Error) -> bool {
    matches!(error,windows_service::Error::Winapi(e) if matches!(e.raw_os_error(),Some(1060|1072)))
}
pub(super) fn stop_existing() -> Result<()> {
    security::require_admin()?;
    let service = match service(ServiceAccess::QUERY_STATUS | ServiceAccess::STOP) {
        Ok(s) => s,
        Err(Error::Service(e)) if missing_service(&e) => return Ok(()),
        Err(e) => return Err(e),
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let state = match service.query_status() {
            Ok(s) => s.current_state,
            Err(e) if missing_service(&e) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        if state == ServiceState::Stopped {
            return Ok(());
        }
        if state != ServiceState::StopPending {
            if let Err(e) = service.stop() {
                if !matches!(&e,windows_service::Error::Winapi(e) if matches!(e.raw_os_error(),Some(1061|1062)))
                {
                    return Err(e.into());
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
fn uninstall() -> Result<()> {
    stop_existing()?;
    let service = match service(ServiceAccess::DELETE) {
        Ok(s) => s,
        Err(Error::Service(e)) if missing_service(&e) => return Ok(()),
        Err(e) => return Err(e),
    };
    match service.delete() {
        Ok(()) => Ok(()),
        Err(e) if missing_service(&e) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn uninstall_clean(directory: &std::path::Path) -> Result<()> {
    security::require_admin()?;
    super::uninstall::validate_cleanup_target(directory)?;
    // Preflight the UI before stopping the service or deleting any data.
    super::uninstall::ensure_files_idle(directory, false)?;
    stop_existing()?;
    super::uninstall::ensure_files_idle(directory, true)?;
    let owner = Store::system()
        .ok()
        .and_then(|s| s.load().ok())
        .map(|s| s.owner_sid)
        .or(super::uninstall::remembered_owner()?)
        .unwrap_or(security::current_sid()?);
    super::uninstall::remove_owner_settings(&owner)?;
    uninstall()?;
    purge_data()
}
fn purge_data() -> Result<()> {
    security::require_admin()?;
    let directory = security::data_directory()?;
    if !directory.exists() {
        return Ok(());
    }
    security::secure_directory(&directory)?;
    let entries: Vec<_> = std::fs::read_dir(&directory)?.collect::<std::io::Result<Vec<_>>>()?;
    use std::os::windows::fs::MetadataExt;
    for entry in &entries {
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.file_attributes() & 0x400 != 0
            || !(metadata.is_file() || metadata.is_dir() && entry.file_name() == "updates")
        {
            return Err(Error::Access);
        }
    }
    for entry in entries {
        if entry.file_type()?.is_dir() {
            super::updates::remove_tree(&entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    std::fs::remove_dir(directory)?;
    Ok(())
}

define_windows_service!(ffi_service_main, service_main);
fn service_main(_: Vec<OsString>) {
    let _ = run_service();
}
fn status(state: ServiceState, exit: ServiceExitCode) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP
                | ServiceControlAccept::SHUTDOWN
                | ServiceControlAccept::POWER_EVENT
                | ServiceControlAccept::TIME_CHANGE
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: exit,
        checkpoint: if matches!(
            state,
            ServiceState::StartPending | ServiceState::StopPending
        ) {
            1
        } else {
            0
        },
        wait_hint: Duration::from_secs(20),
        process_id: None,
    }
}
fn run_service() -> Result<()> {
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let signal = network::Signal::new();
    let events = signal.clone();
    let handle = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stop.cancel();
            events.bump();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::PowerEvent(event) => {
            if matches!(
                event,
                PowerEventParam::Suspend
                    | PowerEventParam::ResumeAutomatic
                    | PowerEventParam::ResumeSuspend
                    | PowerEventParam::ResumeCritical
                    | PowerEventParam::QuerySuspendFailed
            ) {
                events.bump();
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::TimeChange => {
            events.schedule.notify_one();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(
        ServiceState::StartPending,
        ServiceExitCode::Win32(0),
    ))?;
    let result = (|| -> Result<()> {
        let store = Store::system()?;
        store.load()?;
        if super::updates::recover_interrupted()? {
            return Ok(());
        }
        let runtime = runtime::runtime()?;
        handle.set_service_status(status(ServiceState::Running, ServiceExitCode::Win32(0)))?;
        runtime.block_on(runtime::run(store, shutdown, signal))
    })();
    let exit = if result.is_ok() {
        ServiceExitCode::Win32(0)
    } else {
        ServiceExitCode::ServiceSpecific(1)
    };
    handle.set_service_status(status(ServiceState::Stopped, exit))?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn client_rejects_a_pipe_server_outside_scm_process() {
        assert!(verify_server_pid(std::process::id()).is_err());
    }
}
