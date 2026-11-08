use portal_update::*;
use sha2::{Digest, Sha256};
fn manifest() -> Manifest {
    let hash = Sha256::digest(b"synthetic-payload");
    Manifest{schema:1,repository:REPOSITORY.into(),version:"0.3.0".into(),assets:vec![Asset{platform:Platform::Windows,architecture:Architecture::X86_64,name:"PortalLoginer-0.3.0-windows-x86_64-update.zip".into(),url:format!("https://github.com/{REPOSITORY}/releases/download/v0.3.0/PortalLoginer-0.3.0-windows-x86_64-update.zip"),size:17,sha256:hash.iter().fold(String::new(),|mut out,b|{use std::fmt::Write;write!(&mut out,"{b:02x}").unwrap();out})}]}
}
fn verify(value: &Manifest) -> Result<Option<VerifiedUpdate>, Error> {
    verify_raw(
        &serde_json::to_vec(value).unwrap(),
        "0.2.5",
        Platform::Windows,
    )
}
fn verify_raw(
    raw: &[u8],
    current: &str,
    platform: Platform,
) -> Result<Option<VerifiedUpdate>, Error> {
    verify_manifest(raw, current, platform, Architecture::X86_64)
}
#[test]
fn manifest_and_payload_verify() {
    let update = verify(&manifest()).unwrap().unwrap();
    assert_eq!(update.version().to_string(), "0.3.0");
    assert!(update.verify_payload(&b"synthetic-payload"[..]).is_ok());
    assert!(update.verify_payload(&b"synthetic-payloae"[..]).is_err());
    assert!(update.verify_payload(&b"short"[..]).is_err());
    assert!(update
        .verify_payload(&b"synthetic-payload-extra"[..])
        .is_err());
}
#[test]
fn github_metadata_requires_stable_release_and_sha256() {
    let m = manifest();
    let a = &m.assets[0];
    let mut raw = serde_json::json!({"tag_name":"v0.3.0", "draft":false, "prerelease":false,
      "assets":[{"name":a.name,"browser_download_url":a.url,"size":a.size,"digest":format!("sha256:{}",a.sha256)}]});
    assert!(github_manifest(&serde_json::to_vec(&raw).unwrap()).is_ok());
    raw["assets"][0]["digest"] = serde_json::Value::Null;
    assert!(github_manifest(&serde_json::to_vec(&raw).unwrap()).is_err());
    raw["assets"][0]["digest"] = serde_json::json!(format!("sha256:{}", a.sha256));
    raw["prerelease"] = serde_json::json!(true);
    assert!(github_manifest(&serde_json::to_vec(&raw).unwrap()).is_err());
    raw["prerelease"] = serde_json::json!(false);
    raw["assets"][0]["browser_download_url"] = serde_json::json!("https://evil.example/update.zip");
    assert!(github_manifest(&serde_json::to_vec(&raw).unwrap()).is_err());
}
#[test]
fn foreign_repository_and_url_are_rejected() {
    let mut v = manifest();
    v.repository = "someone/other".into();
    assert!(matches!(verify(&v), Err(Error::Origin)));
    v = manifest();
    v.assets[0].url = v.assets[0].url.replace("github.com/", "evil.example/");
    assert!(matches!(verify(&v), Err(Error::Origin)));
    v = manifest();
    v.assets[0].name = "../outside.exe".into();
    assert!(matches!(verify(&v), Err(Error::Origin)));
}
#[test]
fn downgrade_equal_version_and_wrong_platform() {
    let bytes = serde_json::to_vec(&manifest()).unwrap();
    assert!(matches!(
        verify_raw(&bytes, "0.4.0", Platform::Windows),
        Err(Error::Downgrade)
    ));
    assert!(verify_raw(&bytes, "0.3.0", Platform::Windows)
        .unwrap()
        .is_none());
    assert!(matches!(
        verify_raw(&bytes, "0.2.5", Platform::Macos),
        Err(Error::UnsupportedTarget)
    ));
}
#[test]
fn duplicate_targets_and_unknown_fields_are_rejected() {
    let mut v = manifest();
    v.assets.push(v.assets[0].clone());
    assert!(matches!(verify(&v), Err(Error::Manifest)));
    let mut json = serde_json::to_value(manifest()).unwrap();
    json["command"] = serde_json::json!("run arbitrary commands");
    assert!(matches!(
        verify_raw(
            &serde_json::to_vec(&json).unwrap(),
            "0.2.5",
            Platform::Windows
        ),
        Err(Error::Manifest)
    ));
}
#[test]
fn oversized_and_invalid_payload_declarations_are_rejected() {
    let mut v = manifest();
    v.assets[0].size = MAX_PAYLOAD_BYTES + 1;
    assert!(matches!(verify(&v), Err(Error::Manifest)));
    v = manifest();
    v.assets[0].sha256 = "00".into();
    assert!(matches!(verify(&v), Err(Error::Manifest)));
    assert!(matches!(
        verify_raw(
            &vec![b' '; MAX_MANIFEST_BYTES + 1],
            "0.2.5",
            Platform::Windows
        ),
        Err(Error::Limit)
    ));
}

#[test]
fn repository_payload_is_pinned_to_a_commit_in_the_fixed_project() {
    let mut v = manifest();
    let prefix = format!("https://raw.githubusercontent.com/{REPOSITORY}/");
    v.assets[0].url = format!("{prefix}{}/updates/windows-x86_64.zip", "a".repeat(40));
    assert!(verify(&v).is_ok());
    for url in [
        format!("{prefix}main/updates/windows-x86_64.zip"),
        format!(
            "https://raw.githubusercontent.com/other/repo/{}/updates/windows-x86_64.zip",
            "a".repeat(40)
        ),
        format!("{prefix}{}/updates/other.zip", "a".repeat(40)),
    ] {
        v.assets[0].url = url;
        assert!(matches!(verify(&v), Err(Error::Origin)));
    }
}
