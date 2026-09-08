use settlers_engine::*;
use settlers_policies::{Policy, eta_builder::EtaBuilder, fast_builder::FastBuilder};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let count = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(10);
    let start = std::time::Instant::now();
    let mut actions = 0u64;
    for seed in 0..count {
        let mut bytes = [0; 32];
        bytes[..4].copy_from_slice(&seed.to_le_bytes());
        let mut s = State::new(Settings::default(), bytes)?;
        let mut policies: [Box<dyn Policy>; 4] = [
            Box::new(FastBuilder),
            Box::new(EtaBuilder),
            Box::new(FastBuilder),
            Box::new(EtaBuilder),
        ];
        for _ in 0..20000 {
            if matches!(s.phase, Phase::Finished { .. }) {
                break;
            }
            let p = if let Phase::Discard { pending } = s.phase {
                pending.trailing_zeros() as u8
            } else {
                s.current
            };
            let action = policies[p as usize]
                .choose_action(&observe(&s, Some(p)))
                .ok_or("policy stalled")?;
            s = apply(&s, p, action)?;
            actions += 1;
        }
        if !matches!(s.phase, Phase::Finished { .. }) {
            return Err(format!(
                "seed {seed} did not finish: {:?}",
                s.players
                    .map(|p| (p.settlements, p.cities, p.roads, p.resources))
            )
            .into());
        }
        println!(
            "{}",
            serde_json::json!({"seed":seed,"turns":s.turn,"phase":s.phase})
        );
    }
    eprintln!(
        "{count} games, {actions} actions, {:.2}s, board={}B state={}B",
        start.elapsed().as_secs_f64(),
        std::mem::size_of::<Board>(),
        std::mem::size_of::<State>()
    );
    Ok(())
}
