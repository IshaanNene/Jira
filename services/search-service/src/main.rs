use search_service::{migrate, reindex, run, SERVICE_NAME};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = observability::init(SERVICE_NAME)?;
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        // Saved filters; the index itself is created at startup.
        Some("migrate") => migrate().await,
        Some("reindex") => {
            let tenant = args.next().ok_or_else(|| anyhow::anyhow!("usage: search-service reindex <tenant-id>"))?;
            reindex(tenant.parse()?).await
        }
        _ => {
            run(telemetry).await?.shutdown();
            Ok(())
        }
    }
}
