//! Game-tree structure measurements. Plays builder games to sample positions, then records the
//! legal-action width of every decision by phase and, for sampled positions, the afterstates,
//! transitions, leaves, and time of the tunable search at several depths.
use settlers_arena::{SeatSpec, TableSettings, build_seat, play};
use settlers_engine::*;
use settlers_expectimax::v2::{SearchConfig, analyze};
use settlers_protocol::{EventBatch, GameView, MemberView, PublicEvent};
use std::io::Write;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::SetupSettlement { .. } => "setup_settlement",
        Phase::SetupRoad { .. } => "setup_road",
        Phase::Roll => "roll",
        Phase::Discard { .. } => "discard",
        Phase::Robber { .. } => "robber",
        Phase::Main => "main",
        Phase::FreeRoads { .. } => "free_roads",
        Phase::Finished { .. } => "finished",
    }
}
fn actor(s: &State) -> u8 {
    if let Phase::Discard { pending } = s.phase {
        pending.trailing_zeros() as u8
    } else {
        s.current
    }
}
fn view(s: &State, seat: u8, version: u64) -> GameView {
    GameView {
        id: "structure".into(),
        name: "structure".into(),
        research: false,
        research_hands: None,
        version,
        settings: s.settings,
        host: false,
        members: (0..s.settings.players)
            .map(|i| MemberView {
                seat: Some(i),
                name: format!("seat {i}"),
                connected: true,
                ready: true,
                is_you: i == seat,
            })
            .collect(),
        game: Some(observe(s, Some(seat))),
        opening: None,
        offers: vec![],
        filters: None,
        deadline_ms: None,
        discard: None,
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut seeds = (0u32, 15u32);
    let mut every = 7u32;
    let mut depths: Vec<u8> = vec![1, 2, 3];
    let mut time_budget = 0.0;
    let mut out: Option<String> = None;
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut i = 0;
    while i < args.len() {
        let value = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--seeds" => {
                let (a, b) = value.split_once('-').unwrap_or((&value, &value));
                seeds = (a.parse()?, b.parse()?);
            }
            "--every" => every = value.parse()?,
            "--depths" => {
                depths = value
                    .split(',')
                    .map(|d| d.parse())
                    .collect::<Result<_, _>>()?
            }
            "--time" => time_budget = value.parse()?,
            "--threads" => threads = value.parse()?,
            "--out" => out = Some(value),
            _ => {
                eprintln!(
                    "usage: structure --seeds A-B [--every N] [--depths 1,2,3] [--time MS] [--threads N] --out FILE"
                );
                std::process::exit(2)
            }
        }
        i += 2;
    }
    let writer: Mutex<Box<dyn Write + Send>> = Mutex::new(match &out {
        Some(path) => Box::new(std::io::BufWriter::new(std::fs::File::create(path)?)),
        None => Box::new(std::io::stdout()),
    });
    let next = AtomicUsize::new(0);
    let jobs: Vec<u32> = (seeds.0..=seeds.1).collect();
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&seed) = jobs.get(index) else { break };
                    // Replay a builder game locally to sample positions with full history.
                    let mut bytes = [0u8; 32];
                    bytes[..4].copy_from_slice(&seed.to_le_bytes());
                    let mut s = State::new(Settings::default(), bytes).expect("settings");
                    let mut histories: Vec<Vec<EventBatch>> = vec![
                        vec![EventBatch { sequence: 1, timestamp_ms: 0, events: vec![PublicEvent::Started] }];
                        4
                    ];
                    let mut seats: Vec<_> = ["fast", "eta", "fast", "eta"]
                        .iter()
                        .map(|k| build_seat(&SeatSpec::parse(k).unwrap()).unwrap())
                        .collect();
                    let mut version = 1u64;
                    let mut decision = 0u32;
                    let mut lines = Vec::new();
                    while !matches!(s.phase, Phase::Finished { .. }) && s.turn < 400 {
                        let p = actor(&s);
                        let v = view(&s, p, version);
                        let o = v.game.as_ref().unwrap();
                        let legal = o.legal_actions.len();
                        lines.push(serde_json::json!({
                            "kind": "decision", "seed": seed, "turn": s.turn, "phase": phase_name(s.phase),
                            "seat": p, "legal_actions": legal, "hand": o.own.map(|h| h.resources.total()),
                        }));
                        if decision.is_multiple_of(every) && s.turn > 0 && legal > 1 {
                            for &depth in &depths {
                                let config = SearchConfig { depth, time_budget_ms: time_budget, iterative: false, ..SearchConfig::default() };
                                let started = std::time::Instant::now();
                                let clock = || started.elapsed().as_secs_f64() * 1000.0;
                                if let Ok(report) = analyze(&v, &histories[p as usize], &config, None, &clock) {
                                    let st = &report.statistics;
                                    lines.push(serde_json::json!({
                                        "kind": "search", "seed": seed, "turn": s.turn, "phase": phase_name(s.phase),
                                        "seat": p, "depth": depth, "depth_completed": st.depth_completed,
                                        "aborted": st.aborted, "legal_actions": legal, "root_actions": st.root_actions,
                                        "candidates": report.candidates.len(), "transitions": st.transitions,
                                        "leaves": st.leaves, "afterstates": st.afterstates, "expanded": st.expanded,
                                        "transpositions": st.transpositions, "scenarios": st.scenarios,
                                        "own_nodes": st.own_nodes, "per_level_afterstates": st.per_level_afterstates,
                                        "per_level_nodes": st.per_level_nodes, "time_ms": st.time_ms,
                                    }));
                                }
                            }
                        }
                        decision += 1;
                        let command = seats[p as usize].choose(&v, &histories[p as usize]);
                        let action = match command {
                            Some(settlers_protocol::Command::Act { action }) => action,
                            _ => match s.phase {
                                Phase::Main => Action::EndTurn,
                                Phase::Roll => Action::Roll,
                                _ => *legal_actions(&s, p).first().expect("legal"),
                            },
                        };
                        let (nexts, events) = match apply_recorded(&s, p, action) {
                            Ok(x) => x,
                            Err(_) => {
                                let fallback = *legal_actions(&s, p).first().expect("legal");
                                apply_recorded(&s, p, fallback).expect("fallback")
                            }
                        };
                        s = nexts;
                        version += 1;
                        for history in &mut histories {
                            history.push(EventBatch {
                                sequence: version,
                                timestamp_ms: 0,
                                events: events.iter().map(|e| PublicEvent::Rule { event: e.clone() }).collect(),
                            });
                        }
                    }
                    let mut w = writer.lock().unwrap();
                    for line in lines {
                        writeln!(w, "{line}").unwrap();
                    }
                }
            });
        }
    });
    let _ = TableSettings::default();
    let _ = play;
    Ok(())
}
