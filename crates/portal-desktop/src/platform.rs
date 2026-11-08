use std::{
    ffi::OsStr,
    mem::{size_of, zeroed},
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Com::CoTaskMemFree, Registry::*, Threading::*},
    UI::{Shell::*, WindowsAndMessaging::*},
};
pub fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
pub struct Instance(HANDLE);
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
pub fn single_instance(name: &str, title: Option<&str>) -> Result<Option<Instance>, String> {
    let handle = unsafe { CreateMutexW(ptr::null(), 0, wide(name).as_ptr()) };
    if handle.is_null() {
        return Err("无法创建程序实例。".into());
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(handle);
        }
        if let Some(title) = title {
            let window = unsafe { FindWindowW(ptr::null(), wide(title).as_ptr()) };
            if !window.is_null() {
                unsafe {
                    ShowWindow(window, SW_RESTORE);
                    SetForegroundWindow(window);
                }
            }
        }
        return Ok(None);
    }
    Ok(Some(Instance(handle)))
}
pub fn show_error(message: &str) {
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            wide(message).as_ptr(),
            wide("Portal Loginer").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
pub fn close_settings(preview: bool) {
    let title = if preview {
        "Portal Loginer 设置（预览）"
    } else {
        "Portal Loginer 设置"
    };
    let window = unsafe { FindWindowW(ptr::null(), wide(title).as_ptr()) };
    if !window.is_null() {
        unsafe {
            PostMessageW(window, WM_CLOSE, 0, 0);
        }
    }
}
pub fn sibling(name: &str) -> Result<PathBuf, String> {
    let installed = reg_string_machine(
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\PortalLoginer",
        "InstallLocation",
    )
    .map(PathBuf::from)
    .unwrap_or(known_folder(&FOLDERID_ProgramFiles)?.join("PortalLoginer"));
    if installed.join("Uninstall.exe").is_file() && installed.join(name).is_file() {
        return Ok(installed.join(name));
    }
    Ok(std::env::current_exe()
        .map_err(|_| "找不到程序目录。")?
        .with_file_name(name))
}
pub fn open_settings(logs: bool, preview: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new(sibling("portal-settings.exe")?);
    if logs {
        command.arg("--logs");
    }
    if preview {
        command.arg("--preview");
    }
    command
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| "无法打开设置，请确认程序文件完整。".to_owned())?;
    Ok(())
}
pub fn wait_for_tray_host() {
    // A logon task can run before Explorer has created its notification area.
    // Wait only for that host, never for Internet access or the auth service.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while unsafe { FindWindowW(wide("Shell_TrayWnd").as_ptr(), ptr::null()) }.is_null() {
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
pub fn start_tray() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(sibling("portal-tray.exe")?)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| "无法启动托盘程序。".to_owned())?;
    Ok(())
}
pub fn setup_service() -> Result<(), String> {
    let installer = std::env::current_exe()
        .map_err(|_| "找不到程序目录。")?
        .with_file_name(format!(
            "PortalLoginer-{}-Setup.exe",
            env!("CARGO_PKG_VERSION")
        ));
    if !installer.is_file() {
        return Err("请运行完整安装包，或将安装包放在程序同一目录后重试。".into());
    }
    let exe = wide(installer);
    let verb = wide("runas");
    let sid = portal_service::windows::current_user_sid()
        .map_err(|_| "无法读取当前 Windows 用户。".to_owned())?;
    let args = wide(format!("/OWNER_SID={sid}"));
    let mut info = unsafe { zeroed::<SHELLEXECUTEINFOW>() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = exe.as_ptr();
    info.lpParameters = args.as_ptr();
    info.nShow = SW_HIDE;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err("安装未完成；如取消了系统授权，可稍后重试。".into());
    }
    if info.hProcess.is_null() {
        return Err("无法确认后台服务安装结果。".into());
    }
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
    }
    let mut code = 1;
    unsafe {
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
    }
    if code == 0 {
        Ok(())
    } else {
        Err("客户端安装未完成，请退出旧版面板和托盘后重试。".into())
    }
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn reg_key(path: &str, write: bool) -> Result<Key, String> {
    reg_key_at(path, write, HKEY_CURRENT_USER)
}
fn reg_key_at(path: &str, write: bool, root: HKEY) -> Result<Key, String> {
    let mut key = ptr::null_mut();
    let result = if write {
        unsafe {
            RegCreateKeyExW(
                root,
                wide(path).as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_READ | KEY_WRITE,
                ptr::null(),
                &mut key,
                ptr::null_mut(),
            )
        }
    } else {
        unsafe { RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_READ, &mut key) }
    };
    if result != 0 {
        return Err("无法访问当前用户的启动设置。".into());
    }
    Ok(Key(key))
}
pub fn reg_string(path: &str, name: &str) -> Option<String> {
    reg_string_at(path, name, HKEY_CURRENT_USER)
}
fn reg_string_machine(path: &str, name: &str) -> Option<String> {
    reg_string_at(path, name, HKEY_LOCAL_MACHINE)
}
fn reg_string_at(path: &str, name: &str, root: HKEY) -> Option<String> {
    let key = reg_key_at(path, false, root).ok()?;
    let mut size = 0;
    let mut kind = 0;
    if unsafe {
        RegQueryValueExW(
            key.0,
            wide(name).as_ptr(),
            ptr::null(),
            &mut kind,
            ptr::null_mut(),
            &mut size,
        )
    } != 0
        || kind != REG_SZ
        || size > 32768
    {
        return None;
    }
    let mut bytes = vec![0u16; (size as usize).div_ceil(2)];
    if unsafe {
        RegQueryValueExW(
            key.0,
            wide(name).as_ptr(),
            ptr::null(),
            &mut kind,
            bytes.as_mut_ptr().cast(),
            &mut size,
        )
    } != 0
    {
        return None;
    }
    let end = bytes.iter().position(|v| *v == 0).unwrap_or(bytes.len());
    String::from_utf16(&bytes[..end]).ok()
}
pub fn set_reg_string(path: &str, name: &str, value: &str) -> Result<(), String> {
    let key = reg_key(path, true)?;
    let bytes = wide(value);
    if unsafe {
        RegSetValueExW(
            key.0,
            wide(name).as_ptr(),
            0,
            REG_SZ,
            bytes.as_ptr().cast(),
            (bytes.len() * 2) as u32,
        )
    } != 0
    {
        return Err("无法保存当前用户设置。".into());
    }
    Ok(())
}
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub fn tray_autostart() -> bool {
    reg_string(RUN_KEY, "PortalLoginer").is_some()
}
pub fn tray_autostart_choice() -> bool {
    if reg_string(r"Software\SmallSaltedFish\PortalLoginer", "TrayPreference").is_none() {
        true
    } else {
        tray_autostart()
    }
}
pub fn set_tray_autostart(enabled: bool) -> Result<(), String> {
    set_reg_string(
        r"Software\SmallSaltedFish\PortalLoginer",
        "TrayPreference",
        "set",
    )?;
    // The service owns the zero-delay task and the Run fallback. Do not recreate
    // a delayed Run entry after a successful task registration.
    if !enabled {
        remove_legacy_startup()?;
    }
    Ok(())
}
pub fn remove_legacy_startup() -> Result<(), String> {
    let key = reg_key(RUN_KEY, true)?;
    let result = unsafe { RegDeleteValueW(key.0, wide("PortalLoginer").as_ptr()) };
    if result != 0 && result != ERROR_FILE_NOT_FOUND {
        return Err("无法修改登录启动项。".into());
    }
    Ok(())
}

