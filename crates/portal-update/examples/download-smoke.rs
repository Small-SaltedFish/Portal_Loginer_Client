//! Read-only release download check. Never installs or contacts the campus portal.
use portal_update::download::Downloader;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("destination file required")?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let client = Downloader::new()?;
            let release = client.latest("0.0.0").await?.ok_or("no release")?;
            println!("metadata_source={}", release.metadata_source);
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
