//! Read-only release download check. Never installs or contacts the campus portal.
use portal_update::download::Downloader;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("destination file required")?;
    let mirrors = std::env::args().any(|v| v == "--mirrors");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let client = Downloader::new()?;
            let mut release = client.latest("0.0.0").await?.ok_or("no release")?;
            if mirrors {
                release.direct = false;
            }
            let source = client
                .download(&release, std::path::Path::new(&path), |_, _| {})
                .await?;
            println!(
                "verified=true version={} bytes={} source={source}",
                release.update.version(),
                release.update.asset().size
            );
            Ok(())
        })
}
