use server::{
    config::{Config, now_ms},
    service::GameService,
    storage::{
        Archive, Store,
        local::{LocalArchive, LocalStore},
    },
    transport::{App, router},
};
use std::sync::Arc;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "server=info".into()),
        )
        .init();
    let config = Config::from_env()?;
    let auth = config.auth()?;
    let (store, archive): (Arc<dyn Store>, Arc<dyn Archive>) = if config.aws {
        aws_storage().await?
    } else {
        (
            Arc::new(LocalStore::open(&config.data_dir.join("state.sqlite3"))?),
            Arc::new(LocalArchive::new(config.data_dir.join("archive"))),
        )
    };
    let archive = research_archive(archive).await?;
    let capacity = std::env::var("SETTLERS_MAX_LOADED_GAMES")
        .ok()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(usize::MAX);
    anyhow::ensure!(capacity > 0, "SETTLERS_MAX_LOADED_GAMES must be positive");
    let games = GameService::restore_with_capacity(store, archive, now_ms(), capacity).await?;
    #[cfg(feature = "aws")]
    if let (Ok(id), Ok(table)) = (
        std::env::var("SETTLERS_SERVER_ID"),
        std::env::var("SETTLERS_DYNAMODB_TABLE"),
    ) {
        let games = games.clone();
        tokio::spawn(async move {
            if let Err(error) = server::runtime::report(games, id, table).await {
                tracing::warn!(error=%error, "runtime telemetry unavailable");
            }
        });
    }
    let ownership_task = std::env::var("SETTLERS_SERVER_ID").ok().map(|_| {
        let games = games.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                interval.tick().await;
                if let Err(error) = games.sync_ownership().await {
                    tracing::warn!(error=%error, "ownership refresh deferred");
                }
            }
        })
    });
    let timer_games = games.clone();
    let timer = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            let scheduled = interval.tick().await;
            timer_games.observe_tick_lag(scheduled.elapsed().as_millis() as u64);
            if let Err(error) = timer_games.tick(now_ms()).await {
                tracing::warn!(error=%error,"deadline processing deferred");
            }
        }
    });
    let archive_games = games.clone();
    let archive_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            if let Err(error) = archive_games.flush_archive().await {
                tracing::warn!(error=%error,"archive delivery deferred");
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(address=%listener.local_addr()?,"settlers server ready");
    axum::serve(
        listener,
        router(App {
            auth,
            games: games.clone(),
        }),
    )
    .with_graceful_shutdown(async {
        shutdown().await;
    })
    .await?;
    timer.abort();
    archive_task.abort();
    if let Some(task) = ownership_task {
        task.abort();
    }
    if let Err(error) = games.flush_archive().await {
        tracing::warn!(error=%error,"pending archive events will resume after restart");
    }
    Ok(())
}
#[cfg(feature = "aws")]
async fn aws_storage() -> anyhow::Result<(Arc<dyn Store>, Arc<dyn Archive>)> {
    use server::storage::aws::{DynamoStore, FirehoseArchive, S3Archive};
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let table = std::env::var("SETTLERS_DYNAMODB_TABLE")?;
    let bucket = std::env::var("SETTLERS_ARCHIVE_BUCKET")?;
    let mut store = DynamoStore::new(aws_sdk_dynamodb::Client::new(&config), table);
    if let Ok(id) = std::env::var("SETTLERS_SERVER_ID") {
        store = store.with_owner(id);
    } else if let Ok(id) = std::env::var("SETTLERS_GAME_ID") {
        store = store.with_assigned_game(id);
    }
    let mut archive: Arc<dyn Archive> =
        Arc::new(S3Archive::new(aws_sdk_s3::Client::new(&config), bucket));
    if let Ok(stream) = std::env::var("SETTLERS_FIREHOSE_STREAM") {
        anyhow::ensure!(
            stream.split(',').all(|name| !name.trim().is_empty()),
            "SETTLERS_FIREHOSE_STREAM contains an empty stream name"
        );
        archive = Arc::new(FirehoseArchive::new(
            archive,
            aws_sdk_firehose::Client::new(&config),
            stream,
        ));
    }
    Ok((Arc::new(store), archive))
}
#[cfg(not(feature = "aws"))]
async fn aws_storage() -> anyhow::Result<(Arc<dyn Store>, Arc<dyn Archive>)> {
    anyhow::bail!("AWS storage requires cargo build --features aws")
}

async fn research_archive(archive: Arc<dyn Archive>) -> anyhow::Result<Arc<dyn Archive>> {
    let Ok(stream) = std::env::var("SETTLERS_RESEARCH_FIREHOSE_STREAM") else {
        return Ok(archive);
    };
    anyhow::ensure!(
        stream.split(',').all(|name| !name.trim().is_empty()),
        "SETTLERS_RESEARCH_FIREHOSE_STREAM must not be empty"
    );
    #[cfg(feature = "aws")]
    {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Ok(Arc::new(server::storage::research::ResearchArchive::new(
            archive,
            aws_sdk_firehose::Client::new(&config),
            stream,
        )))
    }
    #[cfg(not(feature = "aws"))]
    anyhow::bail!("Research Firehose delivery requires cargo build --features aws")
}

async fn shutdown() {
    #[cfg(unix)]
    if let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}
