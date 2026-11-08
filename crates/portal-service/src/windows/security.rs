use super::{Error, Result};
use std::{
    ffi::c_void,
    io,
    mem::{size_of, zeroed},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, ERROR_ALREADY_EXISTS, HANDLE, INVALID_HANDLE_VALUE},
    Security::{Authorization::*, Cryptography::*, *},
    Storage::FileSystem::*,
    System::{
        Com::CoTaskMemFree,
        SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE, SYSTEM_AUDIT_ACE_TYPE},
        Threading::*,
    },
    UI::Shell::{FOLDERID_ProgramData, FOLDERID_ProgramFiles, SHGetKnownFolderPath},
};
use zeroize::{Zeroize, Zeroizing};

pub fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
pub struct Handle(pub HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub struct Descriptor(*mut c_void);
impl Descriptor {
    pub fn from_sddl(value: &str) -> Result<Self> {
        let mut raw = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(value).as_ptr(),
                1,
                &mut raw,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self(raw))
    }
    pub fn private() -> Result<Self> {
        Self::from_sddl("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")
    }
    pub fn pipe(owner: &str) -> Result<Self> {
        validate_sid(owner)?;
        // No FILE_CREATE_PIPE_INSTANCE for the ordinary owner client.
        Self::from_sddl(&format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;{owner})"
        ))
    }
    pub fn service(owner: &str) -> Result<Self> {
        validate_sid(owner)?;
        // Owner can query/start this fixed service, but cannot change its binary,
        // permissions or recovery command. Shutdown goes through authorized IPC.
        Self::from_sddl(&format!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x14;;;{owner})"))
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}
pub fn set_service_permissions(
    handle: windows_sys::Win32::System::Services::SC_HANDLE,
    owner: &str,
) -> Result<()> {
    let descriptor = Descriptor::service(owner)?;
    if unsafe {
        windows_sys::Win32::System::Services::SetServiceObjectSecurity(
            handle,
            DACL_SECURITY_INFORMATION,
            descriptor.0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub fn validate_sid(value: &str) -> Result<()> {
    if value.len() > 184
        || !value.starts_with("S-1-")
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S')
    {
        return Err(Error::Invalid);
    }
    let mut sid = ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(wide(value).as_ptr(), &mut sid) } == 0 {
        return Err(Error::Invalid);
    }
    unsafe {
        LocalFree(sid);
    }
    Ok(())
}

fn sid_of_token(token: HANDLE) -> Result<String> {
    let mut length = 0;
    unsafe {
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut length);
    }
    if length == 0 || length > 65536 {
        return Err(Error::Access);
    }
    let mut data = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            data.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(Error::Access);
    }
    let user = unsafe { &*data.as_ptr().cast::<TOKEN_USER>() };
    sid_text(user.User.Sid)
}
fn sid_text(sid: PSID) -> Result<String> {
    let mut string = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut string) } == 0 {
        return Err(Error::Access);
    }
    let mut count = 0;
    unsafe {
        while *string.add(count) != 0 {
            count += 1;
        }
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(string, count) })
        .map_err(|_| Error::Access);
    unsafe {
        LocalFree(string.cast());
    }
    value
}
pub fn current_sid() -> Result<String> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(Error::Access);
    }
    let token = Handle(token);
    sid_of_token(token.0)
}
fn token_is_admin(token: HANDLE) -> Result<bool> {
    let mut sid = ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(wide("S-1-5-32-544").as_ptr(), &mut sid) } == 0 {
        return Err(Error::Access);
    }
    let mut member = 0;
    let ok = unsafe { CheckTokenMembership(token, sid, &mut member) };
    unsafe {
        LocalFree(sid);
    }
    if ok == 0 {
        return Err(Error::Access);
    }
    Ok(member != 0)
}
pub fn require_admin() -> Result<()> {
    if token_is_admin(ptr::null_mut())? {
        Ok(())
    } else {
        Err(Error::Access)
    }
}

pub fn authorize_pipe(pipe: HANDLE, owner: &str) -> Result<()> {
    use windows_sys::Win32::System::Pipes::ImpersonateNamedPipeClient;
    if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
        return Err(Error::Access);
    }
    struct Revert;
    impl Drop for Revert {
        fn drop(&mut self) {
            // Never return an impersonated thread to Tokio's shared executor.
            if unsafe { RevertToSelf() } == 0 {
                std::process::abort();
            }
        }
    }
    let _revert = Revert;
    let mut token = ptr::null_mut();
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
        return Err(Error::Access);
    }
    let token = Handle(token);
    let sid = sid_of_token(token.0)?;
    if sid == owner || sid == "S-1-5-18" || token_is_admin(token.0)? {
        Ok(())
    } else {
        Err(Error::Access)
    }
}

