mod common;
use common::*;
use settlers_engine::*;
#[test]
fn seven_discards_half_rounded_down_then_requires_robber() {
    let mut s = started();
    hand(&mut s, 0, Resources([9, 0, 0, 0, 0]));
    hand(&mut s, 1, Resources([0, 7, 0, 0, 0]));
    let s = apply_with_chance(
        &s,
        0,
        Action::Roll,
        Chance::Dice {
            first: 3,
            second: 4,
        },
    )
    .unwrap();
    assert_eq!(s.phase, Phase::Discard { pending: 1 });
    assert_eq!(
        apply(
            &s,
            0,
            Action::Discard {
                resources: Resources([5, 0, 0, 0, 0])
            }
        ),
        Err(RuleError::InvalidDiscard)
    );
    let next = apply(
        &s,
        0,
        Action::Discard {
            resources: Resources([4, 0, 0, 0, 0]),
        },
    )
    .unwrap();
    assert_eq!(
        next.phase,
        Phase::Robber {
            resume: Resume::Main
        }
    );
    conservation(&next);
}
#[test]
fn multiple_discards_can_arrive_in_any_order() {
    let mut s = started();
    hand(&mut s, 0, Resources([8, 0, 0, 0, 0]));
    hand(&mut s, 1, Resources([0, 8, 0, 0, 0]));
    let s = apply_with_chance(
        &s,
        0,
        Action::Roll,
        Chance::Dice {
            first: 1,
            second: 6,
        },
    )
    .unwrap();
    let a = Action::Discard {
        resources: Resources([4, 0, 0, 0, 0]),
    };
    let b = Action::Discard {
        resources: Resources([0, 4, 0, 0, 0]),
    };
    assert_eq!(
        apply(&apply(&s, 0, a).unwrap(), 1, b),
        apply(&apply(&s, 1, b).unwrap(), 0, a)
    );
}
#[test]
fn robber_requires_eligible_victim_and_steals_only_one_available_card() {
    let mut s = main_phase();
    s.phase = Phase::Robber {
        resume: Resume::Main,
    };
    let h = (0..19).find(|&h| h != s.robber).unwrap();
    s.board = board(&[(topology().hex_vertices[h as usize][0], 2)], &[]);
    hand(&mut s, 1, Resources([0, 0, 1, 0, 0]));
    assert_eq!(
        apply(
            &s,
            0,
            Action::MoveRobber {
                hex: h,
                victim: None
            }
        ),
        Err(RuleError::InvalidVictim)
    );
    assert_eq!(
        apply(
            &s,
            0,
            Action::MoveRobber {
                hex: s.robber,
                victim: None
            }
        ),
        Err(RuleError::InvalidLocation)
    );
    let n = apply_with_chance(
        &s,
        0,
        Action::MoveRobber {
            hex: h,
            victim: Some(1),
        },
        Chance::Steal {
            resource: Resource::Wool,
        },
    )
    .unwrap();
    assert_eq!(n.players[1].resources.total(), 0);
    assert_eq!(n.players[0].resources.0[2], s.players[0].resources.0[2] + 1);
}
#[test]
fn purchased_development_waits_until_next_turn_but_vp_wins_immediately() {
    let mut s = main_phase();
    hand(&mut s, 0, Resources::DEVELOPMENT);
    let n = apply_with_chance(
        &s,
        0,
        Action::BuyDevelopment,
        Chance::Development {
            card: Development::Knight,
        },
    )
    .unwrap();
    assert_eq!(
        apply(&n, 0, Action::PlayKnight),
        Err(RuleError::CannotPlayDevelopment)
    );
    let ended = apply(&n, 0, Action::EndTurn).unwrap();
    assert_eq!(ended.players[0].development[0], 1);
    assert_eq!(ended.players[0].new_development[0], 0);
    s.settings.points_to_win = 3;
    let n = apply_with_chance(
        &s,
        0,
        Action::BuyDevelopment,
        Chance::Development {
            card: Development::VictoryPoint,
        },
    )
    .unwrap();
    assert_eq!(n.phase, Phase::Finished { winner: 0 });
}
#[test]
fn development_before_roll_resumes_roll_and_only_one_can_be_played() {
    let mut s = started();
    dev(&mut s, 0, Development::Knight);
    dev(&mut s, 0, Development::Monopoly);
    let n = apply(&s, 0, Action::PlayKnight).unwrap();
    let a = legal_actions(&n, 0)[0];
    let n = apply(&n, 0, a).unwrap();
    assert_eq!(n.phase, Phase::Roll);
    assert_eq!(
        apply(
            &n,
            0,
            Action::PlayMonopoly {
                resource: Resource::Ore
            }
        ),
        Err(RuleError::CannotPlayDevelopment)
    );
}
#[test]
fn monopoly_transfers_all_of_chosen_resource() {
    let mut s = main_phase();
    dev(&mut s, 0, Development::Monopoly);
    hand(&mut s, 1, Resources([0, 0, 0, 0, 3]));
    hand(&mut s, 2, Resources([0, 0, 0, 0, 4]));
    let original = s.players[0].resources.0[4];
    let n = apply(
        &s,
        0,
        Action::PlayMonopoly {
            resource: Resource::Ore,
        },
    )
    .unwrap();
    assert_eq!(
        n.players[0].resources.0[4],
        original + 7 + s.players[3].resources.0[4]
    );
    assert_eq!(n.players[1].resources.0[4], 0);
    conservation(&n);
}
#[test]
fn plenty_takes_two_and_respects_shortage() {
    let mut s = main_phase();
    dev(&mut s, 0, Development::YearOfPlenty);
    let n = apply(
        &s,
        0,
        Action::PlayYearOfPlenty {
            resources: Resources([0, 0, 0, 0, 2]),
        },
    )
    .unwrap();
    assert_eq!(n.players[0].resources.0[4], s.players[0].resources.0[4] + 2);
    s.bank = Resources([1, 0, 0, 0, 0]);
    assert!(
        apply(
            &s,
            0,
            Action::PlayYearOfPlenty {
                resources: Resources([1, 0, 0, 0, 0])
            }
        )
        .is_ok()
    );
    assert_eq!(
        apply(
            &s,
            0,
            Action::PlayYearOfPlenty {
                resources: Resources([2, 0, 0, 0, 0])
            }
        ),
        Err(RuleError::BankShortage)
    );
}
#[test]
fn road_building_places_two_free_roads_and_resumes_phase() {
    let mut s = started();
    dev(&mut s, 0, Development::RoadBuilding);
    let hand = s.players[0].resources;
    let n = apply(&s, 0, Action::PlayRoadBuilding).unwrap();
    let n = apply(&n, 0, legal_actions(&n, 0)[0]).unwrap();
    let n = apply(&n, 0, legal_actions(&n, 0)[0]).unwrap();
    assert_eq!(n.phase, Phase::Roll);
    assert_eq!(n.players[0].roads, 4);
    assert_eq!(n.players[0].resources, hand);
    conservation(&n);
}
#[test]
fn largest_army_requires_three_and_holder_keeps_tie() {
    let mut s = main_phase();
    s.players[0].played_knights = 2;
    dev(&mut s, 0, Development::Knight);
    let n = apply(&s, 0, Action::PlayKnight).unwrap();
    assert_eq!(n.largest_army, Some(0));
    let mut s = n;
    s.phase = Phase::Main;
    s.current = 1;
    s.played_development = false;
    s.players[1].played_knights = 2;
    dev(&mut s, 1, Development::Knight);
    let n = apply(&s, 1, Action::PlayKnight).unwrap();
    assert_eq!(n.largest_army, Some(0));
}

