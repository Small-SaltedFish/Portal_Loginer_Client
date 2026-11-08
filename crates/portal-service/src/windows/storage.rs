use super::{security, settings::Stored, Error, Result};
use rand::{rngs::OsRng, RngCore};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, FILE_ATTRIBUTE_REPARSE_POINT, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};
use zeroize::Zeroizing;

pub struct Store {
    pub directory: PathBuf,
}

#[cfg(test)]
pub(super) struct TestStore {
    pub store: Store,
    root: PathBuf,
}
#[cfg(test)]
impl TestStore {
    pub fn new() -> Self {
        let root = std::env::current_dir()
            .unwrap()
            .join("../../target")
            .canonicalize()
            .unwrap();
        let directory = root.join(format!("service-test-{}", random_id().unwrap()));
        fs::create_dir(&directory).unwrap();
        Self {
            store: Store { directory },
            root,
        }
    }
}
#[cfg(test)]
impl Drop for TestStore {
    fn drop(&mut self) {
        if !self.store.directory.exists() {
            return;
        }
        let resolved = self.store.directory.canonicalize().unwrap();
        assert_eq!(resolved.parent(), Some(self.root.as_path()));
        assert!(resolved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("service-test-"));
        let _ = fs::remove_dir_all(resolved);
    }
}
impl Store {
    pub fn system() -> Result<Self> {
        Ok(Self {
            directory: security::data_directory()?,
        })
    }
    pub fn initialize(&self, owner: String) -> Result<()> {
        security::secure_directory(&self.directory)?;
        if self.directory.join("settings.json").exists() {
            self.load()?;
            return Ok(());
        }
        self.save(&Stored::new(owner))
    }
    pub fn load(&self) -> Result<Stored> {
        let data = read_limited(&self.directory.join("settings.json"), 64 * 1024)?;
        let mut config: Stored = serde_json::from_slice(&data).map_err(|_| Error::Invalid)?;
        if config.settings.logout_host == "http://192.168.199.200"
            && config.settings.logout_bras == "hn"
        {
            config.settings.logout_bras = "mlyx".into();
        }
        config.validate()?;
        Ok(config)
    }
    pub fn save(&self, config: &Stored) -> Result<()> {
        config.validate()?;
        let bytes = Zeroizing::new(serde_json::to_vec_pretty(config).map_err(|_| Error::Invalid)?);
        atomic_write(&self.directory, "settings.json", &bytes)
    }
    pub fn password(&self, config: &Stored) -> Result<Option<Zeroizing<Vec<u8>>>> {
        config.validate()?;
        config
            .credential_file
            .as_ref()
            .map(|name| {
                let encrypted = read_limited(&self.directory.join(name), 16 * 1024)?;
                security::unprotect(&encrypted)
            })
            .transpose()
    }
    /// Write new immutable ciphertext, then atomically switch the config's
    /// reference. A crash leaves an orphan encrypted file, never mixed accounts.
    pub fn change_password(&self, config: &mut Stored, password: &str) -> Result<()> {
        if password.is_empty() {
            return self.save(config);
        }
        let _ = super::settings::credentials(&config.settings.user, password.as_bytes())?;
        let encrypted = security::protect(password.as_bytes())?;
        let name = format!("credential-{}.bin", random_id()?);
        atomic_write(&self.directory, &name, &encrypted)?;
        let previous = config.credential_file.replace(name.clone());
        if let Err(error) = self.save(config) {
            config.credential_file = previous;
            let _ = fs::remove_file(self.directory.join(name));
            return Err(error);
        }
        // No secrets are returned to the client; old ciphertext is obsolete.
        if let Some(previous) = previous {
            let _ = fs::remove_file(self.directory.join(previous));
        }
        Ok(())
    }
}
fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| Error::Credentials)?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(32);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(output)
}
pub fn read_limited(path: &Path, limit: usize) -> Result<Zeroizing<Vec<u8>>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || !metadata.is_file()
        || metadata.len() > limit as u64
    {
        return Err(Error::Access);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    File::open(path)?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::Invalid);
    }
    Ok(bytes)
}
pub fn atomic_write(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    if name.contains(['/', '\\', ':']) || name == "." || name == ".." {
        return Err(Error::Invalid);
    }
    let temporary = directory.join(format!(".pending-{}", random_id()?));
    let target = directory.join(name);
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(not(test))]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.access_mode(
                windows_sys::Win32::Foundation::GENERIC_WRITE
                    | windows_sys::Win32::Storage::FileSystem::WRITE_OWNER,
            );
        }
        let mut file = options.open(&temporary)?;
        // Installer writes must not leave an ordinary user's owner SID able to
        // grant write access to the privileged executable or service config.
        #[cfg(not(test))]
        {
            use std::os::windows::io::AsRawHandle;
            security::assign_admin_owner(file.as_raw_handle())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if unsafe {
            MoveFileExW(
                security::wide(&temporary).as_ptr(),
                security::wide(&target).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_config_and_password_keep_or_replace() {
        let root = std::env::current_dir()
            .unwrap()
            .join("../../target")
            .canonicalize()
            .unwrap();
        let directory = root.join(format!("store-test-{}", random_id().unwrap()));
        fs::create_dir_all(&directory).unwrap();
        let store = Store {
            directory: directory.clone(),
        };
        let mut config = Stored::new(security::current_sid().unwrap());
        config.settings.user = "synthetic-user".into();
        store.save(&config).unwrap();
        store
            .change_password(&mut config, "synthetic-password")
            .unwrap();
        let before = config.credential_file.clone();
        store.change_password(&mut config, "").unwrap();
        assert_eq!(config.credential_file, before);
        assert_eq!(
            &*store.password(&store.load().unwrap()).unwrap().unwrap(),
            b"synthetic-password"
        );
        store
            .change_password(&mut config, "new-synthetic-password")
            .unwrap();
        assert_ne!(config.credential_file, before);
        assert!(!directory.join(before.unwrap()).exists());
        assert!(!String::from_utf8(
            read_limited(&directory.join("settings.json"), 65536)
                .unwrap()
                .to_vec()
        )
        .unwrap()
        .contains("synthetic-password"));
        let resolved = directory.canonicalize().unwrap();
        assert_eq!(resolved.parent(), Some(root.as_path()));
        assert!(resolved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("store-test-"));
        fs::remove_dir_all(resolved).unwrap();
    }
    #[test]
    fn invalid_credential_reference_cannot_escape_store() {
        let mut config = Stored::new(security::current_sid().unwrap());
        for value in [
            "../secret",
            "credential-../../x.bin",
            "credential-00000000000000000000000000000000.bin",
        ] {
            config.credential_file = Some(value.into());
            assert_eq!(config.validate().is_ok(), value.len() == 47);
        }
    }
    #[test]
    fn failed_config_switch_rolls_back_credential_reference() {
        let fixture = TestStore::new();
        fs::create_dir(fixture.store.directory.join("settings.json")).unwrap();
        let mut config = Stored::new(security::current_sid().unwrap());
        config.settings.user = "synthetic-user".into();
        assert!(fixture
            .store
            .change_password(&mut config, "synthetic-password")
            .is_err());
        assert!(config.credential_file.is_none());
        assert_eq!(fs::read_dir(&fixture.store.directory).unwrap().count(), 1);
    }
}