fn known_folder(id: &windows_sys::core::GUID) -> Result<PathBuf> {
    let mut value = ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(id, 0, ptr::null_mut(), &mut value) } < 0 {
        return Err(Error::Access);
    }
    let mut length = 0;
    unsafe {
        while *value.add(length) != 0 {
            length += 1;
        }
    }
    let path = PathBuf::from(
        String::from_utf16(unsafe { std::slice::from_raw_parts(value, length) })
            .map_err(|_| Error::Invalid)?,
    );
    unsafe {
        CoTaskMemFree(value.cast());
    }
    Ok(path)
}
pub fn data_directory() -> Result<PathBuf> {
    Ok(known_folder(&FOLDERID_ProgramData)?.join("PortalLoginer"))
}
pub fn boot_id() -> Result<String> {
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQuerySystemInformation(
            class: u32,
            buffer: *mut c_void,
            length: u32,
            returned: *mut u32,
        ) -> i32;
    }
    let mut buffer = [0u64; 4];
    let mut returned = 0;
    let status =
        unsafe { NtQuerySystemInformation(90, buffer.as_mut_ptr().cast(), 32, &mut returned) };
    if status < 0 {
        return Err(Error::Invalid);
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), 16) };
    const HEX: &[u8; 16] = b"0123456789abcdef";
    Ok(bytes
        .iter()
        .flat_map(|b| {
            [
                HEX[(b >> 4) as usize] as char,
                HEX[(b & 15) as usize] as char,
            ]
        })
        .collect())
}
pub fn install_directory() -> Result<PathBuf> {
    Ok(known_folder(&FOLDERID_ProgramFiles)?.join("PortalLoginer"))
}

pub fn secure_directory(path: &Path) -> Result<()> {
    secure_directory_with(path, false)
}
pub fn secure_install_directory(path: &Path) -> Result<()> {
    secure_directory_with(path, true)
}
#[cfg(not(test))]
pub fn assign_admin_owner(handle: HANDLE) -> Result<()> {
    let mut admins = ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(wide("S-1-5-32-544").as_ptr(), &mut admins) } == 0 {
        return Err(Error::Access);
    }
    let code = unsafe {
        SetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            admins,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
        )
    };
    unsafe {
        LocalFree(admins);
    }
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32).into());
    }
    Ok(())
}
fn secure_directory_with(path: &Path, executable: bool) -> Result<()> {
    let descriptor = if executable {
        Descriptor::from_sddl("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;BU)")?
    } else {
        Descriptor::private()?
    };
    let attrs = descriptor.attributes();
    let path_wide = wide(path);
    let created = unsafe { CreateDirectoryW(path_wide.as_ptr(), &attrs) } != 0;
    if !created && io::Error::last_os_error().raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
        return Err(io::Error::last_os_error().into());
    }
    let raw = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            READ_CONTROL | WRITE_DAC | WRITE_OWNER,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error().into());
    }
    let handle = Handle(raw);
    let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(Error::Access);
    }
    if !created {
        let mut owner = ptr::null_mut();
        let mut existing_descriptor = ptr::null_mut();
        let code = unsafe {
            GetSecurityInfo(
                handle.0,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut existing_descriptor,
            )
        };
        if code != 0 {
            return Err(Error::Access);
        }
        let existing_owner = sid_text(owner);
        unsafe {
            LocalFree(existing_descriptor);
        }
        if !matches!(existing_owner?.as_str(), "S-1-5-18" | "S-1-5-32-544") {
            return Err(Error::Access);
        }
    }
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = ptr::null_mut();
    if unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut acl, &mut defaulted) }
        == 0
        || present == 0
    {
        return Err(Error::Access);
    }
    let mut admins = ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(wide("S-1-5-32-544").as_ptr(), &mut admins) } == 0 {
        return Err(Error::Access);
    }
    let code = unsafe {
        SetSecurityInfo(
            handle.0,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            admins,
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    unsafe {
        LocalFree(admins);
    }
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32).into());
    }
    Ok(())
}

pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
    crypt(plain, true).map(|bytes| bytes.to_vec())
}
pub fn unprotect(encrypted: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    crypt(encrypted, false)
}
fn crypt(input: &[u8], encrypt: bool) -> Result<Zeroizing<Vec<u8>>> {
    if input.len() > 16384 {
        return Err(Error::Credentials);
    }
    let data = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr().cast_mut(),
    };
    let entropy = b"PortalLoginer.credentials.v1";
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_ptr().cast_mut(),
    };
    let mut output = unsafe { zeroed::<CRYPT_INTEGER_BLOB>() };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &data,
                ptr::null(),
                &entropy,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &data,
                ptr::null_mut(),
                &entropy,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(Error::Credentials);
    }
    let native = unsafe { std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize) };
    let result = Zeroizing::new(native.to_vec());
    native.zeroize();
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

