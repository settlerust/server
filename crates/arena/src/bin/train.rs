//! Self-play temporal-difference training of the n-tuple leaf.
//!
//! Workers play complete games on shared tables: every seat plans its turn with the tunable
//! search at depth 1 (every first action, best line by static value, exact chance branches)
//! with the tables as the leaf, on the true state. The leaf reads only that seat's own
//! information, so the tables never learn from hidden hands. At game end each seat's chain of
//! end-of-turn afterstates is updated backward with TD(lambda) toward 1 for the winner and 0
//! for everyone else (a capped game counts as 0 for all), with temporal-coherence step sizes.
//!
//! OUTPUT (under --out): config.json, progress.jsonl (one row per chunk), val-<games>.json
//! (a validation tournament of the frozen tables as the depth-D leaf against the hand-written
//! leaf in the same search with the fixed builders, plus the tables played directly at depth
//! 1), best.bin and best.json (the validation point with the largest paired margin),
//! latest.bin, checkpoint.bin (weights plus accumulators, for --resume), stop.json, DONE.
//!
//! STOPPING: the game budget, the wall budget, a file named STOP in --out, or the plateau
//! rule (window W: the mean margin of the last W validation points is not above the mean of
//! the W before them, once at least --plateau-min-points points exist).
use settlers_arena::seats::search_seat;
use settlers_arena::{SeatSpec, TableSettings, build_seat, play};
use settlers_engine::*;
use settlers_expectimax::ntuple::{Layout, MAX_ACTIVE, Model, UpdateStats};
use settlers_expectimax::v2::{Leaf, LeafConfig, SearchConfig, opponent, rank_state};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Config {
    layout: String,
    out: PathBuf,
    threads: usize,
    games: u64,
    chunk_games: u64,
    wall_seconds: u64,
    alpha: f32,
    lambda: f32,
    optimistic: f32,
    clamp: f32,
    epsilon: f64,
    point_reward: f32,
    warmup_games: u64,
    builder_mix: f64,
    seeds_start: u32,
    seeds_count: u32,
    train_seed: u32,
    turn_cap: u32,
    validate_start: u32,
    validate_seeds: u32,
    validate_depth: u8,
    validate_every: u64,
    validate_direct: bool,
    resume: bool,
    checkpoint: bool,
    plateau_window: usize,
    plateau_min_points: usize,
    scale: f64,
}
fn parse_args() -> Result<Config, String> {
    let mut c = Config {
        layout: "hex,vertex,portfolio".into(),
        out: PathBuf::new(),
        threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        games: u64::MAX,
        chunk_games: 2_000,
        wall_seconds: 0,
        alpha: 1.0,
        lambda: 0.7,
        optimistic: 0.5,
        clamp: 1.0,
        epsilon: 0.0,
        point_reward: 0.05,
        warmup_games: 20_000,
        builder_mix: 0.0,
        seeds_start: 1_000_000,
        seeds_count: 1_000_000,
        train_seed: 0x5e77_1e25,
        turn_cap: 240,
        validate_start: 500_000,
        validate_seeds: 32,
        validate_depth: 2,
        validate_every: 1,
        validate_direct: true,
        resume: false,
        checkpoint: true,
        plateau_window: 0,
        plateau_min_points: 6,
        scale: 1000.0,
    };
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--resume" => {
                c.resume = true;
                i += 1;
                continue;
            }
            "--no-checkpoint" => {
                c.checkpoint = false;
                i += 1;
                continue;
            }
            "--no-direct" => {
                c.validate_direct = false;
                i += 1;
                continue;
            }
            _ => {}
        }
        let value = args
            .get(i + 1)
            .map(|s| s.as_str())
            .ok_or(format!("{flag} needs a value"))?;
        let bad = |what: &str| format!("bad {what}: {value}");
        match flag {
            "--layout" => c.layout = value.to_string(),
            "--out" => c.out = PathBuf::from(value),
            "--threads" => c.threads = value.parse().map_err(|_| bad(flag))?,
            "--games" => c.games = value.parse().map_err(|_| bad(flag))?,
            "--chunk-games" => c.chunk_games = value.parse().map_err(|_| bad(flag))?,
            "--wall-seconds" => c.wall_seconds = value.parse().map_err(|_| bad(flag))?,
            "--alpha" => c.alpha = value.parse().map_err(|_| bad(flag))?,
            "--lambda" => c.lambda = value.parse().map_err(|_| bad(flag))?,
            "--optimistic" => c.optimistic = value.parse().map_err(|_| bad(flag))?,
            "--clamp" => c.clamp = value.parse().map_err(|_| bad(flag))?,
            "--epsilon" => c.epsilon = value.parse().map_err(|_| bad(flag))?,
            "--point-reward" => c.point_reward = value.parse().map_err(|_| bad(flag))?,
            "--warmup-games" => c.warmup_games = value.parse().map_err(|_| bad(flag))?,
            "--builder-mix" => c.builder_mix = value.parse().map_err(|_| bad(flag))?,
            "--seeds-start" => c.seeds_start = value.parse().map_err(|_| bad(flag))?,
            "--seeds-count" => c.seeds_count = value.parse().map_err(|_| bad(flag))?,
            "--train-seed" => c.train_seed = value.parse().map_err(|_| bad(flag))?,
            "--turn-cap" => c.turn_cap = value.parse().map_err(|_| bad(flag))?,
            "--validate-start" => c.validate_start = value.parse().map_err(|_| bad(flag))?,
            "--validate-seeds" => c.validate_seeds = value.parse().map_err(|_| bad(flag))?,
            "--validate-depth" => c.validate_depth = value.parse().map_err(|_| bad(flag))?,
            "--validate-every" => c.validate_every = value.parse().map_err(|_| bad(flag))?,
            "--plateau-window" => c.plateau_window = value.parse().map_err(|_| bad(flag))?,
            "--plateau-min-points" => {
                c.plateau_min_points = value.parse().map_err(|_| bad(flag))?
            }
            "--scale" => c.scale = value.parse().map_err(|_| bad(flag))?,
            other => return Err(format!("unknown argument {other}")),
        }
        i += 2;
    }
    if c.out.as_os_str().is_empty() {
        return Err("--out is required".into());
    }
    if c.seeds_count == 0 || c.chunk_games == 0 {
        return Err("--seeds-count and --chunk-games must be positive".into());
    }
    if !(0.0..=1.0).contains(&c.lambda) {
        return Err("--lambda must be in [0, 1]".into());
    }
    Ok(c)
}
fn config_json(c: &Config, layout: &Layout) -> serde_json::Value {
    serde_json::json!({
        "format": "settlers-ntuple-train-config-v1",
        "layout": layout.spec(), "entries": layout.total_entries(), "active_per_position": layout.active_count(),
        "threads": c.threads, "games": c.games, "chunk_games": c.chunk_games, "wall_seconds": c.wall_seconds,
        "alpha": c.alpha, "lambda": c.lambda, "optimistic": c.optimistic, "clamp": c.clamp,
        "epsilon": c.epsilon, "point_reward": c.point_reward, "warmup_games": c.warmup_games, "builder_mix": c.builder_mix,
        "seeds_start": c.seeds_start, "seeds_count": c.seeds_count, "train_seed": c.train_seed, "turn_cap": c.turn_cap,
        "validate_start": c.validate_start, "validate_seeds": c.validate_seeds, "validate_depth": c.validate_depth,
        "validate_every": c.validate_every, "validate_direct": c.validate_direct, "scale": c.scale,
        "plateau_window": c.plateau_window, "plateau_min_points": c.plateau_min_points, "checkpoint": c.checkpoint,
        "target": "win indicator (1 winner, 0 otherwise; capped games 0 for all) plus point_reward per point gained",
        "policy": "expectimax-v2 turn planner at depth 1 with the tables as the leaf, on the true state; no proposals; the first warmup_games are played by the builder for every seat, and builder_mix is the per-decision probability of a builder move afterwards",
    })
}

