//! Signed update verification and bounded GitHub/mirror downloads.
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, io::Read};
pub mod download;
pub mod payload;
pub const UPDATE_PUBLIC_KEY: &[u8; 32] = include_bytes!("../../../assets/update-public-key.bin");
pub const REPOSITORY: &str = "Small-SaltedFish/Portal_Loginer_Client";
pub const RELEASES_URL: &str = "https://github.com/Small-SaltedFish/Portal_Loginer_Client/releases";
pub const MAX_MANIFEST_BYTES: usize = 65536;
pub const MAX_PAYLOAD_BYTES: u64 = 128 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Macos,
}
impl Platform {
    fn name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Architecture {
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "aarch64")]
    Aarch64,
}
impl Architecture {
    fn name(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub repository: String,
    pub version: String,
    pub assets: Vec<Asset>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub platform: Platform,
    pub architecture: Architecture,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("manifest or signature exceeds limits")]
    Limit,
    #[error("update signature verification failed")]
    Signature,
    #[error("invalid update manifest")]
    Manifest,
    #[error("update origin is not this project")]
    Origin,
    #[error("invalid or unsupported version")]
    Version,
    #[error("automatic downgrade is forbidden")]
    Downgrade,
    #[error("release has no matching platform asset")]
    UnsupportedTarget,
    #[error("payload size or digest mismatch")]
    Integrity,
    #[error("cannot read update payload")]
    Read,
}
/// Only constructed after signature, origin, version and target validation.
#[derive(Debug)]
pub struct VerifiedUpdate {
    version: semver::Version,
    asset: Asset,
    digest: [u8; 32],
}
impl VerifiedUpdate {
    pub fn version(&self) -> &semver::Version {
        &self.version
    }
    pub fn asset(&self) -> &Asset {
        &self.asset
    }
    pub fn verify_payload(&self, mut reader: impl Read) -> Result<(), Error> {
        let mut hash = Sha256::new();
        let mut count = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let read = reader.read(&mut buffer).map_err(|_| Error::Read)?;
            if read == 0 {
                break;
            }
            count = count.checked_add(read as u64).ok_or(Error::Integrity)?;
            if count > self.asset.size {
                return Err(Error::Integrity);
            }
            hash.update(&buffer[..read]);
        }
        let actual: [u8; 32] = hash.finalize().into();
        if count != self.asset.size || actual != self.digest {
            return Err(Error::Integrity);
        }
        Ok(())
    }
}
fn digest(value: &str) -> Result<[u8; 32], Error> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Manifest);
    }
    let mut result = [0u8; 32];
    for (index, item) in result.iter_mut().enumerate() {
        *item = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| Error::Manifest)?;
    }
    Ok(result)
}
fn version(value: &str) -> Result<semver::Version, Error> {
    let v = semver::Version::parse(value).map_err(|_| Error::Version)?;
    if !v.pre.is_empty() || !v.build.is_empty() || v.to_string() != value {
        return Err(Error::Version);
    }
    Ok(v)
}
pub fn verify_manifest(
    raw: &[u8],
    signature: &[u8],
    public_key: &[u8; 32],
    current: &str,
    platform: Platform,
    architecture: Architecture,
) -> Result<Option<VerifiedUpdate>, Error> {
    if raw.is_empty() || raw.len() > MAX_MANIFEST_BYTES || signature.len() != 64 {
        return Err(Error::Limit);
    }
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(signature).map_err(|_| Error::Signature)?;
    key.verify_strict(raw, &sig).map_err(|_| Error::Signature)?;
    // Authenticate the exact published UTF-8 bytes before parsing JSON.
    let manifest: Manifest = serde_json::from_slice(raw).map_err(|_| Error::Manifest)?;
    if manifest.schema != 1 || manifest.assets.is_empty() || manifest.assets.len() > 6 {
        return Err(Error::Manifest);
    }
    if manifest.repository != REPOSITORY {
        return Err(Error::Origin);
    }
    let release = version(&manifest.version)?;
    let current = version(current)?;
    let mut targets = HashSet::new();
    let mut selected = None;
    for asset in manifest.assets {
        if !targets.insert((asset.platform, asset.architecture))
            || asset.size == 0
            || asset.size > MAX_PAYLOAD_BYTES
        {
            return Err(Error::Manifest);
        }
        let expected = format!(
            "PortalLoginer-{release}-{}-{}-update.zip",
            asset.platform.name(),
            asset.architecture.name()
        );
        let url = url::Url::parse(&asset.url).map_err(|_| Error::Origin)?;
        let path = format!("/{REPOSITORY}/releases/download/v{release}/{expected}");
        if asset.name != expected
            || url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != path
        {
            return Err(Error::Origin);
        }
        let digest = digest(&asset.sha256)?;
        if asset.platform == platform && asset.architecture == architecture {
            selected = Some(VerifiedUpdate {
                version: release.clone(),
                asset,
                digest,
            });
        }
    }
    if release < current {
        return Err(Error::Downgrade);
    }
    if release == current {
        return Ok(None);
    }
    selected.map(Some).ok_or(Error::UnsupportedTarget)
}
