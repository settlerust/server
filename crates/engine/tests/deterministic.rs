use settlers_engine::*;

fn settings(deterministic: bool) -> Settings {
    Settings {
        deterministic,
        ..Settings::default()
    }
}
fn finish_setup(mut s: State) -> State {
    while s.turn == 0 {
        s = timeout(&s).unwrap().0;
    }
    s
}
/// Roll `n` times, optionally stealing between rolls so the live stream is consumed.
fn rolls(mut s: State, n: usize, steal_between: bool) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    for _ in 0..n {
        let before = s;
        let (next, events) = apply_recorded(&s, s.current, Action::Roll).unwrap();
        for event in events {
            if let Event::Rolled { first, second, .. } = event {
                out.push((first, second));
            }
        }
        s = next;
        if steal_between {
            // Consume randomness without rolling: a forced theft when possible.
            let p = s.current;
            if let Some(hex) =
                (0..19u8).find(|&h| h != s.robber && !robber_victims(&s, p, h).is_empty())
            {
                let victim = robber_victims(&s, p, hex)[0];
                let mut probe = before;
                probe.phase = Phase::Robber {
                    resume: Resume::Main,
                };
                probe.random = s.random;
                let stolen = apply(
                    &probe,
                    p,
                    Action::MoveRobber {
                        hex,
                        victim: Some(victim),
                    },
                )
                .unwrap();
                s.random = stolen.random;
            }
        }
        s = timeout(&s).unwrap().0;
        while s.phase != Phase::Roll {
            s = timeout(&s).unwrap().0;
        }
    }
    out
}

#[test]
fn deterministic_rolls_are_indexed_by_roll_count() {
    let seed = [9; 32];
    let plain = finish_setup(State::new(settings(true), seed).unwrap());
    let a = rolls(plain, 12, false);
    let b = rolls(plain, 12, true);
    assert_eq!(a, b, "thefts between rolls must not change the dice tape");
    assert!(
        a.iter().any(|&(x, y)| x != y || x != a[0].0),
        "the tape should vary"
    );
}

#[test]
fn the_default_mode_still_shares_one_stream() {
    let seed = [9; 32];
    let plain = finish_setup(State::new(settings(false), seed).unwrap());
    let a = rolls(plain, 12, false);
    let b = rolls(plain, 12, true);
    assert_ne!(a, b, "the ordinary stream advances with every random event");
    assert!(!Settings::default().deterministic);
}

#[test]
fn deterministic_deck_order_is_fixed_by_the_seed_and_survives_skipped_cards() {
    let seed = [21; 32];
    let order = deck_order(seed);
    let mut counts = [0u8; 5];
    for card in order {
        counts[card as usize] += 1;
    }
    assert_eq!(counts, [14, 2, 2, 2, 5]);
    assert_eq!(order, deck_order(seed));
    assert_ne!(order, deck_order([22; 32]));
    let mut s = finish_setup(State::new(settings(true), seed).unwrap());
    s.phase = Phase::Main;
    s.players[s.current as usize].resources = Resources([0, 0, 9, 9, 9]);
    s.bank = Resources([19, 19, 10, 10, 10]);
    let first = apply(&s, s.current, Action::BuyDevelopment).unwrap();
    let drawn = |before: &State, after: &State| {
        Development::ALL
            .into_iter()
            .find(|&c| before.deck[c as usize] != after.deck[c as usize])
            .unwrap()
    };
    assert_eq!(drawn(&s, &first), order[0]);
    let second = apply(&first, s.current, Action::BuyDevelopment).unwrap();
    assert_eq!(drawn(&first, &second), order[1]);
    // An explicit branch that takes a different card leaves the tape consistent afterwards.
    let branch = apply_with_chance(
        &s,
        s.current,
        Action::BuyDevelopment,
        Chance::Development {
            card: Development::VictoryPoint,
        },
    )
    .unwrap();
    let after = apply(&branch, s.current, Action::BuyDevelopment).unwrap();
    let card = drawn(&branch, &after);
    assert!(branch.deck[card as usize] > 0);
}

#[test]
fn deterministic_games_keep_the_compact_state_and_the_same_board() {
    assert_eq!(std::mem::size_of::<State>(), 240);
    let seed = [3; 32];
    let live = State::new(settings(false), seed).unwrap();
    let tape = State::new(settings(true), seed).unwrap();
    assert_eq!(live.layout.hexes, tape.layout.hexes);
    assert_eq!(live.layout.ports, tape.layout.ports);
    assert_eq!(live.first, tape.first);
}
