//! Explicit one-game import; online requests never scan S3 archives.
#[cfg(feature = "aws")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use server::storage::{Archive, aws::S3Archive};
    let id = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: archive-import <game UUID>"))?;
    uuid::Uuid::parse_str(&id)?;
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let archive = S3Archive::new(
        aws_sdk_s3::Client::new(&config),
        std::env::var("SETTLERS_ARCHIVE_BUCKET")?,
    );
    let records = archive.read_firehose_export(&id).await?;
    // Verify the whole selected replay before writing any history batches.
    server::reducer::replay(&records)?;
    archive.deliver(&records).await?;
    println!(
        "Batched {} verified base-v9 records for {id}",
        records.len()
    );
    Ok(())
}
#[cfg(not(feature = "aws"))]
fn main() {
    eprintln!("archive-import requires --features aws");
    std::process::exit(1);
}
