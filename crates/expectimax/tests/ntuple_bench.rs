#[allow(dead_code)]
mod common;
use settlers_engine::*;
use settlers_expectimax::ntuple::{Layout, MAX_ACTIVE, Model};
use settlers_expectimax::v2::{Weights, leaf::evaluate, opponent};
use std::hash::{Hash, Hasher};

/// A few hundred mid-game positions from builder play on several seeds. The builder opens
/// with the opening planner, so a change to the planner changes these positions and the
/// digest below; a pure change to feature extraction must leave the digest unchanged.
fn positions() -> Vec<State> {
    let mut out = Vec::new();
    for seed in 0..10u8 {
        let mut s = State::new(Settings::default(), [seed; 32]).unwrap();
        let mut steps = 0;
        while !matches!(s.phase, Phase::Finished { .. }) && steps < 1500 {
            let p = match s.phase {
                Phase::Discard { pending } => pending.trailing_zeros() as u8,
                _ => s.current,
            };
            let a = opponent::act(&s, p, 0);
            s = apply(&s, p, a)
                .unwrap_or_else(|_| apply(&s, p, *legal_actions(&s, p).first().unwrap()).unwrap());
            steps += 1;
            if steps % 7 == 0 {
                out.push(s);
            }
        }
    }
    out
}

/// The feature indices of every position for every seat, hashed. Any change to pattern
/// extraction that alters a frozen table's meaning changes this value; a pure speed-up
/// must leave it unchanged.
#[test]
fn feature_extraction_is_stable() {
    let model = Model::new(Layout::parse("hex,vertex,portfolio").unwrap(), 0.0, false);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut scratch = [0u32; MAX_ACTIVE];
    let states = positions();
    assert!(states.len() > 300);
    for s in &states {
        for seat in 0..4 {
            let n = model.features(s, seat, &mut scratch);
            scratch[..n].hash(&mut h);
        }
    }
    let digest = h.finish();
    eprintln!("feature digest {digest:#x} over {} positions", states.len());
    assert_eq!(digest, 0x45dc_9381_441c_d238, "feature digest changed");
}

#[test]
#[ignore]
fn feature_extraction_cost() {
    let model = Model::new(Layout::parse("hex,portfolio").unwrap(), 0.0, false);
    let states = positions();
    let w = Weights::default();
    let rounds = 20;
    let started = std::time::Instant::now();
    let mut acc = 0.0f32;
    for _ in 0..rounds {
        for s in &states {
            acc += model.value(s, 1);
        }
    }
    let tables = started.elapsed().as_secs_f64() / (rounds * states.len()) as f64;
    let started = std::time::Instant::now();
    let mut acc2 = 0.0f64;
    for _ in 0..rounds {
        for s in &states {
            acc2 += evaluate(s, 1, &w).total();
        }
    }
    let hand = started.elapsed().as_secs_f64() / (rounds * states.len()) as f64;
    eprintln!(
        "tables {:.2} us, hand leaf {:.2} us per evaluation ({} positions; {acc} {acc2})",
        tables * 1e6,
        hand * 1e6,
        states.len()
    );
}
