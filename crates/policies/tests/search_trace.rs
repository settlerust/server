use settlers_engine::Phase;
use settlers_expectimax::Statistics;
use settlers_policies::expectimax::search_trace;

#[test]
fn search_trace_names_the_phase_and_carries_the_depth_and_times() {
    let statistics = Statistics {
        depth_completed: 3,
        time_ms: 1234.5,
        aborted: true,
        legal_actions: 9,
        root_actions: 4,
        own_nodes: 77,
        transitions: 4096,
        ..Statistics::default()
    };
    let line = search_trace(
        "expectimax-v2-tables",
        42,
        17,
        &Phase::Robber {
            resume: settlers_engine::Resume::Main,
        },
        3,
        &statistics,
        1300.25,
    );
    let text = line.to_string();
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("one JSON line");
    assert_eq!(
        parsed,
        serde_json::json!({
            "trace": "search",
            "policy": "expectimax-v2-tables",
            "version": 42,
            "turn": 17,
            "phase": "robber",
            "legal_actions": 9,
            "root_actions": 4,
            "candidates": 3,
            "depth_completed": 3,
            "time_ms": 1234.5,
            "wall_ms": 1300.25,
            "aborted": true,
            "own_nodes": 77,
            "transitions": 4096,
        })
    );
}
