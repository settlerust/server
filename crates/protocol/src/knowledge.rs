//! Resource accounting from the recipient's event history. No authoritative hands are used.
use crate::{EventBatch, PublicEvent};
use serde::{Deserialize, Serialize};
use settlers_engine::{Event, Observation, Phase, Resources};

/// Conservative marginal bounds, plus a proportional-loss estimate (not a posterior).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ResourceEstimate {
    pub minimum: [u8; 5],
    pub maximum: [u8; 5],
    pub expected: [f64; 5],
    pub total: u16,
}
impl ResourceEstimate {
    pub fn exact(hand: Resources) -> Self {
        Self {
            minimum: hand.0,
            maximum: hand.0,
            expected: hand.0.map(f64::from),
            total: hand.total(),
        }
    }
    pub fn unknown(total: u16) -> Self {
        Self {
            minimum: [0; 5],
            maximum: [total.min(19) as u8; 5],
            expected: [f64::from(total) / 5.0; 5],
            total,
        }
    }
    /// Impossible offers can be excluded without assuming the estimate is exact.
    pub fn can_have(&self, resources: Resources) -> bool {
        resources
            .0
            .iter()
            .zip(self.minimum)
            .map(|(&n, min)| u16::from(n.max(min)))
            .sum::<u16>()
            <= self.total
            && resources
                .0
                .iter()
                .zip(self.maximum)
                .all(|(&n, max)| n <= max)
    }
    pub fn is_exact(&self) -> bool {
        self.minimum == self.maximum
    }
    /// A funded open offer proves its sender currently holds at least the offered cards.
    pub fn condition_at_least(&mut self, cards: Resources) {
        for r in 0..5 {
            self.minimum[r] = self.minimum[r].max(cards.0[r]);
            self.maximum[r] = self.maximum[r].max(cards.0[r]);
        }
        self.tighten();
    }
    fn gain(&mut self, cards: Resources) {
        self.total += cards.total();
        for r in 0..5 {
            self.minimum[r] = self.minimum[r].saturating_add(cards.0[r]);
            self.maximum[r] = self.maximum[r].saturating_add(cards.0[r]);
            self.expected[r] += f64::from(cards.0[r]);
        }
        self.tighten();
    }
    fn spend(&mut self, cards: Resources) {
        self.total = self.total.saturating_sub(cards.total());
        for r in 0..5 {
            self.minimum[r] = self.minimum[r].saturating_sub(cards.0[r]);
            self.maximum[r] = self.maximum[r].saturating_sub(cards.0[r]);
            self.expected[r] = (self.expected[r] - f64::from(cards.0[r])).max(0.0);
        }
        self.tighten();
    }
    fn hidden_loss(&mut self, count: u16) {
        let before = self.total;
        self.total = self.total.saturating_sub(count);
        for r in 0..5 {
            self.minimum[r] = self.minimum[r].saturating_sub(count.min(255) as u8);
            self.expected[r] *= f64::from(self.total) / f64::from(before.max(1));
        }
        self.tighten();
    }
    fn tighten(&mut self) {
        for _ in 0..5 {
            for r in 0..5 {
                let other_min: u16 = (0..5)
                    .filter(|&i| i != r)
                    .map(|i| u16::from(self.minimum[i]))
                    .sum();
                let other_max: u16 = (0..5)
                    .filter(|&i| i != r)
                    .map(|i| u16::from(self.maximum[i]))
                    .sum();
                self.maximum[r] =
                    self.maximum[r].min(self.total.saturating_sub(other_min).min(19) as u8);
                self.minimum[r] = self.minimum[r]
                    .max(self.total.saturating_sub(other_max).min(19) as u8)
                    .min(self.maximum[r]);
                self.expected[r] =
                    self.expected[r].clamp(f64::from(self.minimum[r]), f64::from(self.maximum[r]));
            }
        }
        // Project the point estimate onto the bounded simplex; the bounds remain conservative.
        for _ in 0..10 {
            let delta = f64::from(self.total) - self.expected.iter().sum::<f64>();
            let room: [f64; 5] = std::array::from_fn(|r| {
                if delta > 0.0 {
                    f64::from(self.maximum[r]) - self.expected[r]
                } else {
                    self.expected[r] - f64::from(self.minimum[r])
                }
            });
            let sum: f64 = room.iter().sum();
            if sum < 1e-9 || delta.abs() < 1e-9 {
                break;
            }
            for (r, space) in room.iter().enumerate() {
                self.expected[r] += delta * space / sum;
            }
        }
    }
}

