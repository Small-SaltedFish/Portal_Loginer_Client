use super::{App, Page, GREEN, INK, MUTED, PAPER};
use eframe::egui::{self, Color32, RichText, Vec2};
use portal_desktop::client::Tag;
use portal_desktop::model;
use portal_service::windows::ClientRequest;
use zeroize::Zeroize;

const RADIUS: u8 = 4;
const BORDER: Color32 = Color32::from_rgb(218, 225, 231);
const SELECTED: Color32 = Color32::from_rgb(226, 240, 235);
const CONTROL_HEIGHT: f32 = 34.0;

pub(super) fn configure(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.set_visuals(egui::Visuals::light());
    ctx.style_mut(|style| {
        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.spacing.interact_size = Vec2::new(40.0, CONTROL_HEIGHT);
        style.spacing.button_padding = Vec2::new(12.0, 7.0);
        style.spacing.text_edit_width = 280.0;
        style.visuals.override_text_color = Some(INK);
        style.visuals.extreme_bg_color = Color32::WHITE;
        style.visuals.selection.bg_fill = SELECTED;
        style.visuals.selection.stroke = egui::Stroke::new(1.0, GREEN);
        for widget in [
            &mut style.visuals.widgets.noninteractive,
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widget.corner_radius = egui::CornerRadius::same(RADIUS);
            widget.bg_stroke = egui::Stroke::new(1.0, BORDER);
            widget.fg_stroke = egui::Stroke::new(1.0, INK);
            widget.expansion = 0.0;
        }
        style.visuals.widgets.inactive.weak_bg_fill = Color32::WHITE;
        style.visuals.widgets.hovered.weak_bg_fill = PAPER;
        style.visuals.widgets.active.weak_bg_fill = SELECTED;
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    });
}

fn field<'a>(text: &'a mut String, hint: &str) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        .desired_width(f32::INFINITY)
        .margin(egui::Margin::symmetric(10, 8))
        .hint_text(RichText::new(hint).color(MUTED))
}

fn heading(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).size(22.0).strong());
    ui.add_space(14.0);
}

impl App {
    fn account(&mut self, ui: &mut egui::Ui) {
        heading(ui, "校园网登录");
        if !self.interfaces.is_empty() || self.settings.interface_index.is_some() {
            let previous = self.settings.interface_index;
            egui::ComboBox::from_label("认证网络出口")
                .selected_text(
                    previous
                        .map(|i| {
                            self.interfaces
                                .iter()
                                .find(|n| n.interface_index == Some(i))
                                .map(|n| {
                                    format!(
                                        "{} · 接口 {i}",
                                        if n.is_wifi { "Wi-Fi" } else { "有线" }
                                    )
                                })
                                .unwrap_or_else(|| format!("接口 {i} 当前未连接"))
                        })
                        .unwrap_or("跟随 Windows 默认路由".into()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.settings.interface_index,
                        None,
                        "跟随 Windows 默认路由",
                    );
                    for network in &self.interfaces {
                        let label = format!(
                            "{} {}",
                            if network.is_wifi {
                                network.ssid.as_deref().unwrap_or("Wi-Fi")
                            } else {
                                "有线"
                            },
                            network.ipv4.as_deref().unwrap_or("")
                        );
                        ui.selectable_value(
                            &mut self.settings.interface_index,
                            network.interface_index,
                            label,
                        );
                    }
                });
            if previous != self.settings.interface_index {
                self.send(ClientRequest::SelectInterface {
                    index: self.settings.interface_index,
                });
            }
        }

