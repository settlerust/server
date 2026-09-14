mod common;
use settlers_expectimax::{
    ExpectimaxPlayer,
    ntuple::{Layout, Model},
    v2::{Cache, Leaf, SearchConfig},
};
use std::sync::Arc;

#[test]
fn browser_player_uses_the_installed_frozen_leaf_and_matches_native_search() {
    let (state, history) = common::fixture(2);
    let view = common::view(&state, state.current, history.len() as u64);
    let model = Arc::new(Model::new(
        Layout::parse("hex,portfolio").unwrap(),
        0.3,
        false,
    ));
    let mut bytes = Vec::new();
    model.write_to(&mut bytes, false).unwrap();
    let search = SearchConfig {
        depth: 1,
        samples: 1,
        scenarios: 1,
        afterstates: 8,
        leaf: settlers_expectimax::v2::LeafConfig {
            tables: Some("fixture".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let request = serde_json::json!({ "view": view, "search": search }).to_string();
    let mut player = ExpectimaxPlayer::new();
    for batch in &history {
        player
            .on_events(&serde_json::to_string(batch).unwrap())
            .unwrap();
    }
    assert!(
        player
            .analyze(&request)
            .unwrap_err()
            .contains("have not been loaded")
    );
    player.load_tables("fixture", &bytes).unwrap();
    let actual: serde_json::Value =
        serde_json::from_str(&player.analyze(&request).unwrap()).unwrap();
    let expected = settlers_expectimax::v2::analyze_with(
        &view,
        &history,
        &search,
        None,
        &|| 0.0,
        &Leaf::tables(model, 1000.0, 0.0),
        &mut Cache::default(),
    )
    .unwrap();
    // Timings differ; the complete ranked decisions and learned contributions must agree.
    assert_eq!(
        actual["candidates"],
        serde_json::to_value(expected.candidates).unwrap()
    );
    assert!(
        actual["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|candidate| candidate["contributions"].to_string().contains("tables"))
    );
}

#[test]
fn browser_player_rejects_invalid_table_bytes() {
    assert!(
        ExpectimaxPlayer::new()
            .load_tables("broken", b"not a model")
            .is_err()
    );
}
