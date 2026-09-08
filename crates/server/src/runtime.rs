//! Optional ECS resource telemetry for an external fleet operator.
use crate::{config::now_ms, service::GameService};
use aws_sdk_dynamodb::{Client, types::AttributeValue as A};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub async fn report(games: GameService, server_id: String, table: String) -> anyhow::Result<()> {
    let endpoint = std::env::var("ECS_CONTAINER_METADATA_URI_V4")?;
    let vcpus: f64 = std::env::var("SETTLERS_TASK_VCPU")?.parse()?;
    let memory_mib: f64 = std::env::var("SETTLERS_TASK_MEMORY_MIB")?.parse()?;
    anyhow::ensure!(
        vcpus > 0.0 && memory_mib > 0.0,
        "task resource limits must be positive"
    );
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let db = Client::new(&config);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let mut previous: Option<(Instant, f64)> = None;
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let sampled = async {
            let stats: Value = client.get(format!("{endpoint}/stats")).send().await?.error_for_status()?.json().await?;
            let cpu = stats.pointer("/cpu_stats/cpu_usage/total_usage").and_then(Value::as_f64).ok_or_else(|| anyhow::anyhow!("CPU statistics missing"))?;
            let memory = stats.pointer("/memory_stats/usage").and_then(Value::as_f64).ok_or_else(|| anyhow::anyhow!("memory statistics missing"))?;
            let cache = stats.pointer("/memory_stats/stats/total_inactive_file").or_else(|| stats.pointer("/memory_stats/stats/inactive_file")).and_then(Value::as_f64).unwrap_or(0.0);
            let time = Instant::now();
            let cpu_percent = previous.map_or(0.0, |(at, usage)| ((cpu - usage).max(0.0) / at.elapsed().as_nanos() as f64 / vcpus) * 100.0);
            previous = Some((time, cpu));
            let (commands, command_p95, tick_lag) = games.take_metrics();
            let data = json!({ "timestamp": now_ms() / 1000, "cpuPercent": cpu_percent,
                "memoryPercent": (memory - cache).max(0.0) / (memory_mib * 1024.0 * 1024.0) * 100.0,
                "commandP95Ms": command_p95, "commands": commands, "tickLagMs": tick_lag, "loadedGames": games.loaded_games().await });
            db.put_item().table_name(&table).item("game_id", A::S(format!("SERVER#{server_id}")))
                .item("kind", A::S("METRICS".into())).item("data", A::S(data.to_string())).send().await?;
            Ok::<_, anyhow::Error>(())
        }.await;
        if let Err(error) = sampled {
            tracing::warn!(error=%error, "runtime telemetry deferred");
        }
    }
}
