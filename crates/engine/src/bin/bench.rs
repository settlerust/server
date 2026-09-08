use settlers_engine::*;
use std::{hint::black_box, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut state = State::new(Settings::default(), [17; 32])?;
    while state.turn == 0 {
        state = timeout(&state)?.0;
    }
    let iterations = 500_000;
    let started = Instant::now();
    for _ in 0..iterations {
        black_box(apply_with_chance(
            black_box(&state),
            state.current,
            Action::Roll,
            Chance::Dice {
                first: 1,
                second: 1,
            },
        )?);
    }
    let roll_ns = started.elapsed().as_nanos() as f64 / iterations as f64;
    let mut checksum = 0u64;
    let reads = 5_000_000;
    let started = Instant::now();
    for i in 0..reads {
        checksum += black_box(&state.board)
            .road((i % 72) as u8)
            .map_or(0, |p| p + 1) as u64;
    }
    black_box(checksum);
    let packed_ns = started.elapsed().as_nanos() as f64 / reads as f64;
    let unpacked: [u8; 72] =
        std::array::from_fn(|i| state.board.road(i as u8).map_or(0, |p| p + 1));
    let started = Instant::now();
    for i in 0..reads {
        checksum += black_box(&unpacked)[i % 72] as u64;
    }
    black_box(checksum);
    let unpacked_ns = started.elapsed().as_nanos() as f64 / reads as f64;
    println!(
        "board_bytes={} state_bytes={} explicit_roll_ns={roll_ns:.2} packed_road_read_ns={packed_ns:.2} byte_road_read_ns={unpacked_ns:.2}",
        std::mem::size_of::<Board>(),
        std::mem::size_of::<State>()
    );
    Ok(())
}
