//! Setup uses the draft planner because pre-production roll sampling adds no useful information.
use super::{SearchConfig, opening};
use crate::search::{Candidate, Contributions};
use settlers_engine::{Action, Observation, Phase, State};
use std::collections::BTreeMap;
pub(super) fn rank(
    root: &State,
    o: &Observation,
    config: &SearchConfig,
    seat: u8,
) -> Vec<Candidate> {
    let bias = opening::Bias {
        expansion: config.inclinations.expansion,
        development: config.inclinations.development,
        weights: config.opening_weights,
    };
    let mut candidates: Vec<Candidate> = Vec::new();
    match root.phase {
        Phase::SetupSettlement { .. } => {
            for plan in opening::rank_settlements(root, seat, 0, &bias) {
                let action = Action::BuildSettlement {
                    vertex: plan.vertex,
                };
                if !o.legal_actions.contains(&action) {
                    continue;
                }
                let mut line = vec![action];
                if let Some(partner) = plan.partner
                    && partner != plan.vertex
                    && root.board.owner(partner).is_none()
                {
                    line.push(Action::BuildSettlement { vertex: partner });
                }
                candidates.push(Candidate {
                    id: String::new(),
                    command: Some(settlers_protocol::Command::Act { action }),
                    expected_value: plan.value,
                    minimum_leaf_value: plan.value,
                    maximum_leaf_value: plan.value,
                    contributions: Contributions(
                        plan.terms
                            .named()
                            .into_iter()
                            .map(|(k, v)| (k.to_string(), v))
                            .collect(),
                    ),
                    trade: None,
                    principal_variation: line,
                    note: Some(opening::describe(&plan, &root.layout)),
                    speech: None,
                });
            }
        }
        Phase::SetupRoad { vertex, .. } => {
            for (edge, value, target) in opening::rank_roads(root, seat, vertex) {
                let action = Action::BuildRoad { edge };
                if !o.legal_actions.contains(&action) {
                    continue;
                }
                candidates.push(Candidate {
                    id: String::new(),
                    command: Some(settlers_protocol::Command::Act { action }),
                    expected_value: value,
                    minimum_leaf_value: value,
                    maximum_leaf_value: value,
                    contributions: Contributions(BTreeMap::from([(
                        "expansion".to_string(),
                        value,
                    )])),
                    trade: None,
                    principal_variation: vec![action],
                    note: target.map(|w| format!("road toward site {w}")),
                    speech: None,
                });
            }
        }
        _ => {}
    }
    candidates
}
