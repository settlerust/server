use server::{
    model::{Record, ReplayVersion},
    reducer,
};
use settlers_engine::Phase;
fn records() -> Vec<Record> {
    include_str!("fixtures/base-v9-game.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
#[test]
fn permanent_base_v9_fixture_preserves_events_and_final_hash() {
    let records = records();
    assert_eq!(records.len(), 29);
    assert!(
        records
            .iter()
            .all(|r| r.engine_version == ReplayVersion::BaseV9)
    );
    let room = reducer::replay(&records).unwrap();
    assert!(matches!(room.game.unwrap().phase, Phase::Finished { .. }));
    assert_eq!(
        reducer::state_hash(&room).unwrap(),
        "2a2cfbbeafb9c0937548643602cc4a00d113597892ee72d14bebe988289acc8e"
    );
}
#[test]
fn replay_rejects_unknown_versions_and_corrupt_records() {
    for version in (1..=8)
        .map(|v| format!("base-v{v}"))
        .chain(["base-v10".into(), "unknown".into()])
    {
        let mut json = serde_json::to_value(&records()[0]).unwrap();
        json["engine_version"] = version.into();
        assert!(serde_json::from_value::<Record>(json).is_err());
    }
    let source = records();
    let mut gap = source.clone();
    gap.remove(3);
    let mut wrong_game = source.clone();
    wrong_game[1].game_id = "another game".into();
    let mut hash = source.clone();
    hash[2].state_hash = "different".into();
    let mut public = source.clone();
    public[0].public.clear();
    let mut schema = source.clone();
    schema[0].schema = 2;
    for bad in [
        gap,
        wrong_game,
        hash,
        public,
        schema,
        source[1..].to_vec(),
        vec![],
    ] {
        assert!(reducer::replay(&bad).is_err());
    }
    assert_eq!(
        serde_json::to_string(&ReplayVersion::default()).unwrap(),
        "\"base-v9\""
    );
}