#[test]
fn road_building_with_one_piece_left_finishes_after_one_road() {
    let mut s = started();
    s.players[0].roads = 14;
    dev(&mut s, 0, Development::RoadBuilding);
    let played = apply(&s, 0, Action::PlayRoadBuilding).unwrap();
    let n = apply(&played, 0, legal_actions(&played, 0)[0]).unwrap();
    assert_eq!(n.players[0].roads, 15);
    assert_eq!(n.phase, Phase::Roll);
    assert_eq!(n.players[0].resources, s.players[0].resources);
    let mut exhausted = s;
    exhausted.players[0].roads = 15;
    assert_eq!(
        apply(&exhausted, 0, Action::PlayRoadBuilding),
        Err(RuleError::NoPieces)
    );
}

#[test]
fn optional_second_free_road_can_be_declined_without_returning_card() {
    let mut s = started();
    dev(&mut s, 0, Development::RoadBuilding);
    let played = apply(&s, 0, Action::PlayRoadBuilding).unwrap();
    assert_eq!(
        apply(&played, 0, Action::FinishRoadBuilding),
        Err(RuleError::WrongPhase)
    );
    let first = apply(&played, 0, legal_actions(&played, 0)[0]).unwrap();
    assert!(legal_actions(&first, 0).contains(&Action::FinishRoadBuilding));
    let finished = apply(&first, 0, Action::FinishRoadBuilding).unwrap();
    assert_eq!(finished.phase, Phase::Roll);
    assert_eq!(finished.players[0].roads, s.players[0].roads + 1);
    assert_eq!(finished.players[0].resources, s.players[0].resources);
    assert!(finished.played_development);
    conservation(&finished);
}