/// Rebuild in sequence order; duplicate batches are ignored. Missing history widens bounds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceKnowledge {
    pub players: [ResourceEstimate; 4],
    pub bank: Resources,
    /// Total resources across all other seats, not their individual allocation.
    pub opponent_totals: Option<Resources>,
    pub used_development: [u8; 5],
    pub history_complete: bool,
}
impl ResourceKnowledge {
    /// Use the snapshot version to detect missing trailing events, including unseen swaps
    /// that preserve a player's public card count. Future batches cannot inform this snapshot.
    pub fn from_history_at_version(
        observation: &Observation,
        batches: &[EventBatch],
        version: u64,
    ) -> Self {
        let visible: Vec<_> = batches
            .iter()
            .filter(|batch| batch.sequence <= version)
            .cloned()
            .collect();
        let mut knowledge = Self::from_history(observation, &visible);
        if !visible.iter().any(|batch| batch.sequence == version) {
            knowledge.history_complete = false;
            for player in &observation.players {
                if observation.seat != Some(player.seat) {
                    knowledge.players[player.seat as usize] =
                        ResourceEstimate::unknown(player.resource_count);
                }
            }
            knowledge.condition_on_bank(observation);
        }
        knowledge
    }

    pub fn from_history(observation: &Observation, batches: &[EventBatch]) -> Self {
        let mut ordered: Vec<_> = batches.iter().collect();
        ordered.sort_by_key(|b| b.sequence);
        ordered.dedup_by_key(|b| b.sequence);
        let mut knowledge = Self {
            players: [ResourceEstimate::default(); 4],
            bank: observation.bank,
            opponent_totals: observation.own.map(|own| {
                Resources(std::array::from_fn(|r| {
                    19u8.saturating_sub(observation.bank.0[r])
                        .saturating_sub(own.resources.0[r])
                }))
            }),
            used_development: [0; 5],
            history_complete: false,
        };
        let mut phase = Phase::SetupSettlement { step: 0 };
        let mut previous = None;
        let mut gap = false;
        for batch in ordered {
            if previous.is_some_and(|p| batch.sequence != p + 1) {
                gap = true;
            }
            previous = Some(batch.sequence);
            for event in &batch.events {
                if matches!(event, PublicEvent::Started) {
                    knowledge.history_complete = true;
                    gap = false;
                }
                let PublicEvent::Rule { event } = event else {
                    continue;
                };
                match *event {
                    Event::PhaseChanged { phase: next } => phase = next,
                    Event::Produced { player, resources }
                    | Event::TookResources { player, resources } => {
                        knowledge.players[player as usize].gain(resources)
                    }
                    Event::BuiltSettlement { player, .. } if phase == Phase::Main => {
                        knowledge.players[player as usize].spend(Resources::SETTLEMENT)
                    }
                    Event::BuiltRoad { player, .. } if phase == Phase::Main => {
                        knowledge.players[player as usize].spend(Resources::ROAD)
                    }
                    Event::BuiltCity { player, .. } => {
                        knowledge.players[player as usize].spend(Resources::CITY)
                    }
                    Event::BoughtDevelopment { player } => {
                        knowledge.players[player as usize].spend(Resources::DEVELOPMENT)
                    }
                    Event::PlayedDevelopment { card, .. } => {
                        knowledge.used_development[card as usize] += 1;
                        // Free-road effects can occur in this same batch before PhaseChanged.
                        if card == settlers_engine::Development::RoadBuilding {
                            phase = Phase::FreeRoads {
                                remaining: 2,
                                resume: settlers_engine::Resume::Main,
                            };
                        }
                    }
                    Event::SetupSettlementMoved {
                        player,
                        returned,
                        received,
                        ..
                    } => {
                        knowledge.players[player as usize].spend(returned);
                        knowledge.players[player as usize].gain(received);
                    }
                    Event::Discarded { player, count } => {
                        knowledge.players[player as usize].hidden_loss(count)
                    }
                    Event::BankTraded {
                        player,
                        give,
                        receive,
                    } => {
                        knowledge.players[player as usize].spend(give);
                        knowledge.players[player as usize].gain(receive);
                    }
                    Event::Traded {
                        player,
                        other,
                        give,
                        receive,
                    } => {
                        knowledge.players[player as usize].spend(give);
                        knowledge.players[other as usize].spend(receive);
                        knowledge.players[player as usize].gain(receive);
                        knowledge.players[other as usize].gain(give);
                    }
                    Event::MonopolyCollected {
                        player,
                        victim,
                        resource,
                        count,
                    } => {
                        let cards = Resources::one(resource, count);
                        knowledge.players[victim as usize].spend(cards);
                        let victim = &mut knowledge.players[victim as usize];
                        victim.minimum[resource as usize] = 0;
                        victim.maximum[resource as usize] = 0;
                        victim.expected[resource as usize] = 0.0;
                        victim.tighten();
                        knowledge.players[player as usize].gain(cards);
                    }
                    Event::Stole {
                        player,
                        victim,
                        count,
                    } => {
                        let receipt = batch.events.iter().find_map(|e| match *e {
                            PublicEvent::TheftDetails {
                                player: p,
                                victim: v,
                                resource,
                            } if p == player && v == victim => Some(resource),
                            _ => None,
                        });
                        if let Some(r) = receipt {
                            let cards = Resources::one(r, count);
                            knowledge.players[victim as usize].spend(cards);
                            knowledge.players[player as usize].gain(cards);
                        } else {
                            let source = knowledge.players[victim as usize];
                            knowledge.players[victim as usize].hidden_loss(u16::from(count));
                            let target = &mut knowledge.players[player as usize];
                            target.total += u16::from(count);
                            for r in 0..5 {
                                if source.maximum[r] > 0 {
                                    target.maximum[r] = target.maximum[r].saturating_add(count);
                                }
                                target.expected[r] += f64::from(count) * source.expected[r]
                                    / f64::from(source.total.max(1));
                            }
                            target.tighten();
                        }
                    }
                    _ => {}
                }
            }
        }
        knowledge.history_complete &= !gap;
        for player in &observation.players {
            let estimate = &mut knowledge.players[player.seat as usize];
            if !knowledge.history_complete || estimate.total != player.resource_count {
                *estimate = ResourceEstimate::unknown(player.resource_count);
            }
        }
        if let (Some(seat), Some(own)) = (observation.seat, observation.own) {
            knowledge.players[seat as usize] = ResourceEstimate::exact(own.resources);
        }
        knowledge.condition_on_bank(observation);
        knowledge
    }