        let width = (ui.available_width() - 28.0).max(0.0);
        egui::Frame::new()
            .fill(PAPER)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .corner_radius(RADIUS)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_min_width(width);
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                    ui.painter().circle_filled(
                        dot.center(),
                        3.5,
                        if self.status.online() { GREEN } else { MUTED },
                    );
                    ui.label(RichText::new(self.status.label()).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(self.loaded && !self.busy && !self.login_pending, |ui| {
                            if ui.button("退出登录").clicked() {
                                self.send(ClientRequest::Logout);
                            }
                            if ui
                                .button("登录")
                                .on_hover_text("先尝试退出当前登录，再探测并登录；已退出时仍继续。")
                                .clicked()
                            {
                                self.send(ClientRequest::Relogin);
                            }
                            if ui.button("检查").clicked() {
                                self.send(ClientRequest::Check);
                            }
                        });
                    });
                });
                if let Some(network) = &self.status.network {
                    let text = network.label();
                    ui.label(RichText::new(text).small().color(MUTED));
                    if network.possible_tunnel_route {
                        ui.label(
                            RichText::new("当前出口可能受 VPN 或 TUN 影响")
                                .small()
                                .color(MUTED),
                        );
                    }
                }
            });
        ui.add_space(18.0);
        if self.status.phase == "NEEDS_LOGOUT" {
            ui.label("需要退出当前宽带登录，才能继续配置并验证账号密码。");
            if ui.button("退出当前登录并重新探测").clicked() {
                self.send(ClientRequest::Logout);
            }
        }
        if self.status.issue.is_some() {
            ui.label(
                RichText::new(model::issue_text(self.status.issue.as_deref()))
                    .small()
                    .color(Color32::from_rgb(177, 64, 60)),
            );
        }

        if !self.loaded {
            ui.label("首次使用请安装客户端，安装时需要系统授权。");
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new("安装客户端"))
                    .clicked()
                {
                    self.busy = self.worker.setup();
                    self.message = "等待系统授权…".into();
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("重试连接"))
                    .clicked()
                {
                    self.worker.send(Tag::Load, ClientRequest::GetSettings);
                }
            });
            ui.add_space(14.0);
        }

        ui.add_enabled_ui(
            self.loaded
                && !self.busy
                && !self.login_pending
                && (self.has_password || self.status.phase == "AUTHENTICATION_REQUIRED"),
            |ui| {
                ui.label("校园网账号");
                ui.add(field(&mut self.settings.user, "输入账号"));
                ui.add_space(6.0);
                ui.label("密码");
                let hint = if self.has_password {
                    "密码已保存，留空不修改"
                } else {
                    "输入密码"
                };
                ui.add(field(&mut self.password, hint).password(true));
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);
                ui.checkbox(&mut self.wifi_only, "仅在指定 Wi-Fi 下自动登录")
                    .on_hover_text("Wi-Fi 名称需完全一致；有线连接不触发登录。");
                if self.wifi_only {
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 122.0).max(160.0);
                        ui.add(field(&mut self.ssid, "Wi-Fi 名称").desired_width(width));
                        let current = self
                            .status
                            .network
                            .as_ref()
                            .and_then(|network| network.ssid.clone());
                        if ui
                            .add_enabled(current.is_some(), egui::Button::new("使用当前网络"))
                            .clicked()
                        {
                            self.ssid = current.unwrap_or_default();
                        }
                    });
                    if let Some(network) = &self.status.network {
                        if network.is_wifi
                            && (network.wifi_system_profile == Some(false)
                                || network.wifi_auto_connect == Some(false))
                        {
                            ui.label(
                                RichText::new("登录 Windows 前可能无法连接此 Wi-Fi。")
                                    .small()
                                    .color(MUTED),
                            )
                            .on_hover_text("请检查 Windows 中的系统级 Wi-Fi 配置和自动连接设置。");
                        }
                    }
                }
                ui.checkbox(&mut self.notification, "登录成功后通知")
                    .on_hover_text("遵循系统勿扰设置。");
                ui.checkbox(
                    &mut self.settings.network_error_notifications,
                    "网络错误时通知",
                );
                if ui.button("测试 Windows 通知").clicked() {
                    if self.preview {
                        self.message = "预览模式不发送通知。".into();
                    } else {
                        let result = portal_desktop::notifications::show_test();
                        self.error = result.is_err();
                        self.message = portal_desktop::notifications::diagnostic();
                    }
                }
                ui.checkbox(&mut self.autostart, "开机自启");
            },
        );
    }

    fn advanced(&mut self, ui: &mut egui::Ui) {
        heading(ui, "高级设置");
        ui.add_enabled_ui(self.loaded && !self.busy && !self.login_pending, |ui| {
            egui::Grid::new("advanced-grid")
                .spacing([28.0, 14.0])
                .show(ui, |ui| {
                    ui.label("检查间隔");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.check_seconds).range(5..=600),
                        );
                        ui.label("秒");
                    });
                    ui.end_row();
                    ui.label("失败重试间隔");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.retry_seconds).range(3..=300),
                        );
                        ui.label("秒");
                    });
                    ui.end_row();
                    ui.checkbox(
                        &mut self.settings.force_boot_login,
                        "开机网络就绪后重新登录一次",
                    );
                    ui.end_row();
                });
            ui.add_space(22.0);
            ui.checkbox(&mut self.settings.debug_log, "启用详细 debug 日志");
            ui.horizontal(|ui| {
                ui.label("日志最大保存大小");
                let mut kib = self.settings.log_max_bytes / 1024;
                ui.add(egui::DragValue::new(&mut kib).range(64..=16384));
                ui.label("KiB");
                self.settings.log_max_bytes = kib * 1024;
            });
            ui.label("网络检测地址");
            ui.add(field(&mut self.settings.probe_url, "http://…"))
                .on_hover_text("用于检查联网状态，需返回 HTTP 204。");
        });
        ui.add_space(22.0);
        ui.separator();
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.settings.automatic_updates, "自动下载并静默更新");
        });
    }

    fn about(&mut self, ui: &mut egui::Ui) {
        heading(ui, "关于 Portal Loginer");
        ui.horizontal(|ui| {
            ui.image((self.brand_texture.id(), Vec2::splat(56.0)));
            ui.vertical(|ui| {
                ui.label(RichText::new("Portal Loginer").size(22.0).strong());
                ui.label("校园网自动登录客户端");
            });
        });
        ui.add_space(20.0);
        egui::Grid::new("version-info")
            .spacing([24.0, 12.0])
            .show(ui, |ui| {
                ui.label("客户端版本");
                ui.label(env!("CARGO_PKG_VERSION"));
                ui.end_row();
                ui.label("后台版本");
                ui.label(if self.status.version.is_empty() {
                    "未连接"
                } else {
                    &self.status.version
                });
                ui.end_row();
                ui.label("更新状态");
                ui.label(if self.status.update.message.is_empty() {
                    "等待检查更新"
                } else {
                    &self.status.update.message
                });
                ui.end_row();
                if !self.status.update.version.is_empty() {
                    ui.label("可用版本");
                    ui.label(&self.status.update.version);
                    ui.end_row();
                }
                if !self.status.update.source.is_empty() {
                    ui.label("下载线路");
                    ui.label(
                        self.status
                            .update
                            .source
                            .trim_start_matches("https://")
                            .trim_end_matches('/'),
                    );
                    ui.end_row();
                }
                ui.label("开源许可");
                ui.label("Apache-2.0");
                ui.end_row();
            });
        ui.add_space(12.0);
        if self.status.update.total > 0 && self.status.update.phase == "downloading" {
            ui.add(
                egui::ProgressBar::new(
                    (self.status.update.downloaded as f64 / self.status.update.total as f64) as f32,
                )
                .show_percentage(),
            );
        }
        if ui
            .add_enabled(self.loaded && !self.busy, egui::Button::new("检查更新"))
            .clicked()
        {
            self.worker.send(Tag::Update, ClientRequest::CheckUpdates);
        }
        if matches!(
            self.status.update.phase.as_str(),
            "available" | "ready" | "deferred" | "failed"
        ) && ui
            .button(if self.status.update.phase == "failed" {
                "重试更新"
            } else {
                "下载并安装"
            })
            .clicked()
        {
            self.worker.send(Tag::Update, ClientRequest::InstallUpdate);
        }
        ui.add_space(20.0);
        ui.hyperlink_to(
            "项目主页",
            "https://github.com/Small-SaltedFish/Portal_Loginer_Client",
        );
        ui.hyperlink_to(
            "反馈问题",
            "https://github.com/Small-SaltedFish/Portal_Loginer_Client/issues",
        );
    }
    fn logs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("日志").size(22.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("刷新").clicked() {
                    self.worker.send(Tag::Logs, ClientRequest::Logs);
                }
            });
        });
        ui.add_space(14.0);
        let width = (ui.available_width() - 24.0).max(0.0);
        egui::Frame::new()
            .fill(PAPER)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .corner_radius(RADIUS)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_min_width(width);
                ui.set_min_height(320.0);
                ui.monospace(if self.logs.is_empty() {
                    "暂无日志"
                } else {
                    &self.logs
                });
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.events();
        if self.status.phase == "EXITING" {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.login_pending
            && self.login_submitted
            && self.status.online()
            && self.status.authentication_id > self.login_authentication_id
            && !self.busy
        {
            self.login_pending = false;
            self.password.zeroize();
            self.has_password = true;
            self.saved_user = self.settings.user.clone();
            if self.status.issue.is_some() {
                self.error = true;
                self.message = model::issue_text(self.status.issue.as_deref()).into();
            } else {
                self.error = false;
                self.message =
                    "登录成功，账号密码已保存。可自行关闭设置页面，托盘会继续运行。".into();
                if !self.preview {
                    if let Err(error) = portal_desktop::platform::set_tray_autostart(self.autostart)
                        .and_then(|_| portal_desktop::platform::start_tray())
                    {
                        self.error = true;
                        self.message = format!("登录成功，账号密码已保存。{error}");
                    }
                }
            }
        }
        egui::TopBottomPanel::top("brand")
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(22, 13)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.image((self.brand_texture.id(), Vec2::splat(28.0)));
                    ui.label(RichText::new("Portal Loginer").size(18.0).strong());
                    ui.label(
                        RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .small()
                            .color(MUTED),
                    );
                    if self.preview {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new("预览模式").small().color(MUTED));
                        });
                    }
                });
            });
        egui::SidePanel::left("navigation")
            .exact_width(144.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(egui::Margin::symmetric(12, 20)),
            )
            .show(ctx, |ui| {
                for (page, text) in [
                    (Page::Account, "登录"),
                    (Page::Advanced, "高级设置"),
                    (Page::Logs, "日志"),
                    (Page::About, "关于"),
                ] {
                    let selected = self.page == page;
                    if ui
                        .add_sized(
                            [120.0, 36.0],
                            egui::Button::new(RichText::new(text).color(if selected {
                                GREEN
                            } else {
                                INK
                            }))
                            .fill(if selected { SELECTED } else { PAPER })
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(RADIUS),
                        )
                        .clicked()
                    {
                        self.page = page;
                        if page == Page::Logs {
                            self.worker.send(Tag::Logs, ClientRequest::Logs);
                        }
                    }
                }
            });
        if self.page != Page::About {
            egui::TopBottomPanel::bottom("footer")
                .frame(
                    egui::Frame::new()
                        .fill(Color32::WHITE)
                        .inner_margin(egui::Margin::symmetric(24, 12)),
                )
                .show(ctx, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                self.loaded && !self.busy && !self.login_pending,
                                egui::Button::new(
                                    RichText::new(if self.busy || self.login_pending {
                                        "处理中…"
                                    } else if self.page == Page::Account {
                                        "保存并登录"
                                    } else {
                                        "保存设置"
                                    })
                                    .color(Color32::WHITE),
                                )
                                .fill(GREEN)
                                .stroke(egui::Stroke::NONE)
                                .min_size(Vec2::new(88.0, CONTROL_HEIGHT)),
                            )
                            .clicked()
                        {
                            self.save(self.page == Page::Account);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.label(RichText::new(&self.message).small().color(if self.error {
                                Color32::from_rgb(177, 64, 60)
                            } else {
                                MUTED
                            }));
                        });
                    });
                });
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(24, 22)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| match self.page {
                    Page::Account => self.account(ui),
                    Page::Advanced => self.advanced(ui),
                    Page::Logs => self.logs(ui),
                    Page::About => self.about(ui),
                });
            });
    }
}
