// Fixed signing keys below are public synthetic test fixtures, never release keys.
use ed25519_dalek::{Signer, SigningKey};
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
    let key = SigningKey::from_bytes(&[7; 32]);
    verify_manifest(
        raw,
        &key.sign(raw).to_bytes(),
        &key.verifying_key().to_bytes(),
        current,
        platform,
        Architecture::X86_64,
    )
}
#[test]
fn signed_manifest_and_payload_verify() {
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
fn tampering_and_wrong_public_key_are_rejected_before_parsing() {
    let raw = serde_json::to_vec(&manifest()).unwrap();
    let key = SigningKey::from_bytes(&[7; 32]);
    let sig = key.sign(&raw).to_bytes();
    let wrong = SigningKey::from_bytes(&[8; 32]);
    assert!(matches!(
        verify_manifest(
            &raw,
            &sig,
            &wrong.verifying_key().to_bytes(),
            "0.2.5",
            Platform::Windows,
            Architecture::X86_64
        ),
        Err(Error::Signature)
    ));
    let mut edited = raw.clone();
    edited[0] = b'[';
    assert!(matches!(
        verify_manifest(
            &edited,
            &sig,
            &key.verifying_key().to_bytes(),
            "0.2.5",
            Platform::Windows,
            Architecture::X86_64
        ),
        Err(Error::Signature)
    ));
}
#[test]
fn signed_foreign_repository_and_url_are_rejected() {
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
