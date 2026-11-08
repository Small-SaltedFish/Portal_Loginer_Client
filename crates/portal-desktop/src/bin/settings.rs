#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
#[path = "../settings.rs"]
mod settings;
fn main() {
    #[cfg(windows)]
    if let Err(error) = settings::run() {
        portal_desktop::platform::show_error(&error);
    }
}
