use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};
pub const FILES: [&str; 7] = [
    "portal-service.exe",
    "portal-tray.exe",
    "portal-settings.exe",
    "Uninstall.exe",
    "LICENSE",
    "NOTICE",
    "THIRD-PARTY-NOTICES.txt",
];
pub fn extract(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|_| "cannot open payload")?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| "invalid zip payload")?;
    if zip.len() != FILES.len() {
        return Err("unexpected payload file count".into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(|_| "invalid payload entry")?;
        if !FILES.contains(&entry.name())
            || !seen.insert(entry.name().to_owned())
            || entry.is_dir()
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
            || entry.size() > 64 * 1024 * 1024
        {
            return Err("unexpected payload file".into());
        }
        total = total
            .checked_add(entry.size())
            .ok_or("payload size overflow")?;
        if total > 128 * 1024 * 1024 {
            return Err("expanded payload exceeds limit".into());
        }
    }
    fs::create_dir(destination).map_err(|_| "payload destination must be new")?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|_| "invalid payload entry")?;
        let expected = entry.size();
        let name = entry.name().to_owned();
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination.join(&name))
            .map_err(|_| "cannot write payload")?;
        let copied = std::io::copy(&mut entry.by_ref().take(expected + 1), &mut out)
            .map_err(|_| "cannot extract payload")?;
        if copied != expected {
            return Err("payload size mismatch".into());
        }
        out.flush().map_err(|_| "cannot flush payload")?;
        out.sync_all().map_err(|_| "cannot persist payload")?;
        if name.ends_with(".exe") {
            let mut file =
                File::open(destination.join(name)).map_err(|_| "cannot read executable")?;
            let mut magic = [0; 2];
            file.read_exact(&mut magic)
                .map_err(|_| "invalid executable")?;
            if magic != *b"MZ" {
                return Err("invalid executable".into());
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> std::path::PathBuf {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .canonicalize()
            .unwrap();
        let path = root.join(format!(
            "payload-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    fn archive(path: &Path, malicious: bool) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (i, name) in FILES.iter().enumerate() {
            let name = if malicious && i == 0 {
                "../portal-service.exe"
            } else {
                name
            };
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(if name.ends_with(".exe") {
                b"MZ-synthetic"
            } else {
                b"license"
            })
            .unwrap();
        }
        zip.finish().unwrap();
    }
    #[test]
    fn exact_payload_extracts_and_parent_paths_are_rejected_before_writes() {
        let dir = fixture();
        let good = dir.join("good.zip");
        archive(&good, false);
        extract(&good, &dir.join("out")).unwrap();
        for name in FILES {
            assert!(dir.join("out").join(name).is_file());
            fs::remove_file(dir.join("out").join(name)).unwrap();
        }
        fs::remove_dir(dir.join("out")).unwrap();
        let bad = dir.join("bad.zip");
        archive(&bad, true);
        assert!(extract(&bad, &dir.join("bad-out")).is_err());
        assert!(!dir.join("bad-out").exists());
        fs::remove_file(good).unwrap();
        fs::remove_file(bad).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
