use super::{security, storage, Error, Result};
use std::os::windows::process::CommandExt;
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn definition(owner: &str, directory: &Path) -> Result<String> {
    security::validate_sid(owner)?;
    if !directory.is_absolute() {
        return Err(Error::Invalid);
    }
    let exe = xml(&directory.join("portal-tray.exe").to_string_lossy());
    let directory = xml(&directory.to_string_lossy());
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
<RegistrationInfo><Description>Portal Loginer 托盘：用户登录时立即启动。</Description></RegistrationInfo>
<Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{owner}</UserId><Delay>PT0S</Delay></LogonTrigger></Triggers>
<Principals><Principal id="Owner"><UserId>{owner}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><StartWhenAvailable>true</StartWhenAvailable><RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable><Enabled>true</Enabled><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Priority>5</Priority></Settings>
<Actions Context="Owner"><Exec><Command>{exe}</Command><WorkingDirectory>{directory}</WorkingDirectory></Exec></Actions>
</Task>"#
    ))
}
fn encoded_definition(value: &str) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(value.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}
fn run(arguments: &[&std::ffi::OsStr]) -> Result<bool> {
    let mut buffer = [0u16; 32768];
    let length = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(Error::Invalid);
    }
    let exe =
        std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..length])).join("schtasks.exe");
    let mut child = Command::new(exe)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.success());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Timeout);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
static TASK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub fn refresh_saved() -> Result<()> {
    let _lock = TASK_LOCK.lock().map_err(|_| Error::Invalid)?;
    let stored = storage::Store::system()?.load()?;
    if let Some(directory) = super::uninstall::installed_directory()? {
        configure_inner(&stored.owner_sid, &directory, stored.settings.autostart)?;
    }
    Ok(())
}
pub fn configure(owner: &str, directory: &Path, enabled: bool) -> Result<()> {
    let _lock = TASK_LOCK.lock().map_err(|_| Error::Invalid)?;
    configure_inner(owner, directory, enabled)
}
fn configure_inner(owner: &str, directory: &Path, enabled: bool) -> Result<()> {
    security::validate_sid(owner)?;
    let name = format!("PortalLoginer.Tray.{owner}");
    if !enabled {
        // A missing task is already the desired state; keep uninstall repeatable.
        let _ = run(&[
            "/Delete".as_ref(),
            "/TN".as_ref(),
            name.as_ref(),
            "/F".as_ref(),
        ])?;
        return Ok(());
    }
    super::uninstall::validate_install_target(directory)?;
    let store = storage::Store::system()?;
    storage::atomic_write(
        &store.directory,
        "startup-task.xml",
        &encoded_definition(&definition(owner, directory)?),
    )?;
    let file = store.directory.join("startup-task.xml");
    let result = run(&[
        "/Create".as_ref(),
        "/TN".as_ref(),
        name.as_ref(),
        "/XML".as_ref(),
        file.as_os_str(),
        "/F".as_ref(),
    ]);
    let _ = std::fs::remove_file(file);
    if result? {
        Ok(())
    } else {
        Err(Error::Access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_is_interactive_unprivileged_battery_independent_and_escapes_paths() {
        let value = definition("S-1-5-21-1-2-3-1001", Path::new(r"C:\Apps\Campus & WiFi")).unwrap();
        std::fs::write(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/startup-task-validation.xml"),
            encoded_definition(&value),
        )
        .unwrap();
        assert!(value.contains("Campus &amp; WiFi"));
        assert!(value.contains("<Delay>PT0S</Delay>"));
        assert!(value.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(value.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(value.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(definition("bad<sid>", Path::new(r"C:\Apps")).is_err());
    }
}