/// A LocalSystem executable must not sit below a user-replaceable directory.
/// Inspect ancestors without changing permissions on unrelated folders.
pub fn validate_install_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let raw = unsafe {
            CreateFileW(
                wide(ancestor).as_ptr(),
                READ_CONTROL,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().into());
        }
        let handle = Handle(raw);
        let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(Error::Access);
        }
        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let code = unsafe {
            GetSecurityInfo(
                handle.0,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if code != 0 {
            return Err(Error::Access);
        }
        let result = (|| -> Result<()> {
            fn trusted(sid: &str) -> bool {
                matches!(
                    sid,
                    "S-1-5-18"
                        | "S-1-5-32-544"
                        | "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
                )
            }
            if !trusted(&sid_text(owner)?) || acl.is_null() {
                return Err(Error::Access);
            }
            for index in 0..unsafe { (*acl).AceCount } {
                let mut ace = ptr::null_mut();
                if unsafe { GetAce(acl, index as u32, &mut ace) } == 0 {
                    return Err(Error::Access);
                }
                let header = unsafe { &*(ace as *const ACE_HEADER) };
                if header.AceFlags & INHERIT_ONLY_ACE as u8 != 0 {
                    continue;
                }
                if header.AceType == ACCESS_ALLOWED_ACE_TYPE as u8 {
                    let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
                    let dangerous = DELETE
                        | WRITE_DAC
                        | WRITE_OWNER
                        | FILE_DELETE_CHILD
                        | windows_sys::Win32::Foundation::GENERIC_ALL
                        | windows_sys::Win32::Foundation::GENERIC_WRITE;
                    if allowed.Mask & dangerous != 0
                        && !trusted(&sid_text(
                            (&allowed.SidStart as *const u32).cast_mut().cast(),
                        )?)
                    {
                        return Err(Error::Access);
                    }
                } else if header.AceType != ACCESS_DENIED_ACE_TYPE as u8
                    && header.AceType != SYSTEM_AUDIT_ACE_TYPE as u8
                {
                    return Err(Error::Access);
                }
            }
            Ok(())
        })();
        unsafe {
            LocalFree(descriptor);
        }
        result?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn boot_identity_is_stable_and_nonzero() {
        let first = super::boot_id().unwrap();
        assert_eq!(first, super::boot_id().unwrap());
        assert_eq!(first.len(), 32);
        assert_ne!(first, "0".repeat(32));
    }
    use super::*;
    #[test]
    fn default_install_ancestors_are_protected() {
        validate_install_ancestors(&install_directory().unwrap()).unwrap();
    }
    #[test]
    fn service_owner_can_start_and_query_but_cannot_change_or_delete_service() {
        let descriptor = Descriptor::service(&current_sid().unwrap()).unwrap();
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = ptr::null_mut();
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut acl, &mut defaulted)
            },
            0
        );
        let mut ace = ptr::null_mut();
        assert_ne!(unsafe { GetAce(acl, 2, &mut ace) }, 0);
        let mask = unsafe { (*(ace as *const ACCESS_ALLOWED_ACE)).Mask };
        assert_eq!(mask, 0x14);
        assert!(Descriptor::service("S-1-5-18)(A;;GA;;;WD)").is_err());
    }
    #[test]
    fn machine_dpapi_roundtrip_and_tamper_rejection() {
        let encrypted = protect(b"synthetic-password").unwrap();
        assert!(!encrypted
            .windows(18)
            .any(|bytes| bytes == b"synthetic-password"));
        assert_eq!(&*unprotect(&encrypted).unwrap(), b"synthetic-password");
        let mut damaged = encrypted;
        let n = damaged.len() - 1;
        damaged[n] ^= 1;
        assert!(unprotect(&damaged).is_err());
    }
    #[test]
    fn sid_is_validated_before_sddl_interpolation() {
        assert!(Descriptor::pipe(&current_sid().unwrap()).is_ok());
        assert!(Descriptor::pipe("S-1-5-18)(A;;GA;;;WD)").is_err());
    }
    #[test]
    fn ordinary_pipe_client_cannot_create_server_instances() {
        let descriptor = Descriptor::pipe(&current_sid().unwrap()).unwrap();
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = ptr::null_mut();
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut acl, &mut defaulted)
            },
            0
        );
        let mut ace = ptr::null_mut();
        assert_ne!(unsafe { GetAce(acl, 2, &mut ace) }, 0);
        let mask = unsafe { (*(ace as *const ACCESS_ALLOWED_ACE)).Mask };
        assert_eq!(mask & FILE_CREATE_PIPE_INSTANCE, 0);
        assert_eq!(mask & 3, 3);
    }
}
