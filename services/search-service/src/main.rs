use search_service::{reindex, run, SERVICE_NAME};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = observability::init(SERVICE_NAME)?;
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        // The index is created at startup; there is no database to migrate.
        Some("migrate") => Ok(()),
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
