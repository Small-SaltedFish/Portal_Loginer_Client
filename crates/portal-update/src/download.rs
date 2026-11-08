use crate::{
    verify_manifest, Architecture, Platform, VerifiedUpdate, MAX_MANIFEST_BYTES, REPOSITORY,
};
use reqwest::{Client, StatusCode};
use std::{
    io::Write,
    path::Path,
    time::{Duration, Instant},
};
pub const MIRRORS: [&str; 3] = [
    "https://ghfast.top/",
    "https://ghproxy.net/",
    "https://gh-proxy.org/",
];
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no release published")]
    NotPublished,
    #[error("request timed out")]
    Timeout,
    #[error("HTTP {0}")]
    Http(u16),
    #[error("{0}")]
    Mirrors(String),
    #[error("update source unavailable")]
    Network,
    #[error("update verification failed")]
    Verification,
    #[error("cannot store update")]
    Storage,
}
#[derive(Clone)]
pub struct Downloader {
    client: Client,
    direct: Client,
}
pub struct Release {
    pub update: VerifiedUpdate,
    pub manifest: Vec<u8>,
    pub metadata_source: String,
}
fn trusted_host(host: &str) -> bool {
    matches!(
        host,
        "ghfast.top" | "ghproxy.net" | "gh-proxy.org" | "gh-proxy.com"
    ) || host.ends_with(".gh-proxy.org")
        || host.ends_with(".gh-proxy.com")
}
impl Downloader {
    pub fn new() -> Result<Self, Error> {
        Self::with_proxy(None)
    }
    pub fn with_proxy(proxy: Option<&str>) -> Result<Self, Error> {
        let builder = || {
            Client::builder()
                .https_only(true)
                .user_agent(concat!("PortalLoginer/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(4))
                .redirect(reqwest::redirect::Policy::custom(|a| {
                    if a.previous().len() < 6
                        && a.url().scheme() == "https"
                        && a.url().host_str().is_some_and(trusted_host)
                    {
                        a.follow()
                    } else {
                        a.stop()
                    }
                }))
        };
        let direct = builder().no_proxy().build().map_err(|_| Error::Network)?;
        let client = if let Some(proxy) = proxy {
            reqwest::Proxy::all(proxy)
                .ok()
                .and_then(|p| builder().proxy(p).build().ok())
                .unwrap_or_else(|| direct.clone())
        } else {
            builder().build().map_err(|_| Error::Network)?
        };
        Ok(Self { client, direct })
    }
    async fn small(&self, url: &str, max: usize) -> Result<Vec<u8>, Error> {
        let mut response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(6))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    Error::Timeout
                } else {
                    Error::Network
                }
            })?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(Error::NotPublished);
        }
        if response.status() != StatusCode::OK {
            return Err(Error::Http(response.status().as_u16()));
        }
        if response.content_length().is_some_and(|n| n > max as u64) {
            return Err(Error::Verification);
        }
        let mut out = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
            if out.len() + chunk.len() > max {
                return Err(Error::Network);
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
    pub async fn latest(&self, current: &str) -> Result<Option<Release>, Error> {
        self.latest_with_cache(current, None).await
    }
    pub async fn latest_with_cache(
        &self,
        current: &str,
        cached: Option<&[u8]>,
    ) -> Result<Option<Release>, Error> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let urls = MIRRORS.iter().map(|prefix| (prefix.to_string(), format!("{prefix}https://raw.githubusercontent.com/{REPOSITORY}/main/updates/manifest.json?check={stamp}"))).collect();
        self.latest_from(current, cached, urls).await
    }
    async fn latest_from(
        &self,
        current: &str,
        cached: Option<&[u8]>,
        urls: Vec<(String, String)>,
    ) -> Result<Option<Release>, Error> {
        let version = crate::version(current).map_err(|_| Error::Verification)?;
        let mut tasks = tokio::task::JoinSet::new();
        for (source, url) in urls {
            let mut client = self.clone();
            client.client = client.direct.clone();
            tasks.spawn(async move {
                let started = Instant::now();
                let result = async {
                    let raw = client.small(&url, MAX_MANIFEST_BYTES).await?;
                    let update =
                        verify_manifest(&raw, "0.0.0", Platform::Windows, Architecture::X86_64)
                            .map_err(|_| Error::Verification)?
                            .ok_or(Error::Verification)?;
                    Ok::<_, Error>((update, raw))
                }
                .await;
                (source, started.elapsed(), result)
            });
        }
        let mut best: Option<(VerifiedUpdate, Vec<u8>, String, Duration)> = None;
        let mut failures = Vec::new();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((source, elapsed, Ok((update, raw)))) => {
                    if best.as_ref().is_none_or(|(b, _, _, t)| {
                        update.version() > b.version()
                            || (update.version() == b.version() && elapsed < *t)
                    }) {
                        best = Some((update, raw, source, elapsed));
                    }
                }
                Ok((source, _, Err(error))) => failures.push(format!("{source} {error}")),
                Err(_) => failures.push("mirror task failed".into()),
            }
        }
        if let Some((update, raw, source, _)) = best {
            return Ok((update.version() > &version).then_some(Release {
                update,
                manifest: raw,
                metadata_source: source,
            }));
        }
        if let Some(raw) = cached {
            if let Some(update) =
                verify_manifest(raw, "0.0.0", Platform::Windows, Architecture::X86_64)
                    .ok()
                    .flatten()
                    .filter(|v| v.version() > &version)
            {
                return Ok(Some(Release {
                    update,
                    manifest: raw.to_vec(),
                    metadata_source: "缓存（镜像暂不可用）".into(),
                }));
            }
        }
        Err(Error::Mirrors(failures.join("; ")))
    }
    async fn sample(&self, prefix: &str, asset: &crate::Asset) -> Result<(String, f64), Error> {
        let url = format!("{prefix}{}", asset.url);
        let needed = asset.size.min(65536) as usize;
        let started = Instant::now();
        let mut response = self
            .client
            .get(url)
            .header("Range", format!("bytes=0-{}", needed - 1))
            .header("Accept-Encoding", "identity")
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| Error::Network)?;
        if !matches!(
            response.status(),
            StatusCode::OK | StatusCode::PARTIAL_CONTENT
        ) {
            return Err(Error::Network);
        }
        if response.status() == StatusCode::PARTIAL_CONTENT
            && !range_matches(
                response
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok()),
                0,
                asset.size,
            )
        {
            return Err(Error::Network);
        }
        let mut bytes = Vec::with_capacity(needed);
        while bytes.len() < needed {
            let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? else {
                break;
            };
            let count = (needed - bytes.len()).min(chunk.len());
            bytes.extend_from_slice(&chunk[..count]);
        }
        if bytes.len() != needed || !bytes.starts_with(b"PK\x03\x04") {
            return Err(Error::Network);
        }
        Ok((
            prefix.to_owned(),
            needed as f64 / started.elapsed().as_secs_f64().max(0.001),
        ))
    }
    async fn transfer(
        &self,
        url: &str,
        path: &Path,
        asset: &crate::Asset,
        progress: &(impl Fn(u64) + Sync),
    ) -> Result<(), Error> {
        let mut offset = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if offset > asset.size {
            offset = 0;
        }
        if offset == asset.size {
            return Ok(());
        }
        let mut request = self
            .client
            .get(url)
            .header("Accept-Encoding", "identity")
            .timeout(Duration::from_secs(180));
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
        }
        let mut response = request.send().await.map_err(|_| Error::Network)?;
        if response.status() == StatusCode::PARTIAL_CONTENT {
            if !range_matches(
                response
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok()),
                offset,
                asset.size,
            ) {
                return Err(Error::Network);
            }
        } else if response.status() == StatusCode::OK {
            offset = 0;
        } else {
            return Err(Error::Network);
        }
        if response
            .content_length()
            .is_some_and(|n| n != asset.size - offset)
        {
            return Err(Error::Network);
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(offset == 0)
            .append(offset > 0)
            .open(path)
            .map_err(|_| Error::Storage)?;
        progress(offset);
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
            if offset + chunk.len() as u64 > asset.size {
                return Err(Error::Verification);
            }
            file.write_all(&chunk).map_err(|_| Error::Storage)?;
            offset += chunk.len() as u64;
            progress(offset);
        }
        file.sync_all().map_err(|_| Error::Storage)?;
        if offset != asset.size {
            return Err(Error::Network);
        }
        Ok(())
    }
    pub async fn download(
        &self,
        release: &Release,
        path: &Path,
        progress: impl Fn(u64, &str) + Sync,
    ) -> Result<String, Error> {
        let asset = release.update.asset();
        if std::fs::metadata(path).is_ok_and(|m| m.len() >= asset.size) {
            std::fs::remove_file(path).map_err(|_| Error::Storage)?;
        }
        let mut tasks = tokio::task::JoinSet::new();
        for mirror in MIRRORS {
            let mut client = self.clone();
            client.client = client.direct.clone();
            let asset = asset.clone();
            tasks.spawn(async move { client.sample(mirror, &asset).await });
        }
        let mut ranked = Vec::new();
        while let Some(Ok(result)) = tasks.join_next().await {
            if let Ok(sample) = result {
                ranked.push(sample);
            }
        }
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (prefix, _) in ranked {
            for _ in 0..2 {
                let mut direct = self.clone();
                direct.client = direct.direct.clone();
                let result = direct
                    .transfer(&format!("{prefix}{}", asset.url), path, asset, &|n| {
                        progress(n, &prefix)
                    })
                    .await;
                if result.is_ok() {
                    if verify_file(&release.update, path).is_ok() {
                        return Ok(prefix);
                    }
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        Err(Error::Network)
    }
}
pub fn verify_file(update: &VerifiedUpdate, path: &Path) -> Result<(), Error> {
    let file = std::fs::File::open(path).map_err(|_| Error::Storage)?;
    update.verify_payload(file).map_err(|_| Error::Verification)
}
fn range_matches(value: Option<&str>, start: u64, total: u64) -> bool {
    let Some(value) = value.and_then(|v| v.strip_prefix("bytes ")) else {
        return false;
    };
    let Some((range, size)) = value.split_once('/') else {
        return false;
    };
    let Some((first, last)) = range.split_once('-') else {
        return false;
    };
    matches!((first.parse::<u64>(),last.parse::<u64>(),size.parse::<u64>()),(Ok(a),Ok(b),Ok(c))if a==start&&b>=a&&b<c&&c==total)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_range_validation() {
        assert!(range_matches(Some("bytes 5-9/10"), 5, 10));
        for value in [
            "bytes 0-9/10",
            "bytes 5-10/10",
            "bytes 5-9/11",
            "bytes */10",
        ] {
            assert!(!range_matches(Some(value), 5, 10));
        }
    }
    #[test]
    fn mirror_redirects_never_downgrade_or_use_arbitrary_hosts() {
        assert!(trusted_host("ghfast.top"));
        assert!(!trusted_host("github.com.evil.example"));
        assert!(!trusted_host("127.0.0.1"));
        assert!(!trusted_host("github.com"));
        assert!(!trusted_host("api.github.com"));
        assert!(!trusted_host("raw.githubusercontent.com"));
    }
}
#[cfg(test)]
mod transfer_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn server(response: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut input = vec![0; 8192];
            let count = stream.read(&mut input).await.unwrap();
            stream.write_all(&response).await.unwrap();
            String::from_utf8_lossy(&input[..count]).to_string()
        });
        (url, task)
    }
    fn asset() -> crate::Asset {
        crate::Asset {
            platform: Platform::Windows,
            architecture: Architecture::X86_64,
            name: "sample.zip".into(),
            url: "https://github.com/example/file.zip".into(),
            size: 17,
            sha256: "00".repeat(32),
        }
    }
    fn downloader() -> Downloader {
        Downloader {
            client: Client::builder().no_proxy().build().unwrap(),
            direct: Client::builder().no_proxy().build().unwrap(),
        }
    }
    fn file() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .canonicalize()
            .unwrap()
            .join(format!(
                "resume-test-{}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
    }
    fn metadata(version: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"schema":1,"repository":REPOSITORY,"version":version,"assets":[{"platform":"windows","architecture":"x86_64","name":format!("PortalLoginer-{version}-windows-x86_64-update.zip"),"url":format!("https://raw.githubusercontent.com/{REPOSITORY}/{}/updates/windows-x86_64.zip", "a".repeat(40)),"size":17,"sha256":"00".repeat(32)}]})).unwrap()
    }
    async fn metadata_server(body: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        server([header.as_bytes(), &body].concat()).await
    }
    #[tokio::test]
    async fn mirror_check_selects_newest_valid_metadata_and_ignores_html_and_bad_proxy() {
        let (old, old_task) = metadata_server(metadata("0.2.10")).await;
        let (new, new_task) = metadata_server(metadata("0.2.11")).await;
        let (html, html_task) = metadata_server(b"<html>blocked</html>".to_vec()).await;
        let mut client = downloader();
        client.client = Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
            .build()
            .unwrap();
        let value = client
            .latest_from(
                "0.2.9",
                None,
                vec![
                    ("old".into(), old),
                    ("new".into(), new),
                    ("html".into(), html),
                ],
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(value.update.version().to_string(), "0.2.11");
        assert_eq!(value.metadata_source, "new");
        for task in [old_task, new_task, html_task] {
            assert!(task.await.unwrap().starts_with("GET /"));
        }
    }
    #[tokio::test]
    async fn unavailable_mirrors_report_http_error_and_only_newer_cache_can_be_used() {
        for (cached, usable) in [
            (None, false),
            (Some(metadata("0.2.10")), false),
            (Some(metadata("0.2.11")), true),
        ] {
            let (url, task) = server(
                b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            )
            .await;
            let result = downloader()
                .latest_from(
                    "0.2.10",
                    cached.as_deref(),
                    vec![("test-mirror".into(), url)],
                )
                .await;
            if usable {
                assert!(result.unwrap().unwrap().metadata_source.contains("缓存"));
            } else {
                assert!(
                    matches!(result,Err(Error::Mirrors(text)) if text.contains("test-mirror") && text.contains("503"))
                );
            }
            task.await.unwrap();
        }
    }
    #[tokio::test]
    async fn resumes_only_matching_content_range() {
        let path = file();
        std::fs::write(&path, b"synth").unwrap();
        let(url,task)=server(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 12\r\nContent-Range: bytes 5-16/17\r\nConnection: close\r\n\r\netic-payload".to_vec()).await;
        downloader()
            .transfer(&url, &path, &asset(), &|_| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"synthetic-payload");
        assert!(task
            .await
            .unwrap()
            .to_lowercase()
            .contains("range: bytes=5-"));
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn mismatched_range_never_appends_to_partial_file() {
        let path = file();
        std::fs::write(&path, b"synth").unwrap();
        let(url,task)=server(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 12\r\nContent-Range: bytes 0-11/17\r\nConnection: close\r\n\r\netic-payload".to_vec()).await;
        assert!(downloader()
            .transfer(&url, &path, &asset(), &|_| {})
            .await
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"synth");
        let _ = task.await;
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn html_is_not_counted_as_fast_mirror() {
        let (url, task) = server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\n<html>not-a-zip!!!"
                .to_vec(),
        )
        .await;
        assert!(downloader().sample(&url, &asset()).await.is_err());
        let _ = task.await;
    }
}