/// Small deterministic generator for exploration and tie-breaking (SplitMix64).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}
fn actor(s: &State) -> u8 {
    if let Phase::Discard { pending } = s.phase {
        pending.trailing_zeros() as u8
    } else {
        s.current
    }
}
#[derive(Default, Clone)]
struct GameStats {
    games: u64,
    builder_games: u64,
    capped: u64,
    turns: u64,
    decisions: u64,
    searched: u64,
    afterstates: u64,
    winner_points: u64,
    updates: UpdateStats,
    wins: [u64; 4],
}
/// One seat's chain of afterstates: feature indices, own points at each, and the prediction.
struct Chain {
    features: Vec<u32>,
    points: Vec<u8>,
}
#[allow(clippy::too_many_arguments)]
fn play_training_game(
    model: &Model,
    leaf: &Leaf,
    search: &SearchConfig,
    c: &Config,
    seed: u32,
    builder: bool,
    rng: &mut Rng,
    stats: &mut GameStats,
) {
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&seed.to_le_bytes());
    let settings = Settings::default();
    let Ok(mut s) = State::new(settings, bytes) else {
        return;
    };
    let players = settings.players as usize;
    let n = model.layout.active_count();
    let mut chains: Vec<Chain> = (0..players)
        .map(|_| Chain {
            features: Vec::with_capacity(n * 80),
            points: Vec::with_capacity(80),
        })
        .collect();
    let mut pending = [false; 4];
    let mut scratch = [0u32; MAX_ACTIVE];
    let mut steps = 0u32;
    let mut turn_key = (u32::MAX, 0u8);
    let mut turn_actions = 0u32;
    let capped = loop {
        if matches!(s.phase, Phase::Finished { .. }) {
            break false;
        }
        if s.turn > c.turn_cap || steps > 20_000 {
            break true;
        }
        if (s.turn, s.current) != turn_key {
            turn_key = (s.turn, s.current);
            turn_actions = 0;
        }
        let p = actor(&s);
        stats.decisions += 1;
        let action = if turn_actions >= 40 && s.phase == Phase::Main {
            Action::EndTurn
        } else if builder || (c.builder_mix > 0.0 && rng.unit() < c.builder_mix) {
            opponent::act(&s, p, turn_actions)
        } else {
            match rank_state(&s, p, search, leaf, 1) {
                Ok(ranked) if !ranked.is_empty() => {
                    if ranked.len() > 1 {
                        stats.searched += 1;
                    }
                    if c.epsilon > 0.0 && rng.unit() < c.epsilon {
                        ranked[rng.below(ranked.len())].0
                    } else {
                        let best = ranked.iter().map(|r| r.1).fold(f64::NEG_INFINITY, f64::max);
                        let ties: Vec<Action> = ranked
                            .iter()
                            .filter(|r| r.1 >= best - 1e-9)
                            .map(|r| r.0)
                            .collect();
                        ties[rng.below(ties.len())]
                    }
                }
                _ => match s.phase {
                    Phase::Main => Action::EndTurn,
                    Phase::Roll => Action::Roll,
                    Phase::Discard { .. } => Action::Discard {
                        resources: automatic_discard(s.players[p as usize].resources),
                    },
                    _ => match legal_actions(&s, p).first() {
                        Some(&a) => a,
                        None => break true,
                    },
                },
            }
        };
        let next = match apply(&s, p, action) {
            Ok(next) => next,
            Err(_) => {
                let fallback = match s.phase {
                    Phase::Main => Action::EndTurn,
                    Phase::Roll => Action::Roll,
                    Phase::Discard { .. } => Action::Discard {
                        resources: automatic_discard(s.players[p as usize].resources),
                    },
                    _ => match legal_actions(&s, p).first() {
                        Some(&a) => a,
                        None => break true,
                    },
                };
                match apply(&s, p, fallback) {
                    Ok(next) => next,
                    Err(_) => break true,
                }
            }
        };
        s = next;
        steps += 1;
        turn_actions += 1;
        pending[p as usize] = true;
        if !matches!(s.phase, Phase::Discard { .. }) {
            let finished = matches!(s.phase, Phase::Finished { .. });
            let now = actor(&s);
            for q in 0..players {
                if !pending[q] {
                    continue;
                }
                pending[q] = false;
                if finished || now == q as u8 {
                    continue;
                }
                let count = model.features(&s, q as u8, &mut scratch);
                chains[q].features.extend_from_slice(&scratch[..count]);
                chains[q].points.push(points(&s, q as u8));
                stats.afterstates += 1;
            }
        }
    };
    let winner = match s.phase {
        Phase::Finished { winner } if !capped => Some(winner),
        _ => None,
    };
    stats.games += 1;
    stats.builder_games += u64::from(builder);
    stats.capped += u64::from(capped);
    stats.turns += u64::from(s.turn);
    if let Some(w) = winner {
        stats.wins[w as usize] += 1;
        stats.winner_points += u64::from(points(&s, w));
    }
    // Backward TD(lambda) over each seat's afterstate chain.
    for (q, chain) in chains.iter().enumerate() {
        let steps = chain.points.len();
        if steps == 0 {
            continue;
        }
        let z: f32 = if winner == Some(q as u8) { 1.0 } else { 0.0 };
        let final_points = points(&s, q as u8);
        let values: Vec<f32> = (0..steps)
            .map(|t| model.value_of(&chain.features[t * n..(t + 1) * n]))
            .collect();
        let mut next_return = z;
        for t in (0..steps).rev() {
            let later = if t + 1 < steps {
                chain.points[t + 1]
            } else {
                final_points
            };
            let reward = c.point_reward * f32::from(later.saturating_sub(chain.points[t]));
            let bootstrap = if t + 1 < steps {
                (1.0 - c.lambda) * values[t + 1] + c.lambda * next_return
            } else {
                z
            };
            let target = reward + bootstrap;
            model.update(
                &chain.features[t * n..(t + 1) * n],
                target,
                c.alpha,
                c.clamp,
                &mut stats.updates,
            );
            next_return = target;
        }
    }
}

