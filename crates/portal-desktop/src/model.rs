use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct NetworkView {
    pub interface_index: Option<u32>,
    pub ssid: Option<String>,
    pub ipv4: Option<String>,
    pub is_wifi: bool,
    pub route_ready: bool,
    pub wifi_system_profile: Option<bool>,
    pub wifi_auto_connect: Option<bool>,
    pub possible_tunnel_route: bool,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct StatusView {
    pub version: String,
    pub network_online: Option<bool>,
    pub instance_id: u64,
    pub phase: String,
    pub authentication_id: u64,
    pub failures: u32,
    pub network: Option<NetworkView>,
    pub issue: Option<String>,
    pub last_authentication_unix: Option<u64>,
    pub update: portal_service::windows::UpdateView,
}
impl StatusView {
    pub fn parse(value: &serde_json::Value) -> Option<Self> {
        if value["ok"] != true {
            return None;
        }
        serde_json::from_value(value["status"].clone()).ok()
    }
    pub fn key(&self) -> NotificationKey {
        NotificationKey {
            instance: self.instance_id,
            authentication: self.authentication_id,
        }
    }
    pub fn label(&self) -> &'static str {
        if self.phase == "WAIT_NETWORK" {
            return match self.network.as_ref() {
                Some(n) if n.ipv4.is_none() => "等待网络分配地址",
                Some(n) if !n.route_ready => "网络已连接，等待可用路由",
                _ => "网络变化后正在确认状态",
            };
        }
        phase_text(&self.phase)
    }
    pub fn online(&self) -> bool {
        self.phase == "LOGGED_IN"
    }
}
pub fn phase_text(phase: &str) -> &'static str {
    match phase {
        "NETWORK_ERROR" => "网络异常，请检查 TUN 等代理配置",
        "MANUAL_REQUIRED" => "当前未连接互联网，请检查代理配置，或登录参数可能已更新；需要手动操作",
        "UPDATING" => "正在静默更新",
        "LOGGED_IN" => "校园网已连接",
        "LOGGING_IN" => "正在认证",
        "VERIFYING" => "等待联网确认",
        "CHECKING" => "正在检查网络",
        "WAIT_NETWORK" => "网络正在稳定，稍后重试",
        "WAIT_WIFI" => "等待指定 Wi-Fi",
        "PAUSED" => "自动认证已暂停",
        "OFFLINE" => "未连接到任何网络",
        "PROBE_UNKNOWN" => "网络状态未确认",
        "FAILED" => "认证暂未成功",
        "NEEDS_CONFIGURATION" => "未配置账号",
        "AUTHENTICATION_REQUIRED" => "校园网需要认证",
        "UNAVAILABLE" => "后台服务未连接",
        "ONLINE_UNVERIFIED" => "当前已联网，账号尚未验证",
        "NEEDS_LOGOUT" => "请先注销当前会话以验证账号",
        "LOGGING_OUT" => "正在退出当前登录",
        "VERIFICATION_CANCELLED" => "账号验证已中断",
        _ => "等待后台服务",
    }
}
pub fn error_text(error: &str) -> &'static str {
    match error {
        "busy" => "正在处理，请稍后重试。",
        "access_denied" => "当前 Windows 用户没有访问此服务的权限。",
        "service_unavailable" => "后台服务未连接。",
        "credential_unavailable" => "无法读取已保存的密码，请重新输入密码后验证。",
        "operation_failed" | "invalid_settings" => "操作未完成，请检查账号、Wi-Fi 名称和高级设置。",
        "verification_network_unavailable" => {
            "请连接校园宽带 Wi-Fi，检查 Wi-Fi 限制和注销预设后重试。"
        }
        _ => "连接失败，请重试。",
    }
}
pub fn issue_text(issue: Option<&str>) -> &'static str {
    match issue {
        Some("portal_unavailable") => {
            "无法访问校园认证服务器，请连接校园网络，手动退出宽带登录或等待本次登录到期后再配置。"
        }
        Some("autostart_failure") => "账号已验证，但开机启动设置未生效，请检查服务设置。",
        Some("probe_unreachable") => "直连探测全部失败，请查看日志中的超时或连接错误。",
        Some("invalid_portal_redirect") => "认证跳转参数不匹配，未发送账号密码。请查看网络诊断。",
        Some("probe_unexpected_response") => "探测收到非预期网页，尚未找到有效认证入口。",
        Some("login_rejected") => "认证接口未接受登录，请核对账号密码并查看日志。",
        Some("login_transport_failed") => "无法连接认证接口，或接口返回 HTTP 错误。",
        Some("login_invalid_response") => "认证接口未返回有效结果，可能受到代理或网络拦截。",
        Some("confirmation_failed") => "认证确认未成功，请稍后重试。",
        Some("credential_unavailable") => "无法读取已保存的密码，请重新输入并保存。",
        Some("storage_failure") => "配置保存失败，请检查磁盘和权限。",
        Some("logout_failed") => {
            "注销未成功，请检查高级设置中的校园认证地址和 bras，或从校园网页手动注销。"
        }
        Some("verification_network_changed") => {
            "网络已变化，验证已停止，未保存本次账号密码。请重试。"
        }
        Some("verification_interrupted") => "验证已中断，未保存本次账号密码。请重试。",
        _ => "尚未完成登录，请检查当前网络或查看日志。",
    }
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationKey {
    pub instance: u64,
    pub authentication: u64,
}
#[derive(Default)]
pub struct NotificationGate {
    pub seen: Option<NotificationKey>,
    pending: Option<NotificationKey>,
}
impl NotificationGate {
    pub fn new(seen: Option<NotificationKey>) -> Self {
        Self {
            seen,
            pending: None,
        }
    }
    pub fn observe(&mut self, status: &StatusView, enabled: bool) -> bool {
        let key = status.key();
        if !enabled {
            self.seen = Some(key);
            self.pending = None;
            return false;
        }
        if !status.online()
            || key.authentication == 0
            || self.seen == Some(key)
            || self.pending == Some(key)
        {
            return false;
        }
        self.pending = Some(key);
        true
    }
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }
    pub fn delivered(&mut self, key: NotificationKey) {
        self.seen = Some(key);
    }
    pub fn confirm(&mut self, status: &StatusView, enabled: bool) -> Option<NotificationKey> {
        let key = self.pending.take()?;
        (enabled && status.online() && status.key() == key).then_some(key)
    }
}
pub fn app_icon_rgba() -> Vec<u8> {
    include_bytes!("../../../assets/app.rgba").to_vec()
}
pub fn icon_rgba(online: bool) -> Vec<u8> {
    if online {
        include_bytes!("../../../assets/tray-online.rgba").to_vec()
    } else {
        include_bytes!("../../../assets/tray-waiting.rgba").to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn online(id: u64) -> StatusView {
        StatusView {
            instance_id: 5,
            phase: "LOGGED_IN".into(),
            authentication_id: id,
            ..Default::default()
        }
    }
    #[test]
    fn notifications_require_fresh_online_confirmation_and_deduplicate() {
        let mut gate = NotificationGate::default();
        assert!(gate.observe(&online(1), true));
        assert!(!gate.observe(&online(1), true));
        let key = gate.confirm(&online(1), true).unwrap();
        gate.delivered(key);
        assert!(!gate.observe(&online(1), true));
        assert!(gate.observe(&online(2), true));
        assert!(gate
            .confirm(
                &StatusView {
                    phase: "PROBE_UNKNOWN".into(),
                    ..online(2)
                },
                true
            )
            .is_none());
    }
    #[test]
    fn disabled_notifications_and_plain_online_checks_do_not_notify() {
        let mut gate = NotificationGate::default();
        assert!(!gate.observe(&online(0), true));
        assert!(!gate.observe(&online(1), false));
        assert!(!gate.observe(&online(1), true));
        assert!(gate.observe(&online(2), true));
        assert!(gate.confirm(&online(2), false).is_none());
    }
    #[test]
    fn saved_notification_key_survives_tray_restart() {
        let mut gate = NotificationGate {
            seen: Some(online(1).key()),
            ..Default::default()
        };
        assert!(!gate.observe(&online(1), true));
        assert!(gate.observe(
            &StatusView {
                instance_id: 6,
                ..online(1)
            },
            true
        ));
    }
    #[test]
    fn transient_check_does_not_consume_success_notification() {
        let mut gate = NotificationGate::default();
        assert!(gate.observe(&online(1), true));
        assert!(gate
            .confirm(
                &StatusView {
                    phase: "CHECKING".into(),
                    ..online(1)
                },
                true
            )
            .is_none());
        assert!(gate.observe(&online(1), true));
        let key = gate.confirm(&online(1), true).unwrap();
        gate.delivered(key);
        assert!(!gate.observe(&online(1), true));
    }
    #[test]
    fn rejected_notification_can_retry_same_authentication() {
        let mut gate = NotificationGate::default();
        assert!(gate.observe(&online(1), true));
        assert!(gate.confirm(&online(1), true).is_some());
        assert!(gate.observe(&online(1), true));
    }
    #[test]
    fn tray_icon_has_expected_size_and_transparency() {
        let bytes = icon_rgba(true);
        assert_eq!(bytes.len(), 4096);
        assert!(bytes.chunks_exact(4).any(|pixel| pixel[3] == 0));
        assert!(bytes.chunks_exact(4).any(|pixel| pixel[3] == 255));
    }
}
