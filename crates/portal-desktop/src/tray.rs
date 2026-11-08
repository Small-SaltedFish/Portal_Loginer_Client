use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use portal_desktop::{
    client::{Event, Tag, Worker},
    model::{self, NotificationGate, StatusView},
    notifications, platform,
};
use portal_service::windows::ClientRequest;
use std::{
    mem::zeroed,
    ptr,
    sync::{mpsc, Arc},
    time::Instant,
};
use tray_icon::{Icon, MouseButton, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::{System::Threading::GetCurrentThreadId, UI::WindowsAndMessaging::*};
enum Input {
    Menu(muda::MenuId),
    Open,
}
pub fn run() -> Result<(), String> {
    let preview = std::env::args().any(|arg| arg == "--preview");
    let smoke = std::env::args().any(|arg| arg == "--smoke-test");
    if std::env::args().any(|arg| arg == "--scheduled-start") {
        let _ = platform::remove_legacy_startup();
    }
    if !preview && !smoke && platform::maintenance_busy() {
        platform::wait_and_restart_tray(0)?;
        return Ok(());
    }
    let Some(_instance) = platform::single_instance(
        if preview {
            r"Local\PortalLoginer.Tray.Preview.v1"
        } else {
            r"Local\PortalLoginer.Tray.v1"
        },
        None,
    )?
    else {
        return Ok(());
    };
    let _apartment = platform::Apartment::new()?;
    if !preview && !smoke {
        let _ = notifications::initialize();
    }
    let mut message = unsafe { zeroed::<MSG>() };
    unsafe {
        PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }
    let thread_id = unsafe { GetCurrentThreadId() };
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || unsafe {
        PostThreadMessageW(thread_id, WM_APP + 1, 0, 0);
    });
    let (input_tx, input_rx) = mpsc::channel();
    let menu_tx = input_tx.clone();
    let menu_wake = wake.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = menu_tx.send(Input::Menu(event.id));
        menu_wake();
    }));
    let icon_wake = wake.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if matches!(
            event,
            TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
        ) {
            let _ = input_tx.send(Input::Open);
            icon_wake();
        }
    }));
    let state = MenuItem::new("等待后台服务", false, None);
    let open = MenuItem::new("打开设置", true, None);
    let check = MenuItem::new("立即检查", false, None);
    let relogin = MenuItem::new("登录", false, None);
    let pause = MenuItem::new("退出登录", false, None);
    let logs = MenuItem::new("查看日志", true, None);
    let update = MenuItem::new("检查更新", false, None);
    let quit = MenuItem::new("退出程序", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &state,
        &PredefinedMenuItem::separator(),
        &open,
        &check,
        &relogin,
        &pause,
        &logs,
        &update,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|_| "无法创建托盘菜单。".to_owned())?;
    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Portal Loginer · 等待后台服务")
        .with_icon(Icon::from_rgba(model::icon_rgba(false), 32, 32).map_err(|_| "无法创建图标。")?)
        .with_menu_on_left_click(false)
        .build()
        .map_err(|_| "无法启动托盘图标。".to_owned())?;
    let worker = Worker::new(wake, preview);
    let mut exiting = false;
    let mut last_online = false;
    let mut network_error_notified = false;
    let mut gate = NotificationGate::new(if preview || smoke {
        None
    } else {
        notifications::seen()
    });
    worker.send(
        Tag::Activity,
        ClientRequest::UserActivity {
            busy: platform::fullscreen_busy(),
        },
    );
    let activity_timer = unsafe { SetTimer(ptr::null_mut(), 0, 30000, None) };
    let started = Instant::now();
    if smoke {
        unsafe {
            SetTimer(ptr::null_mut(), 1, 12000, None);
        }
    }
    'running: loop {
        let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if smoke && started.elapsed().as_secs() >= 12 {
            break;
        }
        if message.message == WM_TIMER && message.wParam == activity_timer {
            worker.send(
                Tag::Activity,
                ClientRequest::UserActivity {
                    busy: platform::fullscreen_busy(),
                },
            );
        }
        while let Ok(event) = worker.events.try_recv() {
            match event {
                Event::Reply(Tag::Activity, _) => {}
                Event::Reply(Tag::Quit, Ok(value)) if value["ok"] == true => {
                    platform::close_settings(preview);
                    break 'running;
                }
                Event::Reply(Tag::Quit, _) => {
                    exiting = false;
                    quit.set_enabled(true);
                    quit.set_text("退出程序");
                    platform::show_error("后台未能停止，程序尚未完全退出，请重试。");
                }
                Event::Status(value) => {
                    if exiting {
                        continue;
                    }
                    let status = StatusView::parse(&value).unwrap_or_else(|| StatusView {
                        phase: "UNAVAILABLE".into(),
                        ..Default::default()
                    });
                    if status.phase == "UPDATING"
                        && platform::wait_and_restart_tray(status.instance_id).is_ok()
                    {
                        platform::close_settings(preview);
                        break 'running;
                    }
                    let available = status.phase != "UNAVAILABLE";
                    update.set_enabled(available);
                    let error = matches!(
                        status.phase.as_str(),
                        "NETWORK_ERROR" | "MANUAL_REQUIRED" | "PROBE_UNKNOWN"
                    );
                    if error
                        && !network_error_notified
                        && !preview
                        && value["network_error_notifications"] == true
                    {
                        network_error_notified = notifications::show_network_error().is_ok();
                    }
                    if status.online() {
                        network_error_notified = false;
                    }
                    state.set_text(status.label());
                    check.set_enabled(available);
                    relogin.set_enabled(available);
                    pause.set_enabled(available);
                    let _ = icon.set_tooltip(Some(format!("Portal Loginer · {}", status.label())));
                    if last_online != status.online() {
                        last_online = status.online();
                        let _ = icon
                            .set_icon(Icon::from_rgba(model::icon_rgba(last_online), 32, 32).ok());
                    }
                    if !preview
                        && !smoke
                        && gate.observe(&status, value["success_notifications"] == true)
                        && !worker.send(Tag::Confirm, ClientRequest::Status)
                    {
                        gate.cancel_pending();
                    }
                }
                Event::Reply(Tag::Confirm, Err(_)) => {
                    gate.cancel_pending();
                }
                Event::Reply(Tag::Confirm, Ok(value)) => {
                    if let Some(status) = StatusView::parse(&value) {
                        if let Some(key) =
                            gate.confirm(&status, value["success_notifications"] == true)
                        {
                            match notifications::show_connected() {
                                Ok(()) => {
                                    gate.delivered(key);
                                    notifications::save_seen(key);
                                }
                                Err(_) => {
                                    let _ = icon
                                        .set_tooltip(Some("通知未能发送，请在设置中查看日志。"));
                                }
                            }
                        }
                    }
                }
                Event::Reply(Tag::Start, Err(error)) => {
                    state.set_text(model::error_text(&error));
                    let _ = icon.set_tooltip(Some(model::error_text(&error)));
                }
                Event::Reply(_, Err(_)) => {
                    state.set_text("暂时无法连接后台服务");
                }
                Event::Reply(_, Ok(value)) => {
                    if value["ok"] != true {
                        let _ = icon.set_tooltip(Some("操作暂未完成，请打开设置查看。"));
                    }
                }
            }
        }
        while let Ok(input) = input_rx.try_recv() {
            if exiting {
                continue;
            }
            match input {
                Input::Open => {
                    let _ = platform::open_settings(false, preview);
                }
                Input::Menu(id) => {
                    if id == *open.id() {
                        let _ = platform::open_settings(false, preview);
                    } else if id == *logs.id() {
                        let _ = platform::open_settings(true, preview);
                    } else if id == *check.id() {
                        worker.send(Tag::Action, ClientRequest::Check);
                    } else if id == *relogin.id() {
                        worker.send(Tag::Action, ClientRequest::Relogin);
                    } else if id == *pause.id() {
                        worker.send(Tag::Action, ClientRequest::Logout);
                    } else if id == *update.id() {
                        worker.send(Tag::Update, ClientRequest::CheckUpdates);
                    } else if id == *quit.id() && worker.quit() {
                        exiting = true;
                        quit.set_enabled(false);
                        quit.set_text("正在退出…");
                        state.set_text("正在停止后台…");
                        check.set_enabled(false);
                        relogin.set_enabled(false);
                        pause.set_enabled(false);
                    }
                }
            }
        }
    }
    unsafe {
        KillTimer(ptr::null_mut(), activity_timer);
    }
    if smoke {
        unsafe {
            KillTimer(ptr::null_mut(), 1);
        }
    }
    drop(icon);
    drop(worker);
    Ok(())
}
