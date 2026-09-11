//! One complete game between seat policies. The table owns the authoritative state; seats only
//! ever receive `observe(&state, Some(seat))` and the event batches a participant would see.
use crate::seats::{Effort, Seat, fallback};
use serde::{Deserialize, Serialize};
use settlers_engine::*;
use settlers_protocol::{
    Command, EventBatch, GameView, MemberView, OfferStatus, PublicEvent, TradeCloseReason,
    TradeOffer,
};
use std::collections::BTreeMap;
use std::time::Instant;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TableSettings {
    pub settings: Settings,
    pub seed: u32,
    pub max_turns: u32,
    pub max_actions_per_turn: u32,
    pub max_decisions: u32,
    /// Collect per-decision summaries from seats that provide them.
    pub trace: bool,
}
impl Default for TableSettings {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            seed: 0,
            max_turns: 500,
            max_actions_per_turn: 60,
            max_decisions: 30_000,
            trace: false,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeatRecord {
    pub seat: u8,
    pub label: String,
    pub points: u8,
    pub decisions: u32,
    pub decision_ms: f64,
    pub max_decision_ms: f64,
    pub invalid: u32,
    pub stalls: u32,
    pub forced: u32,
    pub offers: u32,
    pub offers_accepted: u32,
    pub accepted_others: u32,
    /// Counter-offers this seat made in reply to another seat's offer, and how many the
    /// offerer took.
    #[serde(default)]
    pub counters: u32,
    #[serde(default)]
    pub counters_accepted: u32,
    /// Offers this seat declined explicitly.
    #[serde(default)]
    pub declined: u32,
    pub settlements_built: u32,
    pub cities_built: u32,
    pub roads_built: u32,
    pub development_bought: u32,
    pub bank_trades: u32,
    pub player_trades: u32,
    pub cards_discarded: u32,
    pub times_robbed: u32,
    pub effort: Effort,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameRecord {
    pub seed: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace: Vec<serde_json::Value>,
    pub players: u8,
    pub status: String,
    pub winner: Option<u8>,
    pub turns: u32,
    pub actions: u32,
    pub duration_ms: f64,
    pub seats: Vec<SeatRecord>,
    pub events: BTreeMap<String, u32>,
}
pub struct Table {
    pub state: State,
    pub version: u64,
    pub histories: Vec<Vec<EventBatch>>,
    labels: Vec<String>,
    /// The stolen resource is known to the table between a transition and its publication.
    last_theft: Option<Resource>,
}
impl Table {
    pub fn view(&self, seat: u8, offers: &[TradeOffer]) -> GameView {
        GameView {
            id: "arena".into(),
            name: "engine arena".into(),
            research: false,
            research_hands: None,
            version: self.version,
            settings: self.state.settings,
            host: false,
            members: self
                .labels
                .iter()
                .enumerate()
                .map(|(i, label)| MemberView {
                    seat: Some(i as u8),
                    name: format!("{label} seat {i}"),
                    connected: true,
                    ready: true,
                    is_you: i as u8 == seat,
                })
                .collect(),
            game: Some(observe(&self.state, Some(seat))),
            opening: None,
            offers: offers.to_vec(),
            filters: None,
            deadline_ms: None,
            discard: None,
        }
    }
    /// Commit one batch: public events for everyone, theft receipts only to thief and victim.
    fn publish(&mut self, seats: &mut [Box<dyn Seat>], events: Vec<PublicEvent>) {
        self.version += 1;
        for (i, history) in self.histories.iter_mut().enumerate() {
            let mut own = events.clone();
            for event in &events {
                if let PublicEvent::Rule {
                    event: Event::Stole { player, victim, .. },
                } = event
                    && (i as u8 == *player || i as u8 == *victim)
                {
                    // Theft receipts are derived from the authoritative before/after hands.
                    if let Some(resource) = self.last_theft {
                        own.push(PublicEvent::TheftDetails {
                            player: *player,
                            victim: *victim,
                            resource,
                        });
                    }
                }
            }
            let batch = EventBatch {
                sequence: self.version,
                timestamp_ms: self.version,
                events: own,
            };
            seats[i].deliver(&batch);
            history.push(batch);
        }
    }
}
impl Table {
    fn commit(
        &mut self,
        seats: &mut [Box<dyn Seat>],
        next: State,
        events: Vec<Event>,
        counts: &mut BTreeMap<String, u32>,
        records: &mut [SeatRecord],
    ) {
        self.last_theft = None;
        for event in &events {
            *counts.entry(event_name(event).into()).or_default() += 1;
            match *event {
                Event::BuiltSettlement { player, .. } if self.state.turn > 0 => {
                    records[player as usize].settlements_built += 1
                }
                Event::BuiltCity { player, .. } => records[player as usize].cities_built += 1,
                Event::BuiltRoad { player, .. } if self.state.turn > 0 => {
                    records[player as usize].roads_built += 1
                }
                Event::BoughtDevelopment { player } => {
                    records[player as usize].development_bought += 1
                }
                Event::BankTraded { player, .. } => records[player as usize].bank_trades += 1,
                Event::Traded { player, other, .. } => {
                    records[player as usize].player_trades += 1;
                    records[other as usize].player_trades += 1;
                }
                Event::Discarded { player, count } => {
                    records[player as usize].cards_discarded += u32::from(count)
                }
                Event::Stole { victim, .. } => records[victim as usize].times_robbed += 1,
                _ => {}
            }
            if let Event::Stole { player, victim, .. } = event {
                let before = self.state.players[*victim as usize].resources;
                let after = next.players[*victim as usize].resources;
                self.last_theft = (0..5)
                    .find(|&r| before.0[r] > after.0[r])
                    .map(|r| Resource::ALL[r]);
                let _ = player;
            }
        }
        self.state = next;
        self.publish(
            seats,
            events
                .into_iter()
                .map(|event| PublicEvent::Rule { event })
                .collect(),
        );
    }
}
fn event_name(event: &Event) -> &'static str {
    match event {
        Event::SetupSettlementMoved { .. } => "setup_settlement_moved",
        Event::BuiltSettlement { .. } => "built_settlement",
        Event::BuiltRoad { .. } => "built_road",
        Event::BuiltCity { .. } => "built_city",
        Event::Rolled { .. } => "rolled",
        Event::Produced { .. } => "produced",
        Event::Discarded { .. } => "discarded",
        Event::RobberMoved { .. } => "robber_moved",
        Event::Stole { .. } => "stole",
        Event::BoughtDevelopment { .. } => "bought_development",
        Event::PlayedDevelopment { .. } => "played_development",
        Event::TookResources { .. } => "took_resources",
        Event::MonopolyCollected { .. } => "monopoly_collected",
        Event::BankTraded { .. } => "bank_traded",
        Event::Traded { .. } => "traded",
        Event::AwardsChanged { .. } => "awards_changed",
        Event::TurnChanged { .. } => "turn_changed",
        Event::PhaseChanged { .. } => "phase_changed",
        Event::Won { .. } => "won",
        Event::TimedOut { .. } => "timed_out",
    }
}
fn actor(s: &State) -> u8 {
    if let Phase::Discard { pending } = s.phase {
        pending.trailing_zeros() as u8
    } else {
        s.current
    }
}

/// Play one game to completion or a cap. Seat `i` sits at engine seat `i`.
pub fn play(mut seats: Vec<Box<dyn Seat>>, table: TableSettings) -> Result<GameRecord, String> {
    let n = table.settings.players as usize;
    if seats.len() != n {
        return Err(format!("lineup has {} seats for {n} players", seats.len()));
    }
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&table.seed.to_le_bytes());
    let state = State::new(table.settings, bytes).map_err(|e| e.to_string())?;
    let labels: Vec<_> = seats.iter().map(|s| s.label()).collect();
    let mut t = Table {
        state,
        version: 0,
        histories: vec![vec![]; n],
        labels: labels.clone(),
        last_theft: None,
    };
    t.publish(&mut seats, vec![PublicEvent::Started]);
    let mut records: Vec<SeatRecord> = (0..n)
        .map(|i| SeatRecord {
            seat: i as u8,
            label: labels[i].clone(),
            ..SeatRecord::default()
        })
        .collect();
    let mut counts = BTreeMap::new();
    let started = Instant::now();
    let mut actions = 0u32;
    let mut turn_actions = 0u32;
    let mut noops = 0u32;
    let mut turn_key = (0u32, 0u8);
    let mut status = "completed";
    let mut decisions = 0u32;
    let mut trace = Vec::new();
    loop {
        if matches!(t.state.phase, Phase::Finished { .. }) {
            break;
        }
        if t.state.turn > table.max_turns {
            status = "turn_cap";
            break;
        }
        if decisions >= table.max_decisions {
            status = "decision_cap";
            break;
        }
        let key = (t.state.turn, t.state.current);
        if key != turn_key {
            turn_key = key;
            turn_actions = 0;
            noops = 0;
        }
        let p = actor(&t.state);
        let pi = p as usize;
        let view = t.view(p, &[]);
        let clock = Instant::now();
        let command = seats[pi].choose(&view, &t.histories[pi]);
        let ms = clock.elapsed().as_secs_f64() * 1000.0;
        if table.trace
            && let Some(mut entry) = seats[pi].trace()
        {
            if let Some(map) = entry.as_object_mut() {
                map.insert("seat".into(), serde_json::json!(p));
                map.insert("turn".into(), serde_json::json!(t.state.turn));
                map.insert("ms".into(), serde_json::json!(ms));
                map.insert(
                    "chosen".into(),
                    serde_json::to_value(&command).unwrap_or_default(),
                );
            }
            trace.push(entry);
        }
        decisions += 1;
        records[pi].decisions += 1;
        records[pi].decision_ms += ms;
        records[pi].max_decision_ms = records[pi].max_decision_ms.max(ms);
        let forced = turn_actions >= table.max_actions_per_turn || noops > 3;
        if forced {
            records[pi].forced += 1;
        }
        let command = if forced { fallback(&view) } else { command };
        match command {
            Some(Command::Act { action }) => {
                match apply_recorded(&t.state, p, action) {
                    Ok((next, events)) => {
                        t.commit(&mut seats, next, events, &mut counts, &mut records);
                        seats[pi].committed(&Command::Act { action }, &view);
                    }
                    Err(_) => {
                        records[pi].invalid += 1;
                        let Some(Command::Act { action }) = fallback(&view) else {
                            return Err("no fallback action".into());
                        };
                        let (next, events) =
                            apply_recorded(&t.state, p, action).map_err(|e| e.to_string())?;
                        t.commit(&mut seats, next, events, &mut counts, &mut records);
                    }
                }
                actions += 1;
                turn_actions += 1;
            }
            Some(Command::OfferTrade {
                to,
                give,
                receive,
                message,
            }) => {
                let o = view.game.as_ref().ok_or("missing observation")?;
                let funded = o.own.is_some_and(|own| own.resources.contains(give));
                let valid = p == t.state.current
                    && t.state.phase == Phase::Main
                    && funded
                    && give.total() > 0
                    && receive.total() > 0
                    && give.disjoint(receive)
                    && !to.is_empty()
                    && to.iter().all(|&r| r != p && (r as usize) < n);
                if !valid {
                    records[pi].invalid += 1;
                    noops += 1;
                    continue;
                }
                records[pi].offers += 1;
                let offer = TradeOffer {
                    id: format!("offer-{}", t.version + 1),
                    from: p,
                    to: to.clone(),
                    give,
                    receive,
                    message: message.clone(),
                    status: OfferStatus::Open,
                    declined: vec![],
                };
                t.publish(
                    &mut seats,
                    vec![PublicEvent::TradeOffered {
                        offer: offer.clone(),
                    }],
                );
                seats[pi].committed(
                    &Command::OfferTrade {
                        to,
                        give,
                        receive,
                        message,
                    },
                    &view,
                );
                let mut winner = None;
                let mut countered = false;
                let mut order: Vec<u8> = (1..n as u8).map(|k| (p + k) % n as u8).collect();
                order.retain(|r| offer.to.contains(r));
                for r in order {
                    let ri = r as usize;
                    let rview = t.view(r, std::slice::from_ref(&offer));
                    let clock = Instant::now();
                    let answer = seats[ri].choose(&rview, &t.histories[ri]);
                    let ms = clock.elapsed().as_secs_f64() * 1000.0;
                    records[ri].decisions += 1;
                    records[ri].decision_ms += ms;
                    records[ri].max_decision_ms = records[ri].max_decision_ms.max(ms);
                    match &answer {
                        Some(Command::AcceptTrade { offer_id }) if *offer_id == offer.id => {
                            let trade = Action::Trade {
                                other: r,
                                give,
                                receive,
                            };
                            if let Ok((next, events)) = apply_recorded(&t.state, p, trade) {
                                t.commit(&mut seats, next, events, &mut counts, &mut records);
                                seats[ri].committed(
                                    &Command::AcceptTrade {
                                        offer_id: offer.id.clone(),
                                    },
                                    &rview,
                                );
                                records[ri].accepted_others += 1;
                                records[pi].offers_accepted += 1;
                                winner = Some(r);
                                break;
                            }
                        }
                        Some(Command::RejectTrade { offer_id }) if *offer_id == offer.id => {
                            records[ri].declined += 1;
                            t.publish(
                                &mut seats,
                                vec![PublicEvent::TradeRejected {
                                    offer_id: offer.id.clone(),
                                    player: r,
                                }],
                            );
                        }
                        Some(Command::OfferTrade {
                            to: counter_to,
                            give: counter_give,
                            receive: counter_receive,
                            message: counter_message,
                        }) if *counter_to == vec![p] => {
                            // A counter: the recipient proposes other terms to the offerer.
                            let funded = t.state.players[ri].resources.contains(*counter_give);
                            if !funded
                                || counter_give.total() == 0
                                || counter_receive.total() == 0
                                || !counter_give.disjoint(*counter_receive)
                            {
                                records[ri].invalid += 1;
                                continue;
                            }
                            records[ri].offers += 1;
                            records[ri].counters += 1;
                            let counter = TradeOffer {
                                id: format!("offer-{}-c{}", t.version + 1, r),
                                from: r,
                                to: vec![p],
                                give: *counter_give,
                                receive: *counter_receive,
                                message: counter_message.clone(),
                                status: OfferStatus::Open,
                                declined: vec![],
                            };
                            t.publish(
                                &mut seats,
                                vec![PublicEvent::TradeOffered {
                                    offer: counter.clone(),
                                }],
                            );
                            seats[ri].committed(answer.as_ref().unwrap(), &rview);
                            let both = [offer.clone(), counter.clone()];
                            let pview = t.view(p, &both);
                            let clock = Instant::now();
                            let reply = seats[pi].choose(&pview, &t.histories[pi]);
                            let ms = clock.elapsed().as_secs_f64() * 1000.0;
                            records[pi].decisions += 1;
                            records[pi].decision_ms += ms;
                            records[pi].max_decision_ms = records[pi].max_decision_ms.max(ms);
                            let mut taken = false;
                            if let Some(Command::AcceptTrade { offer_id }) = &reply
                                && *offer_id == counter.id
                            {
                                let trade = Action::Trade {
                                    other: r,
                                    give: *counter_receive,
                                    receive: *counter_give,
                                };
                                if let Ok((next, events)) = apply_recorded(&t.state, p, trade) {
                                    t.commit(&mut seats, next, events, &mut counts, &mut records);
                                    seats[pi].committed(
                                        &Command::AcceptTrade {
                                            offer_id: counter.id.clone(),
                                        },
                                        &pview,
                                    );
                                    records[ri].counters_accepted += 1;
                                    records[ri].offers_accepted += 1;
                                    records[pi].accepted_others += 1;
                                    taken = true;
                                }
                            }
                            t.publish(
                                &mut seats,
                                vec![PublicEvent::TradeClosed {
                                    offer_id: counter.id,
                                    reason: if taken {
                                        TradeCloseReason::Accepted
                                    } else {
                                        TradeCloseReason::Declined
                                    },
                                    winner: taken.then_some(p),
                                }],
                            );
                            if taken {
                                t.publish(
                                    &mut seats,
                                    vec![PublicEvent::TradeClosed {
                                        offer_id: offer.id.clone(),
                                        reason: TradeCloseReason::ResourcesChanged,
                                        winner: None,
                                    }],
                                );
                                countered = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if countered {
                    actions += 1;
                    turn_actions += 1;
                    continue;
                }
                t.publish(
                    &mut seats,
                    vec![PublicEvent::TradeClosed {
                        offer_id: offer.id,
                        reason: if winner.is_some() {
                            TradeCloseReason::Accepted
                        } else {
                            TradeCloseReason::Declined
                        },
                        winner,
                    }],
                );
                actions += 1;
                turn_actions += 1;
            }
            Some(_) => {
                noops += 1;
                records[pi].invalid += 1;
            }
            None => {
                let o = view.game.as_ref().ok_or("missing observation")?;
                if o.legal_actions.is_empty() {
                    // Nothing to do for this seat; the engine's actor must still move.
                    return Err(format!(
                        "actor {p} has no legal actions in phase {:?}",
                        t.state.phase
                    ));
                }
                records[pi].stalls += 1;
                let Some(Command::Act { action }) = fallback(&view) else {
                    return Err("no fallback action".into());
                };
                let (next, events) =
                    apply_recorded(&t.state, p, action).map_err(|e| e.to_string())?;
                t.commit(&mut seats, next, events, &mut counts, &mut records);
                actions += 1;
                turn_actions += 1;
            }
        }
    }
    for (i, record) in records.iter_mut().enumerate() {
        record.points = points(&t.state, i as u8);
        record.effort = seats[i].effort();
    }
    Ok(GameRecord {
        seed: table.seed,
        trace,
        players: n as u8,
        status: status.into(),
        winner: match t.state.phase {
            Phase::Finished { winner } => Some(winner),
            _ => None,
        },
        turns: t.state.turn,
        actions,
        duration_ms: started.elapsed().as_secs_f64() * 1000.0,
        seats: records,
        events: counts,
    })
}
