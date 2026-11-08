#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
#[path = "../tray.rs"]
mod tray;
fn main() {
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() == Some("--wait-update") {
        if let Some(id) = std::env::args().nth(2).and_then(|s| s.parse::<u64>().ok()) {
            let _ = portal_desktop::platform::finish_update_restart(id);
        }
        return;
    }
    #[cfg(windows)]
    if std::env::args().any(|a| a == "--test-notification") {
        let result = portal_desktop::platform::Apartment::new().and_then(|_apartment| {
            portal_desktop::notifications::show_test()?;
            std::thread::sleep(std::time::Duration::from_secs(2));
            println!("{}",serde_json::json!({"result":portal_desktop::notifications::diagnostic(),"history_count":portal_desktop::notifications::history_count().ok()}));
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if let Err(error) = tray::run() {
        portal_desktop::platform::show_error(&error);
    }
}
