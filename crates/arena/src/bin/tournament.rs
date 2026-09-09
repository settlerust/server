//! Paired-seed engine tournament. Every seed is played once per lineup rotation so each policy
//! slot sees each seat on the same board. Output is one JSON line per game.
use settlers_arena::{GameRecord, SeatSpec, TableSettings, build_seat, play};
use settlers_engine::Settings;
use std::io::Write;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

fn usage() -> ! {
    eprintln!(
        "usage: tournament --seat SPEC [--seat SPEC ...] --seeds A-B [--rotations N] [--threads N] \
         [--points N] [--max-turns N] [--out FILE] [--trace]\n  SPEC: fast | eta | legal | v1 | v2 | v1:{{json}} | v2:{{json}}"
    );
    std::process::exit(2)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut seats = Vec::new();
    let mut seeds = (0u32, 0u32);
    let mut rotations: Option<u32> = None;
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut points = 10u8;
    let mut max_turns = 500u32;
    let mut out: Option<String> = None;
    let mut trace = false;
    let mut i = 0;
    while i < args.len() {
        let value = |i: usize| args.get(i + 1).cloned().unwrap_or_else(|| usage());
        match args[i].as_str() {
            "--seat" => seats.push(SeatSpec::parse(&value(i)).unwrap_or_else(|e| {
                eprintln!("{e}");
                usage()
            })),
            "--seeds" => {
                let text = value(i);
                let (a, b) = text.split_once('-').unwrap_or((&text, &text));
                seeds = (a.parse()?, b.parse()?);
            }
            "--rotations" => rotations = Some(value(i).parse()?),
            "--threads" => threads = value(i).parse()?,
            "--points" => points = value(i).parse()?,
            "--max-turns" => max_turns = value(i).parse()?,
            "--out" => out = Some(value(i)),
            "--trace" => {
                trace = true;
                i -= 1;
            }
            _ => usage(),
        }
        i += 2;
    }
    if !(2..=4).contains(&seats.len()) || seeds.1 < seeds.0 {
        usage();
    }
    let players = seats.len() as u8;
    let rotations = rotations
        .unwrap_or(u32::from(players))
        .clamp(1, u32::from(players));
    let jobs: Vec<(u32, u32)> = (seeds.0..=seeds.1)
        .flat_map(|seed| (0..rotations).map(move |r| (seed, r)))
        .collect();
    let next = AtomicUsize::new(0);
    let writer: Mutex<Box<dyn Write + Send>> = Mutex::new(match &out {
        Some(path) => Box::new(std::io::BufWriter::new(std::fs::File::create(path)?)),
        None => Box::new(std::io::stdout()),
    });
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<(u32, u32, GameRecord)>> = Mutex::new(Vec::new());
    let started = std::time::Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(seed, rotation)) = jobs.get(index) else {
                        break;
                    };
                    // Seat s holds lineup slot (s + rotation) mod players.
                    let lineup: Vec<_> = (0..players)
                        .map(|seat| {
                            build_seat(&seats[((seat + rotation as u8) % players) as usize])
                        })
                        .collect::<Result<_, _>>()
                        .expect("seat specs were validated");
                    let table = TableSettings {
                        settings: Settings {
                            players,
                            points_to_win: points,
                            ..Settings::default()
                        },
                        seed,
                        max_turns,
                        trace,
                        ..TableSettings::default()
                    };
                    let record = play(lineup, table).unwrap_or_else(|e| {
                        eprintln!("seed {seed} rotation {rotation}: {e}");
                        std::process::exit(1)
                    });
                    let slots: Vec<u8> = (0..players)
                        .map(|seat| (seat + rotation as u8) % players)
                        .collect();
                    let winner_slot = record.winner.map(|w| slots[w as usize]);
                    let line = serde_json::json!({
                        "seed": seed, "rotation": rotation, "slots": slots,
                        "winner_slot": winner_slot, "game": record,
                    });
                    let mut w = writer.lock().unwrap();
                    writeln!(w, "{line}").unwrap();
                    w.flush().unwrap();
                    drop(w);
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(20) || n == jobs.len() {
                        eprintln!(
                            "{n}/{} games, {:.1}s",
                            jobs.len(),
                            started.elapsed().as_secs_f64()
                        );
                    }
                    results.lock().unwrap().push((seed, rotation, record));
                }
            });
        }
    });
    let results = results.into_inner().unwrap();
    let mut wins = vec![0u32; players as usize];
    let mut games = vec![0u32; players as usize];
    let mut total_points = vec![0f64; players as usize];
    let mut ms = vec![0f64; players as usize];
    let mut decisions = vec![0u64; players as usize];
    let mut completed = 0;
    for (_, rotation, record) in &results {
        if record.status != "completed" {
            continue;
        }
        completed += 1;
        for seat in 0..players {
            let slot = ((seat + *rotation as u8) % players) as usize;
            games[slot] += 1;
            total_points[slot] += f64::from(record.seats[seat as usize].points);
            ms[slot] += record.seats[seat as usize].decision_ms;
            decisions[slot] += u64::from(record.seats[seat as usize].decisions);
            if record.winner == Some(seat) {
                wins[slot] += 1;
            }
        }
    }
    eprintln!(
        "{completed}/{} completed games in {:.1}s ({} threads)",
        results.len(),
        started.elapsed().as_secs_f64(),
        threads
    );
    for slot in 0..players as usize {
        eprintln!(
            "slot {slot} {:<28} wins {:>4}/{:<4} {:>5.1}%  mean points {:.2}  mean decision {:.2} ms",
            seats[slot].label(),
            wins[slot],
            games[slot],
            100.0 * f64::from(wins[slot]) / f64::from(games[slot].max(1)),
            total_points[slot] / f64::from(games[slot].max(1)),
            ms[slot] / decisions[slot].max(1) as f64
        );
    }
    Ok(())
}
