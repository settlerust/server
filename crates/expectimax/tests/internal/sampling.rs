use super::{random, worlds};
use settlers_engine::*;
use settlers_protocol::{EventBatch, PublicEvent, knowledge::ResourceKnowledge};

#[test]
fn sampled_hands_respect_public_bank_totals_and_bounds_after_hidden_thefts() {
    for seed in 0..128 {
        let mut rng = random(seed);
        let mut state = State::new(Settings::default(), [17; 32]).unwrap();
        state.phase = Phase::Main;
        state.turn = 1;
        state.current = 0;
        for r in 0..5 {
            let dealt = if rng.below(2) == 0 {
                19
            } else {
                rng.below(20) as u8
            };
            for _ in 0..dealt {
                state.players[rng.below(4) as usize].resources.0[r] += 1;
            }
            state.bank.0[r] -= dealt;
        }
        let mut events = vec![PublicEvent::Started];
        for player in 0..4 {
            events.push(PublicEvent::Rule {
                event: Event::Produced {
                    player,
                    resources: state.players[player as usize].resources,
                },
            });
        }
        for _ in 0..12 {
            let victim = rng.below(4) as u8;
            let player = (victim + 1 + rng.below(3) as u8) % 4;
            let available: Vec<_> = (0..5)
                .filter(|&r| state.players[victim as usize].resources.0[r] > 0)
                .collect();
            if available.is_empty() {
                continue;
            }
            let r = available[rng.below(available.len() as u32) as usize];
            state.players[victim as usize].resources.0[r] -= 1;
            state.players[player as usize].resources.0[r] += 1;
            events.push(PublicEvent::Rule {
                event: Event::Stole {
                    player,
                    victim,
                    count: 1,
                },
            });
        }
        let history = vec![EventBatch {
            sequence: 1,
            timestamp_ms: 0,
            events,
        }];
        let observation = observe(&state, Some(0));
        let knowledge = ResourceKnowledge::from_history_at_version(&observation, &history, 1);
        for sampled in worlds(&observation, &knowledge, 8)
            .unwrap_or_else(|error| panic!("seed {seed}: {error}"))
        {
            for p in 0..4 {
                let estimate = &knowledge.players[p];
                let hand = sampled.players[p].resources;
                assert_eq!(hand.total(), estimate.total);
                for r in 0..5 {
                    assert!((estimate.minimum[r]..=estimate.maximum[r]).contains(&hand.0[r]));
                }
            }
            assert_eq!(sampled.players[0].resources, state.players[0].resources);
            assert_eq!(sampled.bank, observation.bank);
            for r in 0..5 {
                assert_eq!(sampled.bank.0[r] > 0, observation.bank_available[r]);
                assert_eq!(
                    u16::from(sampled.bank.0[r])
                        + sampled
                            .players
                            .iter()
                            .map(|p| u16::from(p.resources.0[r]))
                            .sum::<u16>(),
                    19
                );
            }
        }
    }
}
