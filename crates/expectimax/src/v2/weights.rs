//! Tunable leaf weights and the inclination sliders that scale them. Every value is a plain
//! multiplier in heuristic points, so a configuration can be logged and reproduced exactly.
use serde::{Deserialize, Serialize};

/// Leaf weights. Points, cards, and production share one currency: a card is worth
/// `card_value` points, and production is worth its cards over the rolls that remain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Weights {
    /// One victory point, including hidden victory cards.
    pub point: f64,
    /// One resource card that will be spent later.
    pub card_value: f64,
    /// Discount on cards held now versus cards produced later.
    pub hand_discount: f64,
    /// Expected game length in turns per seat; remaining rolls come from this estimate.
    pub turns_per_player: f64,
    /// Never value production over fewer rolls than this.
    pub minimum_rolls: f64,
    /// Per-resource multipliers in brick, lumber, wool, grain, ore order.
    pub resource_value: [f64; 5],
    /// Bonus for producing every resource type at a useful rate.
    pub diversity: f64,
    /// Multiplier on the best target's gain discounted by turns until it is affordable.
    pub readiness: f64,
    /// Fraction of a reachable site's production-equivalent value credited before it is built.
    pub expansion: f64,
    /// Each additional reachable legal site beyond the best one.
    pub expansion_sites: f64,
    /// Held non-victory development cards.
    pub development: f64,
    /// Knights played, as progress toward the largest army.
    pub knight: f64,
    /// Own longest trail length.
    pub road_length: f64,
    /// Being within one road of taking the longest-road award.
    pub road_contention: f64,
    /// Expected discard loss when holding more than the limit, in card values.
    pub hand_risk: f64,
    /// Fraction of matched production credited for a 2:1 port, and a flat 3:1 port value.
    pub port_match: f64,
    pub port_generic: f64,
    /// Highest opponent public score and the sum of opponents' scores.
    pub opponent_max_point: f64,
    pub opponent_sum_point: f64,
    /// Highest opponent expected production, in cards per roll.
    pub opponent_production: f64,
    /// Discount on a reachable site another seat's predicted line is about to take, per
    /// unit of the site's production value and per turn of that seat's time-to-afford.
    /// Zero, the default, never discounts.
    pub denial: f64,
    /// Terminal win and loss.
    pub terminal: f64,
}
impl Default for Weights {
    fn default() -> Self {
        Self {
            point: 100.0,
            card_value: 20.0,
            hand_discount: 0.6,
            turns_per_player: 18.0,
            minimum_rolls: 6.0,
            resource_value: [1.0, 1.0, 0.85, 1.15, 1.2],
            diversity: 40.0,
            readiness: 1.0,
            expansion: 0.5,
            expansion_sites: 4.0,
            development: 10.0,
            knight: 8.0,
            road_length: 4.0,
            road_contention: 40.0,
            hand_risk: 1.0,
            port_match: 0.25,
            port_generic: 15.0,
            opponent_max_point: -35.0,
            opponent_sum_point: -6.0,
            opponent_production: -60.0,
            denial: 0.0,
            terminal: 10_000.0,
        }
    }
}

/// Sliders in `[0, 1]` that shape the weights. `0.5` leaves every weight unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Inclinations {
    /// Weight on slowing opponents: blocking, robbing the leader, opponent score terms.
    pub aggression: f64,
    /// Tolerance for discard exposure and volatile plans.
    pub risk: f64,
    /// Appetite for development cards and the largest army.
    pub development: f64,
    /// Settlements and roads versus cities.
    pub expansion: f64,
    /// Willingness to propose and accept player trades.
    pub trade: f64,
}
impl Default for Inclinations {
    fn default() -> Self {
        Self {
            aggression: 0.5,
            risk: 0.5,
            development: 0.5,
            expansion: 0.5,
            trade: 0.5,
        }
    }
}
impl Inclinations {
    pub fn clamped(self) -> Self {
        Self {
            aggression: self.aggression.clamp(0.0, 1.0),
            risk: self.risk.clamp(0.0, 1.0),
            development: self.development.clamp(0.0, 1.0),
            expansion: self.expansion.clamp(0.0, 1.0),
            trade: self.trade.clamp(0.0, 1.0),
        }
    }
    /// Scale a slider into a multiplier: `0 -> 0.5`, `0.5 -> 1`, `1 -> 1.5`.
    fn scale(value: f64) -> f64 {
        0.5 + value
    }
    pub fn apply(self, base: Weights) -> Weights {
        let i = self.clamped();
        Weights {
            opponent_max_point: base.opponent_max_point * Self::scale(i.aggression),
            opponent_sum_point: base.opponent_sum_point * Self::scale(i.aggression),
            opponent_production: base.opponent_production * Self::scale(i.aggression),
            hand_risk: base.hand_risk * Self::scale(1.0 - i.risk),
            development: base.development * Self::scale(i.development),
            knight: base.knight * Self::scale(i.development),
            expansion: base.expansion * Self::scale(i.expansion),
            expansion_sites: base.expansion_sites * Self::scale(i.expansion),
            road_length: base.road_length * Self::scale(i.expansion),
            ..base
        }
    }
    /// Minimum improvement in heuristic points before proposing or accepting a trade.
    pub fn trade_margin(self) -> f64 {
        (1.0 - self.clamped().trade) * 24.0
    }
}
