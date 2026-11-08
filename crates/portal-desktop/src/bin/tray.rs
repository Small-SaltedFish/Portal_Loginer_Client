#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
#[path = "../tray.rs"]
mod tray;
fn main() {
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
