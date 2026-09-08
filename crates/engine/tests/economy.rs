mod common;
use common::*;
use settlers_engine::*;
fn production_fixture() -> (State, u8) {
    let mut s = main_phase();
    s.phase = Phase::Roll;
    s.robber = s.layout.desert();
    let h = (0..19).find(|&h| s.layout.resource(h).is_some()).unwrap();
    let vs = topology().hex_vertices[h as usize];
    s.board = board(&[(vs[0], 5), (vs[3], 2)], &[]);
    (s, h)
}
#[test]
fn production_pays_settlements_and_cities() {
    let (s, h) = production_fixture();
    let total = s.layout.number(h);
    let d = (1..=6).find(|&a| (1..=6).contains(&(total - a))).unwrap();
    let next = apply_with_chance(
        &s,
        0,
        Action::Roll,
        Chance::Dice {
            first: d,
            second: total - d,
        },
    )
    .unwrap();
    let r = s.layout.resource(h).unwrap() as usize;
    assert!(next.players[0].resources.0[r] >= s.players[0].resources.0[r] + 2);
    assert!(next.players[1].resources.0[r] > s.players[1].resources.0[r]);
}
#[test]
fn shortage_with_multiple_recipients_pays_nobody() {
    let (mut s, h) = production_fixture();
    let r = s.layout.resource(h).unwrap() as usize;
    s.bank.0[r] = 1;
    let total = s.layout.number(h);
    let d = (1..=6).find(|&a| (1..=6).contains(&(total - a))).unwrap();
    let next = apply_with_chance(
        &s,
        0,
        Action::Roll,
        Chance::Dice {
            first: d,
            second: total - d,
        },
    )
    .unwrap();
    assert_eq!(next.players[0].resources.0[r], s.players[0].resources.0[r]);
    assert_eq!(next.players[1].resources.0[r], s.players[1].resources.0[r]);
}
#[test]
fn shortage_with_one_recipient_pays_remaining_supply() {
    let (mut s, h) = production_fixture();
    let v = topology().hex_vertices[h as usize][0];
    s.board = board(&[(v, 5)], &[]);
    let r = s.layout.resource(h).unwrap() as usize;
    s.bank.0[r] = 1;
    let total = s.layout.number(h);
    let d = (1..=6).find(|&a| (1..=6).contains(&(total - a))).unwrap();
    let next = apply_with_chance(
        &s,
        0,
        Action::Roll,
        Chance::Dice {
            first: d,
            second: total - d,
        },
    )
    .unwrap();
    assert_eq!(
        next.players[0].resources.0[r],
        s.players[0].resources.0[r] + 1
    );
    assert_eq!(next.bank.0[r], 0);
}
#[test]
fn robber_blocks_production() {
    let (mut s, h) = production_fixture();
    s.robber = h;
    for other in 0..19 {
        if other != h && s.layout.number(other) == s.layout.number(h) {
            s.layout.hexes[other as usize] &= 15;
        }
    }
    let total = s.layout.number(h);
    let d = (1..=6).find(|&a| (1..=6).contains(&(total - a))).unwrap();
    let next = apply_with_chance(
        &s,
        0,
        Action::Roll,
        Chance::Dice {
            first: d,
            second: total - d,
        },
    )
    .unwrap();
    assert_eq!(next.players, s.players);
}
#[test]
fn bank_trade_uses_best_port_and_forbids_gifts_and_like_resources() {
    let mut s = main_phase();
    hand(&mut s, 0, Resources([8, 0, 0, 0, 0]));
    s.board = board(&[], &[]);
    let give = Resources([4, 0, 0, 0, 0]);
    let receive = Resources([0, 1, 0, 0, 0]);
    let n = apply(&s, 0, Action::BankTrade { give, receive }).unwrap();
    assert_eq!(n.players[0].resources, Resources([4, 1, 0, 0, 0]));
    assert_eq!(
        apply(
            &s,
            0,
            Action::BankTrade {
                give,
                receive: give
            }
        ),
        Err(RuleError::InvalidTrade)
    );
    let [v, _] = topology().ports[0];
    s.board = board(&[(v, 1)], &[]);
    s.layout.ports[0] = 0;
    assert_eq!(trade_ratio(&s, 0, Resource::Brick), 3);
    s.layout.ports[0] = 1;
    assert_eq!(trade_ratio(&s, 0, Resource::Brick), 2);
}
#[test]
fn domestic_trade_requires_current_player_and_owned_nonoverlapping_cards() {
    let mut s = main_phase();
    hand(&mut s, 0, Resources([1, 0, 0, 0, 0]));
    hand(&mut s, 1, Resources([0, 1, 0, 0, 0]));
    let a = Action::Trade {
        other: 0,
        give: Resources([0, 1, 0, 0, 0]),
        receive: Resources([1, 0, 0, 0, 0]),
    };
    let n = apply(&s, 1, a).unwrap();
    assert_eq!(n.players[0].resources, Resources([0, 1, 0, 0, 0]));
    conservation(&n);
    assert_eq!(
        apply(
            &s,
            2,
            Action::Trade {
                other: 1,
                give: Resources([1, 0, 0, 0, 0]),
                receive: Resources([0, 1, 0, 0, 0])
            }
        ),
        Err(RuleError::InvalidTrade)
    );
}
