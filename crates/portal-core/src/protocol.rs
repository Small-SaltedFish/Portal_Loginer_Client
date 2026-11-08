use crate::crypto::Key;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{net::Ipv4Addr, sync::LazyLock};
use zeroize::Zeroizing;

#[derive(Clone, PartialEq, Eq)]
pub struct Portal {
    host: String,
    bras: String,
    clientip: Ipv4Addr,
    wlanuserip: String,
    wlanacname: String,
    clientmac: String,
    paip: String,
    vlan: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid or stale campus portal parameters")]
pub struct InvalidPortal;

/// Ordinary restricted service config, never a public status response.
/// Mirrors the upstream last_* subset, not the full Portal redirect.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCache {
    host: String,
    bras: String,
    clientip: Ipv4Addr,
    clientmac: String,
    paip: String,
    vlan: String,
}

impl SessionCache {
    pub fn restore(&self, current: Ipv4Addr) -> Result<Portal, InvalidPortal> {
        if self.clientip != current
            || [&self.bras, &self.clientmac, &self.paip, &self.vlan]
                .iter()
                .any(|value| {
                    value.len() > 8192
                        || !value
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._:%-".contains(&b))
                })
        {
            return Err(InvalidPortal);
        }
        let portal = Portal::parse(
            &format!(
                "{}/index.html?bras={}&clientip={}&wlanuserip={}&clientmac={}&paip={}&vlan={}",
                self.host,
                self.bras,
                self.clientip,
                self.clientip,
                self.clientmac,
                self.paip,
                self.vlan
            ),
            current,
        )?;
        if portal.host != self.host {
            return Err(InvalidPortal);
        }
        Ok(portal)
    }
}

impl Portal {
    pub fn parse(raw: &str, current: Ipv4Addr) -> Result<Self, InvalidPortal> {
        if raw.len() > 8192 {
            return Err(InvalidPortal);
        }
        // Preserve the shell's raw, non-decoded query behavior; validate authority
        // before Url can normalize unusual IP spellings or user information.
        let tail = raw
            .strip_prefix("http://")
            .or_else(|| raw.strip_prefix("https://"))
            .ok_or(InvalidPortal)?;
        let authority = tail.split(['/', '?']).next().ok_or(InvalidPortal)?;
        if !authority
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".:-".contains(&b))
        {
            return Err(InvalidPortal);
        }
        let mut parts = authority.split(':');
        let address: Ipv4Addr = parts
            .next()
            .ok_or(InvalidPortal)?
            .parse()
            .map_err(|_| InvalidPortal)?;
        if !address.is_private() {
            return Err(InvalidPortal);
        }
        if let Some(port) = parts.next() {
            if port.starts_with('0') || port.parse::<u16>().ok().filter(|p| *p != 0).is_none() {
                return Err(InvalidPortal);
            }
        }
        if parts.next().is_some() {
            return Err(InvalidPortal);
        }
        let value = |name: &str| -> String {
            let marker = format!("{name}=");
            raw.match_indices(['?', '&'])
                .find_map(|(index, _)| {
                    let val = raw[index + 1..].strip_prefix(&marker)?.split('&').next()?;
                    (!val.is_empty()).then(|| val.split('=').next().unwrap_or("").to_owned())
                })
                .unwrap_or_default()
        };
        let mut bras = value("bras");
        let wlanacname = value("wlanacname");
        if bras.is_empty() {
            bras.clone_from(&wlanacname);
        }
        let wlanuserip = value("wlanuserip");
        let mut clientip = value("clientip");
        if clientip.is_empty() {
            clientip.clone_from(&wlanuserip);
        }
        let clientip: Ipv4Addr = clientip.parse().map_err(|_| InvalidPortal)?;
        let portal = Self {
            host: format!(
                "{}://{authority}",
                if raw.starts_with("https:") {
                    "https"
                } else {
                    "http"
                }
            ),
            bras,
            clientip,
            wlanuserip,
            wlanacname,
            clientmac: value("clientmac"),
            paip: value("paip"),
            vlan: value("vlan"),
        };
        if portal.bras.is_empty()
            || portal.clientip != current
            || [
                &portal.bras,
                &portal.wlanuserip,
                &portal.wlanacname,
                &portal.clientmac,
                &portal.paip,
                &portal.vlan,
            ]
            .into_iter()
            .any(|v| {
                !v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:%-".contains(&b))
            })
        {
            return Err(InvalidPortal);
        }
        Ok(portal)
    }

    pub fn bras(&self) -> &str {
        &self.bras
    }
    pub fn clientip(&self) -> Ipv4Addr {
        self.clientip
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn cache(&self) -> SessionCache {
        SessionCache {
            host: self.host.clone(),
            bras: self.bras.clone(),
            clientip: self.clientip,
            clientmac: self.clientmac.clone(),
            paip: self.paip.clone(),
            vlan: self.vlan.clone(),
        }
    }
    pub fn referer(&self) -> String {
        format!("{}/index.html?bras={}", self.host, self.bras)
    }
    pub fn session_url(&self) -> String {
        format!(
            "{}/index.html?bras={}&clientip={}&wlanuserip={}&clientmac={}",
            self.host, self.bras, self.clientip, self.wlanuserip, self.clientmac
        )
    }
    pub fn api_url(&self, endpoint: Endpoint) -> String {
        let mut url = format!(
            "{}/api/{}.php?bras={}&clientip={}",
            self.host,
            endpoint.path(),
            self.bras,
            self.clientip
        );
        let ip_mac = self.clientmac.replace(':', "-");
        let fields = if endpoint == Endpoint::Logoff {
            vec![("clientmac", &ip_mac), ("vlan", &self.vlan)]
        } else {
            vec![
                ("wlanuserip", &self.wlanuserip),
                ("wlanacname", &self.wlanacname),
                ("clientmac", &self.clientmac),
                ("paip", &self.paip),
                ("vlan", &self.vlan),
            ]
        };
        for (name, val) in fields {
            if !val.is_empty() {
                url.push_str(&format!("&{name}={val}"));
            }
        }
        url
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Login,
    Ack,
    Logoff,
}
impl Endpoint {
    fn path(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Ack => "ack_auth",
            Self::Logoff => "logoff",
        }
    }
}