struct Shared {
    games: AtomicU64,
    cursor: AtomicU64,
    stop: AtomicBool,
}
fn worker(
    model: &Arc<Model>,
    c: &Config,
    shared: &Shared,
    chunk_end: u64,
    worker_index: usize,
    chunk_index: u64,
) -> GameStats {
    // The leaf reads the live tables through the shared allocation; updates are visible to
    // every worker as they happen.
    let leaf = Leaf::tables(model.clone(), c.scale, 0.0);
    let search = SearchConfig {
        depth: 1,
        propose: false,
        iterative: false,
        ..SearchConfig::default()
    };
    let mut rng = Rng(u64::from(c.train_seed)
        ^ (worker_index as u64) << 32
        ^ chunk_index.wrapping_mul(0x9e37));
    let mut stats = GameStats::default();
    while shared.games.load(Relaxed) < chunk_end && !shared.stop.load(Relaxed) {
        let index = shared.cursor.fetch_add(1, Relaxed);
        let seed = c
            .seeds_start
            .wrapping_add((index % u64::from(c.seeds_count)) as u32);
        let builder = index < c.warmup_games;
        play_training_game(
            model, &leaf, &search, c, seed, builder, &mut rng, &mut stats,
        );
        shared.games.fetch_add(1, Relaxed);
    }
    stats
}

