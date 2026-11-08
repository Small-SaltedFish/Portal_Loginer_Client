use eframe::egui::{self, Color32};
use portal_desktop::{
    client::{Event, Tag, Worker},
    model::{self, StatusView},
    platform,
};
use portal_service::windows::{ClientRequest, Settings};
use std::sync::Arc;
use zeroize::{Zeroize, Zeroizing};

const INK: Color32 = Color32::from_rgb(31, 48, 66);
const MUTED: Color32 = Color32::from_rgb(112, 126, 142);
const GREEN: Color32 = Color32::from_rgb(13, 135, 118);
const PAPER: Color32 = Color32::from_rgb(244, 247, 250);
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Account,
    Advanced,
    Logs,
    About,
}
struct App {
    brand_texture: egui::TextureHandle,
    worker: Worker,
    settings: Settings,
    password: Zeroizing<String>,
    ssid: String,
    wifi_only: bool,
    has_password: bool,
    loaded: bool,
    busy: bool,
    message: String,
    error: bool,
    status: StatusView,
    page: Page,
    logs: String,
    autostart: bool,
    preview: bool,
    notification: bool,
    login_pending: bool,
    login_submitted: bool,
    login_authentication_id: u64,
    saved_user: String,
    inspected: bool,
    interfaces: Vec<portal_desktop::model::NetworkView>,
}
impl App {
    fn new(cc: &eframe::CreationContext<'_>, preview: bool, logs: bool) -> Self {
        let ctx = cc.egui_ctx.clone();
        let mut fonts = egui::FontDefinitions::default();
        if let Ok(folder) = platform::fonts_folder() {
            for name in ["msyh.ttc", "msyh.ttf", "simhei.ttf"] {
                if let Ok(bytes) = std::fs::read(folder.join(name)) {
                    fonts
                        .font_data
                        .insert("chinese".into(), egui::FontData::from_owned(bytes).into());
                    fonts
                        .families
                        .entry(egui::FontFamily::Proportional)
                        .or_default()
                        .insert(0, "chinese".into());
                    fonts
                        .families
                        .entry(egui::FontFamily::Monospace)
                        .or_default()
                        .push("chinese".into());
                    break;
                }
            }
        }
        ctx.set_fonts(fonts);
        view::configure(&ctx);
        let brand_texture = ctx.load_texture(
            "portal-brand",
            egui::ColorImage::from_rgba_unmultiplied([32, 32], &model::app_icon_rgba()),
            egui::TextureOptions::LINEAR,
        );
        let wake = Arc::new(move || ctx.request_repaint());
        let worker = Worker::new(wake, preview);
        worker.send(Tag::Load, ClientRequest::GetSettings);
        if logs {
            worker.send(Tag::Logs, ClientRequest::Logs);
        }
        Self {
            brand_texture,
            worker,
            settings: Settings::default(),
            password: Zeroizing::new(String::new()),
            ssid: String::new(),
            wifi_only: false,
            has_password: false,
            loaded: false,
            busy: false,
            message: String::new(),
            error: false,
            status: StatusView::default(),
            page: if logs { Page::Logs } else { Page::Account },
            logs: String::new(),
            autostart: if preview {
                false
            } else {
                platform::tray_autostart_choice()
            },
            preview,
            notification: true,
            login_pending: false,
            login_submitted: false,
            login_authentication_id: 0,
            saved_user: String::new(),
            inspected: false,
            interfaces: Vec::new(),
        }
    }
    fn events(&mut self) {
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::Status(value) => {
                    if let Some(status) = StatusView::parse(&value) {
                        if !status.version.is_empty() && status.version != env!("CARGO_PKG_VERSION")
                        {
                            self.status = status;
                            self.loaded = false;
                            self.error = true;
                            self.message = "旧版后台仍在运行，请安装新版客户端。".into();
                            continue;
                        }
                        if self.status.phase == "UNAVAILABLE" || !self.loaded {
                            self.worker.send(Tag::Load, ClientRequest::GetSettings);
                        }
                        self.status = status;
                        if self.login_pending
                            && self.login_submitted
                            && matches!(
                                self.status.phase.as_str(),
                                "FAILED"
                                    | "PROBE_UNKNOWN"
                                    | "WAIT_WIFI"
                                    | "OFFLINE"
                                    | "NEEDS_LOGOUT"
                                    | "VERIFICATION_CANCELLED"
                                    | "NETWORK_ERROR"
                                    | "MANUAL_REQUIRED"
                            )
                        {
                            self.login_pending = false;
                            self.busy = false;
                            self.error = true;
                            self.message = if self.status.phase == "NEEDS_LOGOUT" {
                                "请连接校园宽带 Wi-Fi，注销当前会话后重新探测并验证账号。"
                            } else {
                                model::issue_text(self.status.issue.as_deref())
                            }
                            .into();
                        }
                    } else {
                        self.status.phase = "UNAVAILABLE".into();
                        self.login_pending = false;
                        self.busy = false;
                    }
                }
                Event::Reply(tag, result) => {
                    if matches!(tag, Tag::Inspect) {
                        continue;
                    }
                    let value = match result {
                        Ok(value) if value["ok"] == true => value,
                        Ok(value) => {
                            self.busy = false;
                            self.login_pending = false;
                            self.error = true;
                            self.message =
                                model::error_text(value["error"].as_str().unwrap_or("")).into();
                            continue;
                        }
                        Err(error) => {
                            self.busy = false;
                            self.login_pending = false;
                            self.error = true;
                            self.message = model::error_text(&error).into();
                            self.status.phase = "UNAVAILABLE".into();
                            continue;
                        }
                    };
                    match tag {
                        Tag::Load => {
                            if let Ok(settings) =
                                serde_json::from_value::<Settings>(value["settings"].clone())
                            {
                                self.interfaces =
                                    serde_json::from_value(value["interfaces"].clone())
                                        .unwrap_or_default();
                                self.wifi_only = settings.wifi_only.is_some();
                                self.ssid = settings.wifi_only.clone().unwrap_or_default();
                                self.has_password = value["has_password"] == true;
                                self.notification = settings.success_notifications;
                                self.autostart = settings.autostart;
                                if !self.has_password
                                    && self.interfaces.len() == 1
                                    && self.interfaces[0].is_wifi
                                    && settings.wifi_only.is_none()
                                {
                                    self.wifi_only = true;
                                    self.ssid = self.interfaces[0].ssid.clone().unwrap_or_default();
                                }
                                self.saved_user = settings.user.clone();
                                self.settings = settings;
                                self.loaded = self.status.version.is_empty()
                                    || self.status.version == env!("CARGO_PKG_VERSION");
                                if self.loaded && !self.inspected {
                                    self.inspected = true;
                                    self.worker.send(Tag::Inspect, ClientRequest::Check);
                                }
                            }
                        }
                        Tag::Save => {
                            self.password.zeroize();
                            self.has_password = true;
                            self.busy = false;
                            self.error = false;
                            self.message = "已保存".into();
                            if !self.preview {
                                if let Err(error) = platform::set_tray_autostart(self.autostart) {
                                    self.message = format!("账号已保存{error}");
                                    self.error = true;
                                } else {
                                    let _ = platform::start_tray();
                                }
                            }
                            if self.login_pending {
                                self.busy = self.worker.send(Tag::Login, ClientRequest::Login);
                            }
                        }
                        Tag::Login => {
                            self.login_submitted = true;
                            self.busy = false;
                            self.worker.send(Tag::Confirm, ClientRequest::Status);
                        }
                        Tag::Confirm => {
                            self.busy = false;
                            if let Some(status) = StatusView::parse(&value) {
                                self.status = status;
                            }

                            if matches!(
                                self.status.phase.as_str(),
                                "FAILED"
                                    | "PROBE_UNKNOWN"
                                    | "WAIT_WIFI"
                                    | "OFFLINE"
                                    | "NEEDS_LOGOUT"
                                    | "VERIFICATION_CANCELLED"
                                    | "NETWORK_ERROR"
                                    | "MANUAL_REQUIRED"
                            ) {
                                self.login_pending = false;
                                self.error = true;
                                self.message = if self.status.phase == "NEEDS_LOGOUT" {
                                    "请注销当前校园会话，再重新探测并验证账号。"
                                } else {
                                    model::issue_text(self.status.issue.as_deref())
                                }
                                .into();
                            }
                        }
                        Tag::Update => {
                            self.busy = false;
                            self.error = false;
                            self.message = "已请求检查更新，可在关于页查看进度。".into();
                        }
                        Tag::Logout => {
                            self.busy = false;
                            self.error = false;
                            self.message="正在退出登录，账号密码已保留。本次开机暂停自动认证，点击“登录”可恢复。".into();
                        }
                        Tag::Logs => {
                            self.logs = format!(
                                "{}\n通知：{}",
                                value["lines"].as_str().unwrap_or(""),
                                portal_desktop::notifications::diagnostic()
                            );
                            self.busy = false;
                        }
                        Tag::Setup => {
                            self.busy = false;
                            self.message = "客户端已安装".into();
                            self.error = false;
                            self.worker.send(Tag::Load, ClientRequest::GetSettings);
                            if !self.preview {
                                let _ = platform::start_tray();
                            }
                        }
                        _ => {
                            self.busy = false;
                            self.error = false;
                            self.message = "已提交".into();
                        }
                    }
                }
            }
        }
    }
    fn send(&mut self, request: ClientRequest) {
        let tag = if matches!(request, ClientRequest::Logout) {
            Tag::Logout
        } else {
            Tag::Action
        };
        if self.worker.send(tag, request) {
            self.busy = true;
            self.message.clear();
        }
    }
    fn save(&mut self, login: bool) {
        self.error = true;
        if self.settings.user.is_empty() {
            self.message = "请输入校园网账号。".into();
            return;
        }
        if !self.has_password && self.password.is_empty() {
            self.message = "请输入密码。".into();
            return;
        }
        if self.wifi_only && self.ssid.is_empty() {
            self.message = "请输入 Wi-Fi 名称。".into();
            return;
        }
        if self.password.len() > 512 || self.password.chars().any(char::is_control) {
            self.message = "密码包含不支持的字符或过长。".into();
            return;
        }
        if !login && (!self.password.is_empty() || self.settings.user != self.saved_user) {
            self.message = "账号或密码已修改，请回到账号页验证登录。".into();
            return;
        }
        let mut settings = self.settings.clone();
        if login {
            settings.paused = false;
        }
        settings.autostart = self.autostart;
        settings.success_notifications = self.notification;
        settings.wifi_only = if self.wifi_only {
            Some(self.ssid.clone())
        } else {
            None
        };
        if settings.core("5a3b9f207411a8ed").is_err() {
            self.message = "请检查 Wi-Fi 名称和高级设置中的数值。".into();
            return;
        }
        let request = if login {
            ClientRequest::Configure {
                settings,
                password: self.password.to_string(),
            }
        } else {
            ClientRequest::Save {
                settings,
                password: self.password.to_string(),
            }
        };
        if self
            .worker
            .send(if login { Tag::Login } else { Tag::Save }, request)
        {
            self.busy = true;
            self.login_pending = login;
            self.login_submitted = false;
            self.login_authentication_id = self.status.authentication_id;
            if login {
                self.status.phase = "CHECKING".into();
            }
            self.message = if login {
                "验证账号中…"
            } else {
                "保存中…"
            }
            .into();
            self.error = false;
        }
    }
}

#[path = "settings_view.rs"]
mod view;

pub fn run() -> Result<(), String> {
    let preview = std::env::args().any(|arg| arg == "--preview");
    let logs = std::env::args().any(|arg| arg == "--logs");
    let title = if preview {
        "Portal Loginer 设置（预览）"
    } else {
        "Portal Loginer 设置"
    };
    let name = if preview {
        r"Local\PortalLoginer.Settings.Preview.v1"
    } else {
        r"Local\PortalLoginer.Settings.v1"
    };
    let Some(_instance) = platform::single_instance(name, Some(title))? else {
        return Ok(());
    };
    if !preview {
        if platform::maintenance_busy() {
            return Err("程序正在安装或更新，请稍后打开。".into());
        }
        platform::start_tray()?;
    }
    let _apartment = platform::Apartment::new()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 650.0])
            .with_min_inner_size([780.0, 600.0])
            .with_icon(egui::IconData {
                rgba: model::app_icon_rgba(),
                width: 32,
                height: 32,
            }),
        run_and_return: true,
        ..Default::default()
    };
    eframe::run_native(
        title,
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, preview, logs)))),
    )
    .map_err(|_| "设置窗口无法启动。".into())
}
