use super::{security, Error, Result};
use std::{
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::System::Registry::*;
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn open(root: HKEY, path: &str, access: u32) -> Result<Option<Key>> {
    let mut raw = ptr::null_mut();
    let code = unsafe { RegOpenKeyExW(root, security::wide(path).as_ptr(), 0, access, &mut raw) };
    match code {
        0 => Ok(Some(Key(raw))),
        2 => Ok(None),
        _ => Err(std::io::Error::from_raw_os_error(code as i32).into()),
    }
}
fn string(root: HKEY, path: &str, name: &str) -> Result<Option<String>> {
    let mut buf = vec![0u16; 32768];
    let mut bytes = (buf.len() * 2) as u32;
    let code = unsafe {
        RegGetValueW(
            root,
            security::wide(path).as_ptr(),
            security::wide(name).as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if code == 2 {
        return Ok(None);
    }
    if code != 0 {
        return Err(std::io::Error::from_raw_os_error(code as i32).into());
    }
    Ok(Some(String::from_utf16_lossy(
        &buf[..(bytes as usize / 2).saturating_sub(1)],
    )))
}
pub fn installed_directory() -> Result<Option<PathBuf>> {
    Ok(string(
        HKEY_LOCAL_MACHINE,
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\PortalLoginer",
        "InstallLocation",
    )?
    .map(PathBuf::from))
}
fn check(code: u32) -> Result<()> {
    if code == 0 || code == 2 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(code as i32).into())
    }
}
pub fn remove_owner_settings(sid: &str) -> Result<()> {
    security::validate_sid(sid)?;
    let profile = string(
        HKEY_LOCAL_MACHINE,
        &format!(r"Software\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}"),
        "ProfileImagePath",
    )?
    .map(PathBuf::from);
    let key = match open(HKEY_USERS, sid, KEY_ALL_ACCESS)? {
        Some(key) => Some(key),
        None => {
            if let Some(profile) = &profile {
                let hive = profile.join("NTUSER.DAT");
                if hive.exists() {
                    let mut raw = ptr::null_mut();
                    check(unsafe {
                        RegLoadAppKeyW(
                            security::wide(hive).as_ptr(),
                            &mut raw,
                            KEY_ALL_ACCESS,
                            0,
                            0,
                        )
                    })?;
                    Some(Key(raw))
                } else {
                    None
                }
            } else {
                None
            }
        }
    };
    if let Some(key) = key {
        if let Some(run) = open(
            key.0,
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_SET_VALUE,
        )? {
            check(unsafe { RegDeleteValueW(run.0, security::wide("PortalLoginer").as_ptr()) })?;
        }
        check(unsafe {
            RegDeleteTreeW(
                key.0,
                security::wide(r"Software\SmallSaltedFish\PortalLoginer").as_ptr(),
            )
        })?;
    }
    if let Some(profile) = profile {
        remove_notification_shortcut(&profile)?;
    }
    Ok(())
}
fn remove_notification_shortcut(profile: &Path) -> Result<()> {
    if !profile.is_absolute() || profile.components().count() < 3 {
        return Err(Error::Access);
    }
    let relative =
        Path::new(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Portal Loginer.lnk");
    let mut current = profile.to_path_buf();
    use std::os::windows::fs::MetadataExt;
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(m) => {
                if m.file_attributes() & 0x400 != 0 {
                    return Err(Error::Access);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
    std::fs::remove_file(current)?;
    Ok(())
}

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\PortalLoginer";
pub fn remembered_owner() -> Result<Option<String>> {
    string(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, "OwnerSid")
}
pub fn remember_owner(sid: &str) -> Result<()> {
    security::validate_sid(sid)?;
    let mut raw = ptr::null_mut();
    check(unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            security::wide(UNINSTALL_KEY).as_ptr(),
            0,
            ptr::null(),
            0,
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            ptr::null(),
            &mut raw,
            ptr::null_mut(),
        )
    })?;
    let key = Key(raw);
    let value = security::wide(sid);
    check(unsafe {
        RegSetValueExW(
            key.0,
            security::wide("OwnerSid").as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    })
}
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches('\\'))
}
fn valid_target(path: &Path, registered: Option<&Path>, cleanup: bool) -> Result<()> {
    use std::path::Component;
    if !path.is_absolute()
        || path.components().count() < 3
        || path
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
    {
        return Err(Error::Invalid);
    }
    if let Some(registered) = registered {
        if !same_path(path, registered) {
            return Err(Error::Invalid);
        }
        return Ok(());
    }
    if cleanup && same_path(path, &security::install_directory()?) {
        return Ok(());
    }
    if path.exists()
        && std::fs::read_dir(path)?.next().is_some()
        && ![
            "portal-service.exe",
            "portal-settings.exe",
            "portal-tray.exe",
        ]
        .iter()
        .any(|name| path.join(name).is_file())
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub fn validate_install_target(path: &Path) -> Result<()> {
    valid_target(path, installed_directory()?.as_deref(), false)
}
pub fn validate_cleanup_target(path: &Path) -> Result<()> {
    valid_target(path, installed_directory()?.as_deref(), true)?;
    security::validate_install_ancestors(path)?;
    if path.exists() {
        security::secure_install_directory(path)?;
    }
    Ok(())
}
pub fn ensure_files_idle(directory: &Path, include_service: bool) -> Result<()> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    for name in ["portal-settings.exe", "portal-tray.exe"]
        .into_iter()
        .chain(include_service.then_some("portal-service.exe"))
    {
        let path = directory.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
            Ok(m) if !m.is_file() || m.file_attributes() & 0x400 != 0 => return Err(Error::Access),
            Ok(_) => {}
        }
        let _guard = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(path)?;
    }
    Ok(())
}
pub fn set_version(version: &str) -> Result<()> {
    let Some(key) = open(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, KEY_SET_VALUE)? else {
        return Err(Error::Invalid);
    };
    let value = security::wide(version);
    check(unsafe {
        RegSetValueExW(
            key.0,
            security::wide("DisplayVersion").as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    })
}

pub fn owner_proxy(sid: &str) -> Result<Option<String>> {
    security::validate_sid(sid)?;
    let environment = format!(r"{sid}\Environment");
    if let Some(value) = string(HKEY_USERS, &environment, "HTTPS_PROXY")?.or(string(
        HKEY_USERS,
        &environment,
        "ALL_PROXY",
    )?) {
        if !value.is_empty() {
            return Ok(Some(value));
        }
    }
    let path = format!(r"{sid}\Software\Microsoft\Windows\CurrentVersion\Internet Settings");
    let mut enabled = 0u32;
    let mut size = 4;
    let code = unsafe {
        RegGetValueW(
            HKEY_USERS,
            security::wide(&path).as_ptr(),
            security::wide("ProxyEnable").as_ptr(),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            (&mut enabled as *mut u32).cast(),
            &mut size,
        )
    };
    if code != 0 || enabled == 0 {
        return Ok(None);
    }
    let Some(value) = string(HKEY_USERS, &path, "ProxyServer")? else {
        return Ok(None);
    };
    let selected = if value.contains('=') {
        value
            .split(';')
            .filter_map(|v| v.split_once('='))
            .find(|(k, _)| *k == "https")
            .map(|(_, v)| v.to_owned())
    } else {
        Some(value)
    };
    Ok(selected.filter(|s| !s.is_empty()).map(|s| {
        if s.contains("://") {
            s
        } else {
            format!("http://{s}")
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registered_partial_directory_can_be_repaired_without_service_binary() {
        let fixture = super::super::storage::TestStore::new();
        let dir = &fixture.store.directory;
        std::fs::write(dir.join("NOTICE"), b"remaining-file").unwrap();
        assert!(valid_target(dir, Some(dir), false).is_ok());
        assert!(valid_target(dir, None, false).is_err());
        assert!(valid_target(&dir.join("other"), Some(dir), false).is_err());
    }
    #[test]
    fn locked_application_blocks_preflight_and_missing_files_allow_retries() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = super::super::storage::TestStore::new();
        let dir = &fixture.store.directory;
        let file = dir.join("portal-tray.exe");
        std::fs::write(&file, b"synthetic-image").unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&file)
            .unwrap();
        assert!(ensure_files_idle(dir, false).is_err());
        assert!(file.exists());
        drop(lock);
        assert!(ensure_files_idle(dir, true).is_ok());
        std::fs::remove_file(file).unwrap();
        assert!(ensure_files_idle(dir, true).is_ok());
        assert!(ensure_files_idle(dir, true).is_ok());
    }
}
