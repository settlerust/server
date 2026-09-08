use settlers_engine::*;
use settlers_policies::{Policy, eta_builder::EtaBuilder, fast_builder::FastBuilder};
#[test]
fn mixed_policies_finish_standard_games_for_two_three_and_four_players() {
    for players in 2..=4 {
        for seed in 0..4 {
            let mut s = State::new(
                Settings {
                    players,
                    ..Settings::default()
                },
                [seed; 32],
            )
            .unwrap();
            let mut fast = FastBuilder;
            let mut eta = EtaBuilder;
            for _ in 0..15000 {
                if matches!(s.phase, Phase::Finished { .. }) {
                    break;
                }
                let p = if let Phase::Discard { pending } = s.phase {
                    pending.trailing_zeros() as u8
                } else {
                    s.current
                };
                let o = observe(&s, Some(p));
                let a = if p % 2 == 0 {
                    fast.choose_action(&o)
                } else {
                    eta.choose_action(&o)
                }
                .expect("policy has action");
                s = apply(&s, p, a).unwrap();
                for r in 0..5 {
                    assert_eq!(
                        s.bank.0[r] as u16
                            + s.players
                                .iter()
                                .map(|p| p.resources.0[r] as u16)
                                .sum::<u16>(),
                        19
                    );
                }
            }
            assert!(
                matches!(s.phase, Phase::Finished { .. }),
                "players={players}, seed={seed}, turn={}",
                s.turn
            );
        }
    }
}