#[derive(Clone, serde::Serialize)]
struct Validation {
    games_trained: u64,
    depth: u8,
    seeds: u32,
    tables_wins: u32,
    hand_wins: u32,
    eta_wins: u32,
    fast_wins: u32,
    completed: u32,
    tables_points: f64,
    hand_points: f64,
    tables_ms: f64,
    hand_ms: f64,
    margin: f64,
    margin_interval: [f64; 2],
    direct_wins: Option<u32>,
    direct_margin: Option<f64>,
    fingerprint: String,
    touched: u64,
    is_best: bool,
}
type SeatFactory = Box<dyn Fn() -> Result<Box<dyn settlers_arena::Seat>, String> + Sync>;
struct Arm {
    wins: [u32; 4],
    points: [f64; 4],
    ms: [f64; 4],
    decisions: [u64; 4],
    completed: u32,
    per_seed: Vec<f64>,
}
/// Play a lineup on the validation seeds with every rotation; slot 0 minus slot 1 per seed.
fn tournament(lineup: &[SeatFactory], c: &Config) -> Arm {
    let players = lineup.len() as u8;
    let jobs: Vec<(u32, u32)> = (0..c.validate_seeds)
        .flat_map(|k| (0..u32::from(players)).map(move |r| (c.validate_start + k, r)))
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Mutex<Vec<(u32, u32, settlers_arena::GameRecord)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..c.threads.max(1) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Relaxed);
                    let Some(&(seed, rotation)) = jobs.get(index) else {
                        break;
                    };
                    let seats: Vec<_> = (0..players)
                        .map(|seat| {
                            lineup[((seat + rotation as u8) % players) as usize]()
                                .expect("validation seat")
                        })
                        .collect();
                    let table = TableSettings {
                        settings: Settings {
                            deterministic: true,
                            ..Settings::default()
                        },
                        seed,
                        max_turns: 500,
                        ..TableSettings::default()
                    };
                    let record = play(seats, table).expect("validation game");
                    results.lock().unwrap().push((seed, rotation, record));
                }
            });
        }
    });
    let results = results.into_inner().unwrap();
    let mut arm = Arm {
        wins: [0; 4],
        points: [0.0; 4],
        ms: [0.0; 4],
        decisions: [0; 4],
        completed: 0,
        per_seed: Vec::new(),
    };
    let mut by_seed: std::collections::BTreeMap<u32, Vec<f64>> = Default::default();
    for (seed, rotation, record) in &results {
        if record.status != "completed" {
            continue;
        }
        arm.completed += 1;
        let mut diff = 0.0;
        for seat in 0..players {
            let slot = ((seat + *rotation as u8) % players) as usize;
            let r = &record.seats[seat as usize];
            arm.points[slot] += f64::from(r.points);
            arm.ms[slot] += r.decision_ms;
            arm.decisions[slot] += u64::from(r.decisions);
            if record.winner == Some(seat) {
                arm.wins[slot] += 1;
                if slot == 0 {
                    diff += 1.0;
                } else if slot == 1 {
                    diff -= 1.0;
                }
            }
        }
        by_seed.entry(*seed).or_default().push(diff);
    }
    arm.per_seed = by_seed
        .values()
        .map(|d| d.iter().sum::<f64>() / d.len() as f64)
        .collect();
    arm
}
fn mean_interval(values: &[f64]) -> (f64, [f64; 2]) {
    let n = values.len().max(1) as f64;
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0);
    let se = (var / n).sqrt();
    (mean, [mean - 1.96 * se, mean + 1.96 * se])
}
fn validate(
    model: &Model,
    c: &Config,
    games_trained: u64,
    is_best: &mut dyn FnMut(f64) -> bool,
) -> Validation {
    let frozen = Arc::new(model.snapshot());
    let fingerprint = format!("{:016x}", frozen.fingerprint());
    let tables_config = SearchConfig {
        depth: c.validate_depth,
        leaf: LeafConfig {
            tables: Some(format!("memory:{fingerprint}")),
            scale: c.scale,
            hand: 0.0,
        },
        ..SearchConfig::default()
    };
    let hand_config = SearchConfig {
        depth: c.validate_depth,
        ..SearchConfig::default()
    };
    let scale = c.scale;
    let f1 = frozen.clone();
    let tc = tables_config.clone();
    let hc = hand_config.clone();
    let lineup: Vec<SeatFactory> = vec![
        Box::new(move || {
            Ok(search_seat(
                tc.clone(),
                Leaf::tables(f1.clone(), scale, 0.0),
            ))
        }),
        Box::new(move || Ok(search_seat(hc.clone(), Leaf::Hand))),
        Box::new(|| build_seat(&SeatSpec::parse("eta")?)),
        Box::new(|| build_seat(&SeatSpec::parse("fast")?)),
    ];
    let arm = tournament(&lineup, c);
    let (margin, interval) = mean_interval(&arm.per_seed);
    let mut direct_wins = None;
    let mut direct_margin = None;
    if c.validate_direct {
        let f2 = frozen;
        let dc = SearchConfig {
            depth: 1,
            ..tables_config
        };
        let hc2 = hand_config;
        let direct: Vec<SeatFactory> = vec![
            Box::new(move || {
                Ok(search_seat(
                    dc.clone(),
                    Leaf::tables(f2.clone(), scale, 0.0),
                ))
            }),
            Box::new(move || Ok(search_seat(hc2.clone(), Leaf::Hand))),
            Box::new(|| build_seat(&SeatSpec::parse("eta")?)),
            Box::new(|| build_seat(&SeatSpec::parse("fast")?)),
        ];
        let d = tournament(&direct, c);
        direct_wins = Some(d.wins[0]);
        direct_margin = Some(mean_interval(&d.per_seed).0);
    }
    let per = |i: usize| arm.points[i] / f64::from(arm.completed.max(1));
    let ms = |i: usize| arm.ms[i] / arm.decisions[i].max(1) as f64;
    Validation {
        games_trained,
        depth: c.validate_depth,
        seeds: c.validate_seeds,
        tables_wins: arm.wins[0],
        hand_wins: arm.wins[1],
        eta_wins: arm.wins[2],
        fast_wins: arm.wins[3],
        completed: arm.completed,
        tables_points: per(0),
        hand_points: per(1),
        tables_ms: ms(0),
        hand_ms: ms(1),
        margin,
        margin_interval: interval,
        direct_wins,
        direct_margin,
        fingerprint,
        touched: model.touched_entries(),
        is_best: is_best(margin),
    }
}
fn plateau(margins: &[f64], window: usize, min_points: usize) -> Option<(f64, f64, bool)> {
    if window == 0 || margins.len() < min_points || margins.len() < 2 * window {
        return None;
    }
    let k = margins.len();
    let recent = margins[k - window..].iter().sum::<f64>() / window as f64;
    let previous = margins[k - 2 * window..k - window].iter().sum::<f64>() / window as f64;
    Some((recent, previous, recent <= previous))
}
fn write_json(path: &std::path::Path, value: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let c = parse_args()?;
    let layout = Layout::parse(&c.layout)?;
    std::fs::create_dir_all(&c.out).map_err(|e| e.to_string())?;
    let config_path = c.out.join("config.json");
    let config_value = config_json(&c, &layout);
    if config_path.exists() {
        let existing: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&config_path).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut a = existing;
        let mut b = config_value;
        for key in ["threads", "games", "wall_seconds"] {
            a[key] = serde_json::Value::Null;
            b[key] = serde_json::Value::Null;
        }
        if a != b {
            return Err(
                "config.json differs from this invocation; a run's configuration is fixed".into(),
            );
        }
    } else {
        write_json(&config_path, &config_value)?;
    }
    let started = Instant::now();
    eprintln!(
        "layout {}: {} entries ({:.1} MB weights, {:.1} MB with accumulators), {} active per position, {} threads",
        layout.spec(),
        layout.total_entries(),
        layout.total_entries() as f64 * 4.0 / 1e6,
        layout.total_entries() as f64 * 12.0 / 1e6,
        layout.active_count(),
        c.threads
    );
    let progress_path = c.out.join("progress.jsonl");
    let checkpoint_path = c.out.join("checkpoint.bin");
    let mut chunk_index = 0u64;
    let mut games_total = 0u64;
    let mut wall_offset = 0.0f64;
    let mut cursor_start = 0u64;
    let mut best_margin = f64::NEG_INFINITY;
    let mut margins: Vec<f64> = Vec::new();
    let model = if c.resume && checkpoint_path.exists() {
        let text = std::fs::read_to_string(&progress_path).unwrap_or_default();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let row: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
            chunk_index = row["chunk"].as_u64().unwrap_or(0) + 1;
            games_total = row["games_total"].as_u64().unwrap_or(0);
            wall_offset = row["wall_seconds"].as_f64().unwrap_or(0.0);
            cursor_start = row["seed_cursor"].as_u64().unwrap_or(0);
            if let Some(m) = row["validation"]["margin"].as_f64() {
                margins.push(m);
                best_margin = best_margin.max(m);
            }
        }
        eprintln!("resuming at chunk {chunk_index}, {games_total} games");
        Arc::new(Model::load(&checkpoint_path, true)?)
    } else {
        Arc::new(Model::new(layout, c.optimistic, true))
    };
    let shared = Shared {
        games: AtomicU64::new(games_total),
        cursor: AtomicU64::new(cursor_start),
        stop: AtomicBool::new(false),
    };
    let mut progress = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&progress_path)
        .map_err(|e| e.to_string())?;
    let stop_reason;
    loop {
        if games_total >= c.games {
            stop_reason = "games";
            break;
        }
        if c.wall_seconds > 0
            && wall_offset + started.elapsed().as_secs_f64() >= c.wall_seconds as f64
        {
            stop_reason = "wall";
            break;
        }
        if c.out.join("STOP").exists() {
            stop_reason = "stop-file";
            break;
        }
        let chunk_end = games_total + c.chunk_games;
        let chunk_started = Instant::now();
        let stats: Vec<GameStats> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..c.threads.max(1))
                .map(|w| {
                    let model = &model;
                    let c = &c;
                    let shared = &shared;
                    scope.spawn(move || worker(model, c, shared, chunk_end, w, chunk_index))
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("worker"))
                .collect()
        });
        let mut total = GameStats::default();
        for s in stats {
            total.games += s.games;
            total.builder_games += s.builder_games;
            total.capped += s.capped;
            total.turns += s.turns;
            total.decisions += s.decisions;
            total.searched += s.searched;
            total.afterstates += s.afterstates;
            total.winner_points += s.winner_points;
            total.updates.updates += s.updates.updates;
            total.updates.entry_updates += s.updates.entry_updates;
            total.updates.abs_delta_sum += s.updates.abs_delta_sum;
            total.updates.beta_sum += s.updates.beta_sum;
            for i in 0..4 {
                total.wins[i] += s.wins[i];
            }
        }
        games_total = shared.games.load(Relaxed);
        let chunk_seconds = chunk_started.elapsed().as_secs_f64();
        let wall = wall_offset + started.elapsed().as_secs_f64();
        let validation = if c.validate_every > 0
            && (chunk_index + 1).is_multiple_of(c.validate_every)
        {
            let v_started = Instant::now();
            let mut best = false;
            let v = validate(&model, &c, games_total, &mut |m| {
                best = m > best_margin;
                best
            });
            if best {
                best_margin = v.margin;
                model.snapshot().save(&c.out.join("best.bin"), false)?;
                write_json(
                    &c.out.join("best.json"),
                    &serde_json::json!({"games_total": games_total, "margin": v.margin, "fingerprint": v.fingerprint, "chunk": chunk_index}),
                )?;
            }
            margins.push(v.margin);
            write_json(
                &c.out.join(format!("val-{games_total}.json")),
                &serde_json::to_value(&v).map_err(|e| e.to_string())?,
            )?;
            eprintln!(
                "  validation at {games_total} games: tables {} hand {} eta {} fast {} of {} (margin {:+.3} [{:+.3}, {:+.3}]) direct {:?} in {:.0}s{}",
                v.tables_wins,
                v.hand_wins,
                v.eta_wins,
                v.fast_wins,
                v.completed,
                v.margin,
                v.margin_interval[0],
                v.margin_interval[1],
                v.direct_wins,
                v_started.elapsed().as_secs_f64(),
                if best { " best" } else { "" }
            );
            Some(v)
        } else {
            None
        };
        let g = total.games.max(1) as f64;
        let row = serde_json::json!({
            "chunk": chunk_index, "games": total.games, "games_total": games_total,
            "seed_cursor": shared.cursor.load(Relaxed), "wall_seconds": wall, "chunk_seconds": chunk_seconds,
            "games_per_second": total.games as f64 / chunk_seconds.max(1e-9),
            "capped": total.capped, "builder_games": total.builder_games, "mean_turns": total.turns as f64 / g,
            "decisions_per_game": total.decisions as f64 / g, "searched_per_game": total.searched as f64 / g,
            "afterstates_per_game": total.afterstates as f64 / g,
            "winner_points": total.winner_points as f64 / (total.games - total.capped).max(1) as f64,
            "wins_by_seat": total.wins,
            "mean_abs_delta": total.updates.abs_delta_sum / total.updates.updates.max(1) as f64,
            "mean_beta": total.updates.beta_sum / total.updates.entry_updates.max(1) as f64,
            "updates": total.updates.updates, "entry_updates": total.updates.entry_updates,
            "validation": validation,
        });
        writeln!(progress, "{row}").map_err(|e| e.to_string())?;
        progress.flush().map_err(|e| e.to_string())?;
        eprintln!(
            "chunk {chunk_index}: {} games ({} builder) in {:.0}s ({:.1}/s), capped {}, turns {:.1}, |delta| {:.3}, beta {:.3}, wall {:.0}s",
            total.games,
            total.builder_games,
            chunk_seconds,
            total.games as f64 / chunk_seconds.max(1e-9),
            total.capped,
            total.turns as f64 / g,
            row["mean_abs_delta"].as_f64().unwrap_or(0.0),
            row["mean_beta"].as_f64().unwrap_or(0.0),
            wall
        );
        if c.checkpoint {
            model.save(&checkpoint_path, true)?;
        }
        model.snapshot().save(&c.out.join("latest.bin"), false)?;
        chunk_index += 1;
        if let Some((recent, previous, stop)) =
            plateau(&margins, c.plateau_window, c.plateau_min_points)
            && stop
        {
            eprintln!("plateau: recent {recent:+.3} not above previous {previous:+.3}");
            stop_reason = "plateau";
            break;
        }
    }
    write_json(
        &c.out.join("stop.json"),
        &serde_json::json!({"reason": stop_reason, "games_total": games_total, "chunks": chunk_index,
            "wall_seconds": wall_offset + started.elapsed().as_secs_f64(), "best_margin": best_margin}),
    )?;
    std::fs::write(c.out.join("DONE"), b"").map_err(|e| e.to_string())?;
    eprintln!("stopped: {stop_reason} after {games_total} games");
    Ok(())
}