    /// Inventory conservation conditions marginal holdings without identifying whose cards
    /// moved in a hidden theft. Missing history cannot erase exact public bank information.
    pub fn condition_on_bank(&mut self, observation: &Observation) {
        self.bank = observation.bank;
        self.opponent_totals = observation.own.map(|own| {
            Resources(std::array::from_fn(|r| {
                19u8.saturating_sub(observation.bank.0[r])
                    .saturating_sub(own.resources.0[r])
            }))
        });
        if !self.constrain_bank(observation) {
            self.history_complete = false;
            for player in &observation.players {
                if observation.seat != Some(player.seat) {
                    self.players[player.seat as usize] =
                        ResourceEstimate::unknown(player.resource_count);
                }
            }
            if let (Some(seat), Some(own)) = (observation.seat, observation.own) {
                self.players[seat as usize] = ResourceEstimate::exact(own.resources);
            }
            // An invalid snapshot is rejected by the sampler; never manufacture a hand.
            self.constrain_bank(observation);
        }
    }

    fn constrain_bank(&mut self, observation: &Observation) -> bool {
        let count = observation.settings.players as usize;
        if count > 4 || observation.bank.0.iter().any(|&n| n > 19) {
            return false;
        }
        let supply = observation.bank.0.map(|n| 19 - n);
        if self.players[..count].iter().map(|p| p.total).sum::<u16>()
            != supply.iter().map(|&n| u16::from(n)).sum::<u16>()
        {
            return false;
        }
        for _ in 0..100 {
            let before = self.players.map(|p| (p.minimum, p.maximum));
            for (r, &total) in supply.iter().enumerate() {
                for p in 0..count {
                    let other_min: u16 = (0..count)
                        .filter(|&other| other != p)
                        .map(|other| u16::from(self.players[other].minimum[r]))
                        .sum();
                    let other_max: u16 = (0..count)
                        .filter(|&other| other != p)
                        .map(|other| u16::from(self.players[other].maximum[r]))
                        .sum();
                    if other_min > u16::from(total) {
                        return false;
                    }
                    let estimate = &mut self.players[p];
                    estimate.minimum[r] =
                        estimate.minimum[r].max(u16::from(total).saturating_sub(other_max) as u8);
                    estimate.maximum[r] =
                        estimate.maximum[r].min((u16::from(total) - other_min) as u8);
                    if estimate.minimum[r] > estimate.maximum[r] {
                        return false;
                    }
                }
            }
            for estimate in &mut self.players[..count] {
                if estimate.minimum.iter().map(|&n| u16::from(n)).sum::<u16>() > estimate.total
                    || estimate.maximum.iter().map(|&n| u16::from(n)).sum::<u16>() < estimate.total
                {
                    return false;
                }
                estimate.tighten();
            }
            if before == self.players.map(|p| (p.minimum, p.maximum)) {
                break;
            }
        }
        // Alternate bounded row/column projections. These point estimates remain a prior,
        // while the sampler enforces both sets of exact inventory totals in every world.
        for _ in 0..64 {
            for estimate in &mut self.players[..count] {
                estimate.tighten();
            }
            let mut largest_delta: f64 = 0.0;
            for (r, &total) in supply.iter().enumerate() {
                let delta = f64::from(total)
                    - self.players[..count]
                        .iter()
                        .map(|p| p.expected[r])
                        .sum::<f64>();
                largest_delta = largest_delta.max(delta.abs());
                let room: Vec<_> = self.players[..count]
                    .iter()
                    .map(|p| {
                        if delta > 0.0 {
                            f64::from(p.maximum[r]) - p.expected[r]
                        } else {
                            p.expected[r] - f64::from(p.minimum[r])
                        }
                    })
                    .collect();
                let total_room: f64 = room.iter().sum();
                if total_room > 1e-9 {
                    for (p, capacity) in room.iter().enumerate() {
                        self.players[p].expected[r] += delta * capacity / total_room;
                    }
                }
            }
            if largest_delta < 1e-9 {
                break;
            }
        }
        true
    }
}
