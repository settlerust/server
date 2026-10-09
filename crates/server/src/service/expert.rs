//! Built-in server policy receives only its own ordinary, redacted seat view.
use crate::{
    error::{Error, Result},
    model::Room,
    reducer,
};
use settlers_expectimax::v2::{Cache, CacheMode, Leaf, SearchConfig};
use settlers_protocol::*;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    sync::{Arc, OnceLock},
    time::Instant,
};
pub(super) type Decision = Result<(u8, u64, Option<Command>)>;
#[cfg(test)]
#[path = "../../tests/service/computer_wait.rs"]
mod tests;

fn awaiting_trade_response(view: &GameView, seat: u8) -> bool {
    view.game
        .as_ref()
        .is_some_and(|game| game.phase == Phase::Main)
        && view
            .offers
            .iter()
            .any(|offer| offer.from == seat && offer.status == OfferStatus::Open)
}

fn leaf() -> Result<Leaf> {
    static MODEL: OnceLock<std::result::Result<Arc<settlers_expectimax::ntuple::Model>, String>> =
        OnceLock::new();
    let model = MODEL
        .get_or_init(|| {
            let mut bytes = Vec::new();
            flate2::read::GzDecoder::new(include_bytes!("../../assets/expert.bin.gz").as_slice())
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if format!("{:x}", Sha256::digest(&bytes))
                != "f8e61e66e0279b0fe67f5f4d6be0ad3ca0a86617e91ed24fe08a34a997b46f0c"
            {
                return Err("Expert model digest mismatch".into());
            }
            settlers_expectimax::ntuple::Model::read_from(&mut bytes.as_slice(), false)
                .map(Arc::new)
        })
        .as_ref()
        .map_err(Error::storage)?;
    Ok(Leaf::tables(Arc::clone(model), 1000.0, 0.0))
}
pub(super) fn next_view(
    room: &Room,
    considered: &std::collections::HashMap<u8, u64>,
) -> Option<(u8, ComputerPolicy, GameView)> {
    let game = room.game.as_ref()?;
    if matches!(game.phase, Phase::Finished { .. }) {
        return None;
    }
    let mut active = None;
    for member in &room.members {
        let Some(seat) = member.seat else {
            continue;
        };
        let Some(policy) = member.policy() else {
            continue;
        };
        if considered.get(&seat) == Some(&room.version) {
            continue;
        }
        let mut own = room.clone();
        let own_member = own.members.iter_mut().find(|m| m.seat == Some(seat))?;
        own_member.resolution = None;
        own_member.computer_policy = None;
        let view = reducer::view(&own, &member.identity.subject).ok()?;
        // Trade responses are protocol commands, not engine legal actions. A recipient
        // must get a decision while the offerer's turn is still in progress.
        let incoming = view
            .offers
            .iter()
            .any(|offer| offer.status == OfferStatus::Open && offer.to.contains(&seat));
        if incoming {
            return Some((seat, policy, view));
        }
        if active.is_none()
            && !awaiting_trade_response(&view, seat)
            && view
                .game
                .as_ref()
                .is_some_and(|g| !g.legal_actions.is_empty())
        {
            active = Some((seat, policy, view));
        }
    }
    // Human responses arrive asynchronously. An open outgoing offer holds normal
    // moves until acceptance, all declines, or the persisted offer deadline.
    active
}
pub(super) fn choose(
    seat: u8,
    policy: ComputerPolicy,
    view: GameView,
    offered_turn: Option<u32>,
) -> Decision {
    let (depth, time_budget_ms, node_budget) = match policy {
        ComputerPolicy::Easy => (2, 1000.0, 400_000),
        ComputerPolicy::Medium | ComputerPolicy::Expert => (3, 4000.0, 1_200_000),
        ComputerPolicy::Hard => (4, 12000.0, 4_000_000),
    };
    let evaluator = if policy == ComputerPolicy::Expert {
        leaf()?
    } else {
        Leaf::Hand
    };
    let config = SearchConfig {
        depth,
        time_budget_ms,
        node_budget,
        cache: CacheMode::Decision,
        deepen_fraction: 1.0,
        ..SearchConfig::default()
    };
    let started = Instant::now();
    let report = settlers_expectimax::v2::analyze_with(
        &view,
        &[],
        &config,
        offered_turn,
        &|| started.elapsed().as_secs_f64() * 1000.0,
        &evaluator,
        &mut Cache::default(),
    )
    .map_err(Error::storage)?;
    let command = report
        .candidates
        .into_iter()
        .next()
        .ok_or_else(|| {
            Error::new(
                ErrorCode::Internal,
                "Built-in computer did not return a decision",
            )
        })?
        .command;
    // Incoming counteroffers still get evaluated while our offer is open, but a
    // preference for building/spending/end-turn must wait for its response window.
    let command =
        if awaiting_trade_response(&view, seat) && matches!(command, Some(Command::Act { .. })) {
            None
        } else {
            command
        };
    // Waiting can outrank accepting a trade. Do not skip it for a lower-ranked command.
    Ok((seat, view.version, command))
}
