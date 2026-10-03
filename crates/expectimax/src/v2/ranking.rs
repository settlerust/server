//! Offline ranking shares evaluation with live search without constructing participant reports.
use super::{
    Cache, CacheMode, Leaf, SearchConfig, root_action,
    search::{Search, World},
};
use crate::search::SearchError;
use settlers_engine::{Action, Phase, State, apply};
/// Rank the actor's candidate actions on a fully known state: the value of each action's
/// best line at the given depth under the leaf. Training and offline diagnostics only, never
/// seated play; the state's hidden hands reach the turn planner's chance branches but never
/// the leaf, which reads only the seat's own information.
pub fn rank_state(
    state: &State,
    seat: u8,
    config: &SearchConfig,
    leaf: &Leaf,
    depth: u8,
) -> Result<Vec<(Action, f64)>, SearchError> {
    let mut cache = Cache::default();
    rank_state_with(state, seat, config, leaf, depth, &mut cache)
}
/// `rank_state` with the caller's transposition tables, cleared according to `config.cache`.
pub fn rank_state_with(
    state: &State,
    seat: u8,
    config: &SearchConfig,
    leaf: &Leaf,
    depth: u8,
    cache: &mut Cache,
) -> Result<Vec<(Action, f64)>, SearchError> {
    let config = config.clone().bounded();
    let mut weights = config.inclinations.apply(config.weights);
    weights.endgame = config.endgame;
    let never = || false;
    match config.cache {
        CacheMode::None | CacheMode::Decision => cache.clear(),
        CacheMode::Turn => {
            if cache.owner != Some((state.turn, state.current)) {
                cache.clear();
                cache.owner = Some((state.turn, state.current));
            }
        }
    }
    let mut search = Search::new(
        seat,
        weights,
        config.limits(depth),
        &never,
        leaf.clone(),
        cache,
    );
    let mut actions = if matches!(state.phase, Phase::Discard { .. }) {
        Search::discard_candidates(state, seat, 4, config.discard_projection)
    } else {
        search.candidates(state, seat, 0)
    };
    actions.retain(|&a| apply(state, seat, a).is_ok());
    let worlds = vec![World {
        state: *state,
        weight: 1.0,
    }];
    let mut out = Vec::with_capacity(actions.len());
    for a in actions {
        let value = root_action(&mut search, &worlds, seat, a, depth)?;
        out.push((a, value.total()));
    }
    Ok(out)
}
