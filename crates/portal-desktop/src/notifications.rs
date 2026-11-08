use crate::{model::NotificationKey, platform};
use windows::{
    core::{Interface, HSTRING, PCWSTR, PROPVARIANT},
    Data::Xml::Dom::XmlDocument,
    Win32::{
        Storage::EnhancedStorage::PKEY_AppUserModel_ID,
        System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER},
        UI::Shell::{IShellLinkW, PropertiesSystem::IPropertyStore, ShellLink},
    },
    UI::Notifications::{NotificationSetting, ToastNotification, ToastNotificationManager},
};
pub const APP_ID: &str = "SmallSaltedFish.PortalLoginer.Desktop";
const PREFS: &str = r"Software\SmallSaltedFish\PortalLoginer";
pub fn seen() -> Option<NotificationKey> {
    serde_json::from_str(&platform::reg_string(PREFS, "LastNotification")?).ok()
}
pub fn save_seen(key: NotificationKey) {
    if let Ok(value) = serde_json::to_string(&key) {
        let _ = platform::set_reg_string(PREFS, "LastNotification", &value);
    }
}
pub fn initialize() -> Result<(), String> {
    register().map_err(|e| format!("通知注册失败：{:08X}", e.code().0 as u32))
}
pub fn history_count() -> Result<u32, String> {
    ToastNotificationManager::History()
        .and_then(|h| h.GetHistoryWithId(&HSTRING::from(APP_ID)))
        .and_then(|v| v.Size())
        .map_err(|e| format!("通知历史读取失败：{:08X}", e.code().0 as u32))
}
pub fn diagnostic() -> String {
    platform::reg_string(PREFS, "LastNotificationResult").unwrap_or_else(|| "尚无通知记录".into())
}
fn record(message: &str) {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let _ = platform::set_reg_string(
        PREFS,
        "LastNotificationResult",
        &format!("{time} {message}"),
    );
}
fn register() -> windows::core::Result<()> {
    let exe =
        platform::sibling("portal-settings.exe").map_err(|_| windows::core::Error::from_win32())?;
    let path = platform::wide(&exe);
    unsafe {
        windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(PCWSTR(
            platform::wide(APP_ID).as_ptr(),
        ))?;
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(path.as_ptr()))?;
        let store: IPropertyStore = link.cast()?;
        store.SetValue(&PKEY_AppUserModel_ID, &PROPVARIANT::from(APP_ID))?;
        store.Commit()?;
        let folder = platform::programs_folder().map_err(|_| windows::core::Error::from_win32())?;
        let destination = platform::wide(folder.join("Portal Loginer.lnk"));
        let file: IPersistFile = link.cast()?;
        file.Save(PCWSTR(destination.as_ptr()), true)?;
    }
    Ok(())
}
pub fn show_connected() -> Result<(), String> {
    show(0)
}
pub fn show_network_error() -> Result<(), String> {
    show(1)
}
pub fn show_test() -> Result<(), String> {
    show(2)
}
fn show(kind: u8) -> Result<(), String> {
    let result = send(kind);
    if let Err(error) = &result {
        record(error);
    }
    result
}
fn send(kind: u8) -> Result<(), String> {
    initialize()?;
    let document = XmlDocument::new().map_err(|_| "无法创建系统通知。".to_owned())?;
    document.LoadXml(&HSTRING::from(if kind==2 {"<toast><visual><binding template=\"ToastGeneric\"><text>Portal Loginer 通知测试</text><text>这是一条测试通知，不表示校园网认证结果。</text></binding></visual><audio silent=\"true\"/></toast>"}else if kind==1 {"<toast><visual><binding template=\"ToastGeneric\"><text>校园网络异常</text><text>当前未连接互联网，请检查代理配置，或登录参数可能已更新。</text></binding></visual><audio silent=\"true\"/></toast>"}else{"<toast><visual><binding template=\"ToastGeneric\"><text>校园网已连接</text><text>Portal Loginer 已完成认证，可以继续使用网络。</text></binding></visual><audio silent=\"true\"/></toast>"}))
        .map_err(|_|"无法创建系统通知。".to_owned())?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
        .map_err(|_| "系统通知不可用。".to_owned())?;
    let setting_note = match notifier.Setting() {
        Ok(setting) if setting != NotificationSetting::Enabled => {
            record(&format!("系统禁用通知：{setting:?}"));
            return Ok(());
        }
        Ok(_) => String::new(),
        Err(error) => format!(
            "（设置查询失败 {:08X}，已交由系统处理）",
            error.code().0 as u32
        ),
    };
    let toast = ToastNotification::CreateToastNotification(&document)
        .map_err(|_| "无法创建系统通知。".to_owned())?;
    toast
        .Failed(&windows::Foundation::TypedEventHandler::<
            ToastNotification,
            windows::UI::Notifications::ToastFailedEventArgs,
        >::new(|_, args| {
            if let Some(args) = args {
                if let Ok(code) = args.ErrorCode() {
                    record(&format!("系统异步拒绝通知：{:08X}", code.0 as u32));
                }
            }
            Ok(())
        }))
        .map_err(|e| format!("无法监测通知结果：{:08X}", e.code().0 as u32))?;
    notifier
        .Show(&toast)
        .map_err(|e| format!("系统未接受通知：{:08X}", e.code().0 as u32))?;
    record(&format!(
        "已提交 Windows，横幅显示由系统通知和勿扰设置决定{setting_note}"
    ));
    Ok(())
}