fn known_folder(id: &windows_sys::core::GUID) -> Result<PathBuf, String> {
    let mut value = ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(id, 0, ptr::null_mut(), &mut value) } < 0 {
        return Err("无法读取系统目录。".into());
    }
    let mut length = 0;
    unsafe {
        while *value.add(length) != 0 {
            length += 1;
        }
    }
    let path = PathBuf::from(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(value, length)
    }));
    unsafe {
        CoTaskMemFree(value.cast());
    }
    Ok(path)
}
pub fn programs_folder() -> Result<PathBuf, String> {
    known_folder(&FOLDERID_Programs)
}
pub fn fonts_folder() -> Result<PathBuf, String> {
    known_folder(&FOLDERID_Fonts)
}
pub struct Apartment;
impl Apartment {
    pub fn new() -> Result<Self, String> {
        unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_SINGLETHREADED,
            )
        }
        .map_err(|_| "无法初始化 Windows 界面。".to_owned())?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::WinRT::RoUninitialize();
        }
    }
}

pub fn fullscreen_busy() -> bool {
    let mut state = 0;
    (unsafe { windows_sys::Win32::UI::Shell::SHQueryUserNotificationState(&mut state) }) >= 0
        && matches!(state, 2..=4)
}
pub fn maintenance_busy() -> bool {
    use windows_sys::Win32::System::Threading::*;
    let handle = unsafe {
        OpenMutexW(
            0x00100001,
            0,
            wide(r"Global\PortalLoginer.Maintenance.v1").as_ptr(),
        )
    };
    if handle.is_null() {
        return std::io::Error::last_os_error().raw_os_error() == Some(5);
    }
    let result = unsafe { WaitForSingleObject(handle, 0) };
    if matches!(result, 0 | 0x80) {
        unsafe {
            ReleaseMutex(handle);
        }
    }
    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(handle);
    }
    !matches!(result, 0 | 0x80)
}
pub fn wait_and_restart_tray(instance: u64) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let source = std::env::current_exe().map_err(|_| "无法读取托盘路径")?;
    let destination = std::env::temp_dir().join(format!(
        "PortalLoginer-restart-{}-{}.exe",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut input = std::fs::File::open(source).map_err(|_| "无法准备更新后恢复")?;
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&destination)
        .map_err(|_| "无法准备更新后恢复")?;
    std::io::copy(&mut input, &mut output).map_err(|_| "无法准备更新后恢复")?;
    drop(output);
    std::process::Command::new(destination)
        .arg("--wait-update")
        .arg(instance.to_string())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| "无法准备更新后恢复")?;
    Ok(())
}
pub fn finish_update_restart(instance: u64) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "无法等待更新")?;
    runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(180);
        while tokio::time::Instant::now() < deadline {
            if !maintenance_busy() {
                if let Ok(value) = portal_service::windows::request(
                    &portal_service::windows::ClientRequest::Status,
                )
                .await
                {
                    if value["status"]["phase"] != "UPDATING"
                        && (value["status"]["instance_id"].as_u64() != Some(instance)
                            || value["status"]["update"]["phase"] == "failed")
                    {
                        return start_tray();
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        start_tray()
    })
}