pub fn form(user: &str, encrypted: &str) -> Zeroizing<String> {
    Zeroizing::new(
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("user", user)
            .append_pair("pass", encrypted)
            .append_pair("authmode", "0")
            .append_pair("pool", "0")
            .append_pair("isp_id", "0")
            .append_pair("pyacct", "0")
            .finish(),
    )
}

pub fn response_ret(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    match value.get("ret")? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}
pub fn login_accepted(body: &[u8]) -> bool {
    matches!(
        response_ret(body).as_deref(),
        Some("0" | "121" | "122" | "3")
    )
}
pub fn ack_accepted(body: &[u8]) -> bool {
    response_ret(body).as_deref() == Some("0")
}

pub const KEY_PATHS: [&str; 3] = [
    "/assets/js/raas.js",
    "/js/raas.js",
    "/tp/default/js/index.js",
];
pub fn detect_key(content: &str) -> Option<Key> {
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"Utf8[.]parse[ \t\r\x0b\x0c]*\([ \t\r\x0b\x0c]*['"]([A-Za-z0-9]{16})['"]"#)
            .expect("constant regex")
    });
    Key::parse(PATTERN.captures(content)?.get(1)?.as_str()).ok()
}

#[derive(Clone, PartialEq, Eq)]
pub enum Probe {
    Portal(Portal),
    Online,
    NoAddress,
    NoRoute,
    Unknown,
}

pub fn classify_probe(status: u16, location: Option<&str>, current: Ipv4Addr) -> Probe {
    if status == 204 {
        return Probe::Online;
    }
    if matches!(status, 301 | 302 | 303 | 307 | 308) {
        if let Some(url) = location {
            if ["bras=", "wlanuserip=", "clientmac=", "wlanacname="]
                .iter()
                .any(|v| url.contains(v))
            {
                if let Ok(portal) = Portal::parse(url, current) {
                    return Probe::Portal(portal);
                }
            }
        }
    }
    Probe::Unknown
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginResult {
    Success,
    Pending,
    Failed,
}
pub fn finish_login(ack_ok: bool, probe: &Probe) -> LoginResult {
    if *probe == Probe::Online {
        LoginResult::Success
    } else if ack_ok {
        LoginResult::Pending
    } else {
        LoginResult::Failed
    }
}
