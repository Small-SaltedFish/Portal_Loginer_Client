use super::{
    host, ipc, security,
    storage::{self, Store},
    Error, Result,
};
use portal_update::{download::Downloader, payload::FILES, Architecture, Platform};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{watch, Notify};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    pub phase: String,
    pub version: String,
    pub source: String,
    pub downloaded: u64,
    pub total: u64,
    pub message: String,
}
pub struct Ready {
    pub job: PathBuf,
    pub manual: bool,
}
pub struct Control {
    pub automatic: AtomicBool,
    pub online: AtomicBool,
    pub wake: Notify,
    pub install: AtomicBool,
    pub state: watch::Sender<View>,
    pub busy_until: AtomicU64,
    pub staged: AtomicBool,
    pub force_staged: AtomicBool,
}
impl Control {
    pub fn new(automatic: bool) -> Arc<Self> {
        let (state, _) = watch::channel(View {
            phase: "idle".into(),
            ..View::default()
        });
        Arc::new(Self {
            automatic: AtomicBool::new(automatic),
            online: AtomicBool::new(false),
            wake: Notify::new(),
            install: AtomicBool::new(false),
            state,
            busy_until: AtomicU64::new(0),
            staged: AtomicBool::new(false),
            force_staged: AtomicBool::new(false),
        })
    }
    pub fn request(&self, install: bool) {
        if install {
            self.force_staged.store(true, Ordering::SeqCst);
        }
        self.install.store(install, Ordering::SeqCst);
        self.wake.notify_one();
    }
    pub fn activity(&self, busy: bool) {
        self.busy_until
            .store(if busy { now() + 90 } else { 0 }, Ordering::SeqCst);
    }
    pub(super) fn show(&self, phase: &str, message: &str) {
        self.state.send_modify(|s| {
            s.phase = phase.into();
            s.message = message.into();
        });
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn root(store: &Store) -> Result<PathBuf> {
    let path = store.directory.join("updates");
    security::secure_directory(&path)?;
    Ok(path)
}
fn safe_job(root: &Path, name: &str) -> Result<PathBuf> {
    if !name.starts_with("job-")
        || name.len() != 36
        || !name[4..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::Invalid);
    }
    Ok(root.join(name))
}
fn reject_links(path: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path)?;
    if m.file_attributes() & 0x400 != 0 {
        return Err(Error::Access);
    }
    Ok(())
}
pub(super) fn remove_tree(path: &Path) -> Result<()> {
    reject_links(path)?;
    for e in std::fs::read_dir(path)? {
        let e = e?;
        reject_links(&e.path())?;
        if e.file_type()?.is_dir() {
            remove_tree(&e.path())?;
        } else {
            std::fs::remove_file(e.path())?;
        }
    }
    std::fs::remove_dir(path)?;
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Journal {
    job: String,
    old_version: String,
    new_version: String,
    phase: String,
    pid: u32,
}
fn journal(root: &Path) -> Result<Option<Journal>> {
    let path = root.join("active.json");
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::from_slice(&storage::read_limited(&path, 4096)?).map_err(|_| Error::Invalid)?,
    ))
}
fn save_journal(root: &Path, value: &Journal) -> Result<()> {
    storage::atomic_write(
        root,
        "active.json",
        &serde_json::to_vec(value).map_err(|_| Error::Invalid)?,
    )
}
fn helper_alive(job: &Path, pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::*;
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return false;
    }
    let handle = security::Handle(raw);
    let mut buffer = vec![0u16; 32768];
    let mut size = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle.0, 0, buffer.as_mut_ptr(), &mut size) } == 0 {
        return false;
    }
    String::from_utf16_lossy(&buffer[..size as usize])
        .eq_ignore_ascii_case(&job.join("runner.exe").to_string_lossy())
}
fn spawn(job: &Path, recover: bool) -> Result<()> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(job.join("runner.exe"))
        .arg(if recover {
            "recover-update"
        } else {
            "apply-update"
        })
        .arg("--job")
        .arg(job)
        .creation_flags(0x08000000)
        .spawn()?;
    Ok(())
}
pub fn recover_interrupted() -> Result<bool> {
    let store = Store::system()?;
    let path = store.directory.join("updates");
    if !path.exists() {
        return Ok(false);
    }
    reject_links(&path)?;
    if let Some(j) = journal(&path)? {
        let job = safe_job(&path, &j.job)?;
        if matches!(j.phase.as_str(), "prepared" | "applying" | "checking")
            && !helper_alive(&job, j.pid)
        {
            spawn(&job, true)?;
            return Ok(true);
        }
    }
    Ok(false)
}
#[cfg_attr(test, allow(dead_code))]
pub async fn worker(
    store: Store,
    control: Arc<Control>,
    ready: tokio::sync::mpsc::Sender<Ready>,
    stop: CancellationToken,
) {
    let owner = store.load().map(|s| s.owner_sid).unwrap_or_default();
    let mut delay = Duration::from_secs(30 + rand::random::<u64>() % 90);
    if let Ok(path) = root(&store) {
        if let Ok(bytes) = storage::read_limited(&path.join("next-check"), 64) {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                if let Ok(time) = text.parse::<u64>() {
                    delay = Duration::from_secs(time.saturating_sub(now()).min(86400));
                }
            }
        }
    }
    loop {
        let manual = tokio::select! {_=stop.cancelled()=>return,_=control.wake.notified()=>true,_=tokio::time::sleep(delay)=>false};
        if control.staged.load(Ordering::SeqCst) {
            delay = Duration::from_secs(60);
            continue;
        }
        let install = control.install.swap(false, Ordering::SeqCst);
        if !manual
            && (!control.automatic.load(Ordering::SeqCst) || !control.online.load(Ordering::SeqCst))
        {
            delay = Duration::from_secs(60);
            continue;
        }
        if recover_interrupted().unwrap_or(false) {
            return;
        }
        let mut failure_message = None;
        let result = async {
            let path = root(&store)?;
            if journal(&path)?
                .is_some_and(|j| matches!(j.phase.as_str(), "prepared" | "applying" | "checking"))
            {
                return Err(Error::Invalid);
            }
            control.show("checking", "正在通过镜像检查更新");
            let proxy = super::uninstall::owner_proxy(&owner).ok().flatten();
            let client = Downloader::with_proxy(proxy.as_deref()).map_err(|_| Error::Invalid)?;
            let cached = storage::read_limited(&path.join("release-cache.json"), 65536).ok();
            let Some(release) = client
                .latest_with_cache(
                    env!("CARGO_PKG_VERSION"),
                    cached.as_ref().map(|v| v.as_slice()),
                )
                .await
                .map_err(|error| {
                    failure_message = Some(format!("镜像检查失败：{error}"));
                    Error::Invalid
                })?
            else {
                control.show("current", "镜像检查完成，当前没有可用更新");
                return Ok(());
            };
            storage::atomic_write(&path, "release-cache.json", &release.manifest)?;
            control.state.send_modify(|s| {
                s.source = release.metadata_source.clone();
                s.version = release.update.version().to_string();
                s.total = release.update.asset().size;
                s.downloaded = 0;
            });
            if let Ok(bytes) = storage::read_limited(&path.join("failed-version"), 128) {
                if !install && bytes.as_slice() == release.update.version().to_string().as_bytes() {
                    control.show("failed", "该版本更新失败并已回退，等待新版本");
                    return Ok(());
                }
            }
            if !install && !control.automatic.load(Ordering::SeqCst) {
                control.show("available", "发现新版本，可点击下载并安装");
                return Ok(());
            }
            let name = format!("job-{:032x}", rand::random::<u128>());
            let job = safe_job(&path, &name)?;
            let retain = journal(&path)?.map(|j| j.job);
            for entry in std::fs::read_dir(&path)? {
                let entry = entry?;
                let n = entry.file_name().to_string_lossy().to_string();
                if n.starts_with("job-") && Some(&n) != retain.as_ref() {
                    safe_job(&path, &n)?;
                    remove_tree(&entry.path())?;
                }
            }
            security::secure_directory(&job)?;
            storage::atomic_write(&job, "manifest.json", &release.manifest)?;
            control.show("downloading", "正在下载更新");
            let state = control.state.clone();
            let source = client
                .download(&release, &job.join("payload.part"), move |n, source| {
                    state.send_modify(|s| {
                        s.downloaded = n;
                        s.source = source.to_owned();
                    });
                })
                .await
                .map_err(|error| {
                    failure_message = Some(format!("镜像下载失败：{error}"));
                    Error::Invalid
                })?;
            std::fs::rename(job.join("payload.part"), job.join("payload.zip"))?;
            std::fs::copy(std::env::current_exe()?, job.join("runner.exe"))?;
            control.state.send_modify(|s| {
                s.source = source;
                s.phase = "ready".into();
                s.message = "更新已验证，等待空闲时安装".into();
            });
            control.staged.store(true, Ordering::SeqCst);
            ready
                .send(Ready {
                    job,
                    manual: install,
                })
                .await
                .map_err(|_| Error::Invalid)?;
            Ok(())
        }
        .await;
        delay = if result.is_ok() {
            Duration::from_secs(22 * 3600 + rand::random::<u64>() % (4 * 3600))
        } else {
            let _ = super::logs::record(
                &store,
                super::logs::Event::Update("check_or_download_failed"),
            );
            let message = failure_message.unwrap_or_else(|| "更新文件处理失败，请查看日志".into());
            let _ = super::logs::record(&store, super::logs::Event::UpdateFailure(message.clone()));
            control.show("failed", &message);
            Duration::from_secs(15 * 60)
        };
        if let Ok(path) = root(&store) {
            let _ = storage::atomic_write(
                &path,
                "next-check",
                (now() + delay.as_secs()).to_string().as_bytes(),
            );
        }
    }
}
pub fn can_install(control: &Control, install: &Path) -> bool {
    if now() < control.busy_until.load(Ordering::SeqCst) {
        return false;
    }
    use std::os::windows::fs::OpenOptionsExt;
    let path = install.join("portal-settings.exe");
    !path.exists()
        || std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(path)
            .is_ok()
}
pub fn launch(ready: &Ready) -> Result<()> {
    spawn(&ready.job, false)
}
fn replace(source: &Path, destination: &Path) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::*;
    reject_links(source)?;
    let temporary = destination.with_extension("update-tmp");
    if temporary.exists() {
        reject_links(&temporary)?;
        std::fs::remove_file(&temporary)?;
    }
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    if unsafe {
        MoveFileExW(
            security::wide(&temporary).as_ptr(),
            security::wide(destination).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
fn healthy(version: &str) -> bool {
    let Ok(runtime) = super::runtime::runtime() else {
        return false;
    };
    runtime.block_on(async {
        ipc::client(b"{\"command\":\"status\"}")
            .await
            .is_ok_and(|v| v["status"]["version"] == version)
    })
}
pub fn apply(job: &Path, recover: bool) -> Result<()> {
    security::require_admin()?;
    let _maintenance = security::maintenance()?;
    let store = Store::system()?;
    let path = root(&store)?;
    let name = job
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(Error::Invalid)?;
    if safe_job(&path, name)?.canonicalize()? != job.canonicalize()? {
        return Err(Error::Access);
    }
    reject_links(job)?;
    let install = super::uninstall::installed_directory()?.ok_or(Error::Invalid)?;
    super::uninstall::validate_cleanup_target(&install)?;
    if recover {
        let mut j = journal(&path)?.ok_or(Error::Invalid)?;
        if j.job != name || !matches!(j.phase.as_str(), "prepared" | "applying" | "checking") {
            return Err(Error::Invalid);
        }
        if j.phase == "prepared" {
            j.phase = "rolled_back".into();
            save_journal(&path, &j)?;
            return host::start_application();
        }
        return rollback(&install, job, &path, &mut j);
    }
    let raw = storage::read_limited(&job.join("manifest.json"), 65536)?;
    let update = portal_update::verify_manifest(
        &raw,
        env!("CARGO_PKG_VERSION"),
        Platform::Windows,
        Architecture::X86_64,
    )
    .map_err(|_| Error::Invalid)?
    .ok_or(Error::Invalid)?;
    portal_update::download::verify_file(&update, &job.join("payload.zip"))
        .map_err(|_| Error::Invalid)?;
    portal_update::payload::extract(&job.join("payload.zip"), &job.join("next"))
        .map_err(|_| Error::Invalid)?;
    let mut j = Journal {
        job: name.into(),
        old_version: env!("CARGO_PKG_VERSION").into(),
        new_version: update.version().to_string(),
        phase: "prepared".into(),
        pid: std::process::id(),
    };
    save_journal(&path, &j)?;
    let _ = super::logs::record(&store, super::logs::Event::Update("install_started"));
    use std::os::windows::process::CommandExt;
    std::process::Command::new(job.join("runner.exe"))
        .arg("watch-update")
        .arg("--job")
        .arg(job)
        .arg("--pid")
        .arg(std::process::id().to_string())
        .creation_flags(0x08000000)
        .spawn()?;
    // Give tray processes time to leave after the service's UPDATING event.
    std::thread::sleep(Duration::from_secs(2));
    host::stop_existing()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while super::uninstall::ensure_files_idle(&install, true).is_err() {
        if Instant::now() >= deadline {
            let _ = host::start_application();
            return Err(Error::Timeout);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let backup = job.join("backup");
    security::secure_directory(&backup)?;
    for name in FILES {
        reject_links(&install.join(name))?;
        std::fs::copy(install.join(name), backup.join(name))?;
    }
    j.phase = "applying".into();
    save_journal(&path, &j)?;
    let result = (|| -> Result<()> {
        // Service executable switches last; an interrupted boot retains a runnable service.
        for name in FILES
            .into_iter()
            .filter(|n| *n != "portal-service.exe")
            .chain(["portal-service.exe"])
        {
            replace(&job.join("next").join(name), &install.join(name))?;
        }
        j.phase = "checking".into();
        save_journal(&path, &j)?;
        host::start_application()?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while !healthy(&j.new_version) {
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        j.phase = "committed".into();
        save_journal(&path, &j)?;
        super::uninstall::set_version(&j.new_version)?;
        let _ = super::logs::record(&store, super::logs::Event::Update("committed"));
        Ok(())
    })();
    if result.is_err() {
        let _ = super::logs::record(&store, super::logs::Event::Update("rollback"));
        rollback(&install, job, &path, &mut j)?;
    }
    result
}
fn rollback(install: &Path, job: &Path, path: &Path, j: &mut Journal) -> Result<()> {
    host::stop_existing()?;
    for name in FILES {
        replace(&job.join("backup").join(name), &install.join(name))?;
    }
    j.phase = "rolled_back".into();
    save_journal(path, j)?;
    storage::atomic_write(path, "failed-version", j.new_version.as_bytes())?;
    super::uninstall::set_version(&j.old_version)?;
    host::start_application()
}

pub fn watch(job: &Path, pid: u32) -> Result<()> {
    security::require_admin()?;
    let store = Store::system()?;
    let path = root(&store)?;
    let name = job
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(Error::Invalid)?;
    if safe_job(&path, name)?.canonicalize()? != job.canonicalize()? {
        return Err(Error::Access);
    }
    let j = journal(&path)?.ok_or(Error::Invalid)?;
    if j.pid != pid || j.job != name {
        return Ok(());
    }
    use windows_sys::Win32::System::Threading::*;
    let handle = unsafe { OpenProcess(0x00100001, 0, pid) };
    if !handle.is_null() {
        let handle = security::Handle(handle);
        if helper_alive(job, pid) {
            unsafe {
                if WaitForSingleObject(handle.0, 300000) == 0x102 {
                    TerminateProcess(handle.0, 1);
                    WaitForSingleObject(handle.0, 10000);
                }
            }
        }
    }
    let Some(j) = journal(&path)? else {
        return Ok(());
    };
    if matches!(j.phase.as_str(), "prepared" | "applying" | "checking") && !helper_alive(job, pid) {
        for _ in 0..3 {
            if apply(job, true).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        let _ = host::start_application();
        return Err(Error::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn job_paths_cannot_escape_protected_root() {
        let root = Path::new(r"C:\ProgramData\PortalLoginer\updates");
        for name in [
            "..",
            "job-../../outside",
            "job-0000",
            "job-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            assert!(safe_job(root, name).is_err());
        }
        assert_eq!(
            safe_job(root, "job-00000000000000000000000000000000")
                .unwrap()
                .parent(),
            Some(root)
        );
    }
    #[test]
    fn atomic_file_switch_can_restore_all_previous_files() {
        let fixture = super::super::storage::TestStore::new();
        let root = &fixture.store.directory;
        let installed = root.join("installed");
        let next = root.join("next");
        let backup = root.join("backup");
        for path in [&installed, &next, &backup] {
            std::fs::create_dir(path).unwrap();
        }
        for name in FILES {
            std::fs::write(installed.join(name), b"previous").unwrap();
            std::fs::write(next.join(name), b"updated").unwrap();
            std::fs::copy(installed.join(name), backup.join(name)).unwrap();
        }
        for name in FILES
            .into_iter()
            .filter(|n| *n != "portal-service.exe")
            .chain(["portal-service.exe"])
        {
            replace(&next.join(name), &installed.join(name)).unwrap();
        }
        for name in FILES {
            assert_eq!(std::fs::read(installed.join(name)).unwrap(), b"updated");
            replace(&backup.join(name), &installed.join(name)).unwrap();
            assert_eq!(std::fs::read(installed.join(name)).unwrap(), b"previous");
        }
    }
}
