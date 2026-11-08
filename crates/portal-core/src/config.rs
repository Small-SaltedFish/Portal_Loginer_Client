use crate::crypto::Key;
use std::{net::Ipv4Addr, time::Duration};
use zeroize::Zeroizing;

pub const DEFAULT_PROBE: &str = "http://connect.rom.miui.com/generate_204";
pub const BACKUP_PROBES: [&str; 2] = [
    "http://connectivitycheck.platform.hicloud.com/generate_204",
    "http://wifi.vivo.com.cn/generate_204",
];

pub struct Credentials {
    user: Zeroizing<String>,
    password: Zeroizing<String>,
}

impl Credentials {
    pub fn new(user: String, password: String) -> Result<Self, ConfigError> {
        let user = Zeroizing::new(user);
        let password = Zeroizing::new(password);
        if user.is_empty()
            || user.len() > 128
            || password.is_empty()
            || password.len() > 512
            || user.chars().any(char::is_control)
            || password.chars().any(char::is_control)
        {
            return Err(ConfigError::Credentials);
        }
        Ok(Self { user, password })
    }
    pub(crate) fn user(&self) -> &str {
        &self.user
    }
    pub(crate) fn password(&self) -> &str {
        &self.password
    }
}

#[derive(Clone)]
pub struct Config {
    pub key: Key,
    pub probe_url: String,
    pub check_interval: Duration,
    pub retry_interval: Duration,
    pub relogin_hour: Option<u8>,
    pub wifi_only: Option<String>,
    pub paused: bool,
    pub desktop_policy: bool,
    pub portal_host: String,
    pub bras: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            key: Key::parse("5a3b9f207411a8ed").expect("default key"),
            probe_url: DEFAULT_PROBE.into(),
            check_interval: Duration::from_secs(30),
            retry_interval: Duration::from_secs(10),
            relogin_hour: Some(4),
            wifi_only: None,
            paused: false,
            desktop_policy: false,
            portal_host: "http://192.168.199.200".into(),
            bras: "mlyx".into(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigError> {
        let portal = url::Url::parse(&self.portal_host).map_err(|_| ConfigError::Probe)?;
        if portal.scheme() != "http"
            || portal.host_str().is_none()
            || !portal.username().is_empty()
            || portal.password().is_some()
            || portal.query().is_some()
            || portal.fragment().is_some()
            || !matches!(portal.path(), "" | "/")
            || self.bras.is_empty()
            || self.bras.len() > 64
            || !self
                .bras
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(ConfigError::Probe);
        }
        let url = url::Url::parse(&self.probe_url).map_err(|_| ConfigError::Probe)?;
        if self.probe_url.len() > 4096
            || url.scheme() != "http"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || self.probe_url.chars().any(char::is_whitespace)
        {
            return Err(ConfigError::Probe);
        }
        if !(5..=600).contains(&self.check_interval.as_secs())
            || !(3..=300).contains(&self.retry_interval.as_secs())
            || self.check_interval.subsec_nanos() != 0
            || self.retry_interval.subsec_nanos() != 0
            || self.relogin_hour.is_some_and(|hour| hour > 23)
        {
            return Err(ConfigError::Interval);
        }
        if self
            .wifi_only
            .as_ref()
            .is_some_and(|ssid| ssid.is_empty() || ssid.len() > 32)
        {
            return Err(ConfigError::Ssid);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid account or password")]
    Credentials,
    #[error("invalid HTTP probe URL")]
    Probe,
    #[error("invalid schedule")]
    Interval,
    #[error("invalid target Wi-Fi name")]
    Ssid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    /// Must change for interface, route, SSID or wake changes, even if IP repeats.
    pub generation: u64,
    pub ipv4: Option<Ipv4Addr>,
    pub route_ready: bool,
    pub is_wifi: bool,
    pub ssid: Option<String>,
}

impl Network {
    pub fn wifi_matches(&self, required: Option<&str>) -> bool {
        required.is_none_or(|ssid| self.is_wifi && self.ssid.as_deref() == Some(ssid))
    }
}
