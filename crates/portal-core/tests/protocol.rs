use portal_core::{
    crypto::{encrypt_with_prefix, Key},
    protocol::*,
};
use serde_json::Value;
use std::net::Ipv4Addr;

#[test]
fn original_portal_fixtures() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../../../fixtures/protocol.json")).unwrap();
    let ip: Ipv4Addr = fixtures["current_ipv4"].as_str().unwrap().parse().unwrap();
    for case in fixtures["portal_urls"].as_array().unwrap() {
        let portal = Portal::parse(case["url"].as_str().unwrap(), ip);
        assert_eq!(portal.is_ok(), case["result"] == 0, "{}", case["id"]);
        if let Ok(portal) = portal {
            assert_eq!(
                portal.bras(),
                case["bras"].as_str().unwrap(),
                "{}",
                case["id"]
            );
            assert_eq!(
                portal.clientip().to_string(),
                case["clientip"].as_str().unwrap()
            );
        }
    }
}

#[test]
fn original_login_decisions() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../../../fixtures/protocol.json")).unwrap();
    for case in fixtures["login_results"].as_array().unwrap() {
        let login = serde_json::to_vec(&serde_json::json!({"ret":case["login_ret"]})).unwrap();
        let ack = serde_json::to_vec(&serde_json::json!({"ret":case["ack_ret"]})).unwrap();
        let probe = if case["probe_result"] == 1 {
            Probe::Online
        } else {
            Probe::Unknown
        };
        let actual = if login_accepted(&login) {
            finish_login(ack_accepted(&ack), &probe)
        } else {
            LoginResult::Failed
        };
        let result = match actual {
            LoginResult::Success => 0,
            LoginResult::Pending => 2,
            LoginResult::Failed => 1,
        };
        assert_eq!(result, case["result"].as_u64().unwrap(), "{}", case["id"]);
    }
    for invalid in [
        b"<html>error</html>".as_slice(),
        b"{}",
        b"{\"ret\":true}",
        b"{\"ret\":[]}",
        b"{\"ret\":0.0}",
    ] {
        assert!(!login_accepted(invalid));
    }
}

#[test]
fn aes_matches_independent_openssl_and_node_oracle() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../../../fixtures/crypto.json")).unwrap();
    let key = Key::parse(fixtures["key"].as_str().unwrap()).unwrap();
    let prefix: [u8; 4] = fixtures["prefix"]
        .as_str()
        .unwrap()
        .as_bytes()
        .try_into()
        .unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let encrypted =
            encrypt_with_prefix(case["password"].as_str().unwrap(), &key, prefix).unwrap();
        assert_eq!(&*encrypted, case["encrypted"].as_str().unwrap());
    }
    assert!(Key::parse("short").is_err());
    assert!(Key::parse("0123456789abcde!").is_err());
    assert!(encrypt_with_prefix("test", &key, *b"!bad").is_err());
}

#[test]
fn key_detection_preserves_baseline_regex_boundaries() {
    assert!(detect_key("var unrelated='0123456789abcdef'").is_none());
    assert!(detect_key("Utf8.parse('too-short')").is_none());
    assert!(detect_key("Utf8.parse\n('0123456789abcdef')").is_none());
    for js in [
        "CryptoJS.enc.Utf8.parse(\"0123456789abcdef\")",
        "<html>Utf8.parse('0123456789abcdef'</html>",
        "Utf8.parse ( \"0123456789abcdef'",
        "Utf8.parse('0123456789abcdef');Utf8.parse('abcdefghijklmnop')",
    ] {
        assert_eq!(detect_key(js).unwrap().expose(), "0123456789abcdef");
    }
}

#[test]
fn redirects_and_request_shapes() {
    let ip = "10.23.45.67".parse().unwrap();
    let raw = "http://192.168.199.200/index.html?bras=hn&clientip=10.23.45.67&wlanuserip=10.23.45.67&wlanacname=hn&clientmac=aa:bb:cc:dd:ee:ff&paip=172.16.100.200&vlan=1407.1051";
    assert!(Portal::parse(
        "http://192.168.199.200/index.html?bras=hn?clientip=10.23.45.67",
        ip
    )
    .is_err());
    for status in [301, 302, 303, 307, 308] {
        assert!(matches!(
            classify_probe(status, Some(raw), ip),
            Probe::Portal(_)
        ));
    }
    for status in [200, 400, 500] {
        assert!(matches!(
            classify_probe(status, Some(raw), ip),
            Probe::Unknown
        ));
    }
    assert!(matches!(
        classify_probe(302, Some("https://example.org/"), ip),
        Probe::Unknown
    ));
    let portal = Portal::parse(raw, ip).unwrap();
    assert_eq!(portal.api_url(Endpoint::Login), "http://192.168.199.200/api/login.php?bras=hn&clientip=10.23.45.67&wlanuserip=10.23.45.67&wlanacname=hn&clientmac=aa:bb:cc:dd:ee:ff&paip=172.16.100.200&vlan=1407.1051");
    assert_eq!(portal.api_url(Endpoint::Logoff), "http://192.168.199.200/api/logoff.php?bras=hn&clientip=10.23.45.67&clientmac=aa-bb-cc-dd-ee-ff&vlan=1407.1051");
    assert_eq!(
        &*form("test+user@net", "a+/=&"),
        "user=test%2Buser%40net&pass=a%2B%2F%3D%26&authmode=0&pool=0&isp_id=0&pyacct=0"
    );
}

#[test]
fn persisted_cache_keeps_subset_and_rejects_stale_or_injected_parameters() {
    let ip = "10.23.45.67".parse().unwrap();
    let portal = Portal::parse("http://192.168.199.200/index.html?bras=hn&clientip=10.23.45.67&wlanuserip=10.9.8.7&wlanacname=original&vlan=1407.1051", ip).unwrap();
    let cache = portal.cache();
    let restored = cache.restore(ip).unwrap();
    assert!(restored.session_url().contains("wlanuserip=10.23.45.67"));
    assert!(!restored.api_url(Endpoint::Login).contains("wlanacname="));
    assert!(cache.restore("10.23.45.68".parse().unwrap()).is_err());
    let mut serialized = serde_json::to_value(&cache).unwrap();
    serialized["bras"] = Value::String("hn&clientip=10.23.45.68".into());
    let tampered: SessionCache = serde_json::from_value(serialized).unwrap();
    assert!(tampered.restore(ip).is_err());
}

#[test]
fn configuration_rejects_invalid_settings_before_use() {
    use portal_core::config::{Config, Credentials};
    use std::time::Duration;
    assert!(Config::default().validate().is_ok());
    for probe in [
        "https://example.org/",
        "http://user:pass@example.org/",
        "http://example.org/#fragment",
        "http://example.org/a b",
    ] {
        assert!(Config {
            probe_url: probe.into(),
            ..Config::default()
        }
        .validate()
        .is_err());
    }
    assert!(Config {
        retry_interval: Duration::from_secs(2),
        ..Config::default()
    }
    .validate()
    .is_err());
    assert!(Config {
        check_interval: Duration::from_millis(5500),
        ..Config::default()
    }
    .validate()
    .is_err());
    assert!(Config {
        wifi_only: Some(String::new()),
        ..Config::default()
    }
    .validate()
    .is_err());
    assert!(Credentials::new("test\nuser".into(), "password".into()).is_err());
    assert!(Credentials::new("user".into(), String::new()).is_err());
}
