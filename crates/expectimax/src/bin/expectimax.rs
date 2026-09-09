use serde::Deserialize;
use settlers_expectimax::{Config, analyze};
use settlers_protocol::{EventBatch, GameView};
use std::io::{self, Read};

#[derive(Deserialize)]
struct Input {
    view: GameView,
    #[serde(default)]
    history: Vec<EventBatch>,
    #[serde(default)]
    config: Config,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let input: Input = serde_json::from_str(&input)?;
    let start = std::time::Instant::now();
    let report = analyze(&input.view, &input.history, input.config, None)?;
    eprintln!(
        "expectimax_ms={:.3}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
