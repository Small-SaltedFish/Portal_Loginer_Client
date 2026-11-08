use super::{Error, Result};
use portal_core::{
    config::{Config, Credentials},
    crypto::Key,
    protocol::SessionCache,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub user: String,
    pub wifi_only: Option<String>,
    pub success_notifications: bool,
    pub automatic_updates: bool,
    pub check_seconds: u64,
    pub retry_seconds: u64,
    pub relogin_hour: i8,
    pub probe_url: String,
    pub paused: bool,
    pub autostart: bool,
    pub interface_index: Option<u32>,
    pub force_boot_login: bool,
    pub debug_log: bool,
    pub log_max_bytes: u64,
    pub network_error_notifications: bool,
    pub logout_host: String,
    pub logout_bras: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            user: String::new(),
            wifi_only: None,
            success_notifications: true,
            automatic_updates: true,
            check_seconds: 30,
            retry_seconds: 10,
            relogin_hour: 4,
            probe_url: portal_core::config::DEFAULT_PROBE.into(),
            paused: false,
            autostart: true,
            interface_index: None,
            force_boot_login: true,
            debug_log: false,
            log_max_bytes: 524288,
            network_error_notifications: true,
            logout_host: "http://192.168.199.200".into(),
            logout_bras: "mlyx".into(),
        }
    }
}
impl Settings {
    pub fn core(&self, key: &str) -> Result<Config> {
        if !(65536..=16777216).contains(&self.log_max_bytes)
            || self.user.len() > 128
            || self.user.chars().any(char::is_control)
            || !(-1..=23).contains(&self.relogin_hour)
        {
            return Err(Error::Invalid);
        }
        let config = Config {
            key: Key::parse(key).map_err(|_| Error::Invalid)?,
            probe_url: self.probe_url.clone(),
            check_interval: Duration::from_secs(self.check_seconds),
            retry_interval: Duration::from_secs(self.retry_seconds),
            relogin_hour: None,
            wifi_only: self.wifi_only.clone(),
            paused: self.paused,
            desktop_policy: true,
            portal_host: self.logout_host.clone(),
            bras: self.logout_bras.clone(),
        };
        config.validate().map_err(|_| Error::Invalid)?;
        Ok(config)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stored {
    pub schema: u32,
    pub owner_sid: String,
    pub settings: Settings,
    pub credential_file: Option<String>,
    pub key: String,
    pub cache: Option<SessionCache>,
    #[serde(default)]
    pub credentials_verified: bool,
    #[serde(default)]
    pub last_boot: Option<String>,
    #[serde(default)]
    pub paused_boot: Option<String>,
}
impl Stored {
    pub fn new(owner_sid: String) -> Self {
        Self {
            schema: 1,
            owner_sid,
            settings: Settings::default(),
            credential_file: None,
            key: "5a3b9f207411a8ed".into(),
            cache: None,
            credentials_verified: false,
            last_boot: None,
            paused_boot: None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(Error::Invalid);
        }
        super::security::validate_sid(&self.owner_sid)?;
        self.settings.core(&self.key)?;
        if let Some(file) = &self.credential_file {
            if file.len() != 47
                || !file.starts_with("credential-")
                || !file.ends_with(".bin")
                || !file[11..43].bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct PasswordInput {
    pub password: String,
}

pub fn credentials(user: &str, password: &[u8]) -> Result<Credentials> {
    let pass = std::str::from_utf8(password)
        .map_err(|_| Error::Credentials)?
        .to_owned();
    Credentials::new(user.to_owned(), pass).map_err(|_| Error::Credentials)
}
