//! Candidate pruning bounds branching while retaining legal actions.
use super::*;
impl<'a> Search<'a> {
    pub(super) fn useful_roads(&self, s: &State, p: u8, cap: usize) -> Vec<Action> {
        road_preference(s, p, &self.w, cap)
    }
    pub(super) fn robber_candidates(&self, s: &State, p: u8, cap: usize) -> Vec<Action> {
        robber_preference_with(
            s,
            p,
            &self.w,
            cap,
            self.limits.robber,
            self.limits.expected_hands,
            self.limits.predict_threat,
        )
    }
    /// Whether a road, settlement, or city is affordable now or within the next own turn at
    /// current production, using the leaf's acquisition-time estimate. The `buy_tempo` rule
    /// offers a development purchase only when this is false.
    pub(super) fn build_within_turn(&self, s: &State, p: u8) -> bool {
        let own = s.players[p as usize];
        let rates = super::super::leaf::rates(s, p);
        let soon =
            |cost: Resources| super::super::leaf::eta_turns(s, own.resources, cost, &rates) <= 1.0;
        (own.cities < 4 && (0..54u8).any(|v| s.board.building(v) == p + 1) && soon(Resources::CITY))
            || (own.settlements < 5 && soon(Resources::SETTLEMENT))
            || (own.roads < 15 && soon(Resources::ROAD))
    }
    /// Whether the robber sits on this seat's best-producing hex: the one a held knight is
    /// saved to unblock before the roll under the `hold_knight` rule.
    pub(super) fn robber_on_best(&self, s: &State, p: u8) -> bool {
        let mut best: Option<(f64, u8)> = None;
        for hex in 0..19u8 {
            let Some(resource) = s.layout.resource(hex) else {
                continue;
            };
            let pips = f64::from(6u8.saturating_sub(s.layout.number(hex).abs_diff(7)));
            let mut rate = 0.0;
            for &v in &topology().hex_vertices[hex as usize] {
                if s.board.owner(v) == Some(p) {
                    rate += pips / 36.0 * if s.board.building(v) > 4 { 2.0 } else { 1.0 };
                }
            }
            let value = rate * self.w.resource_value[resource as usize];
            if value > 0.0 && best.as_ref().is_none_or(|b| value > b.0) {
                best = Some((value, hex));
            }
        }
        best.is_some_and(|(_, hex)| hex == s.robber)
    }
    pub(super) fn deficits(own: Resources) -> [(Resources, f64); 4] {
        [
            Resources::CITY,
            Resources::SETTLEMENT,
            Resources::DEVELOPMENT,
            Resources::ROAD,
        ]
        .map(|cost| (cost, crate::leaf::deficit(own, cost)))
    }
    /// Sum of the four build deficits with the kept hand credited by `rates * rolls` of
    /// expected production per resource.
    pub(super) fn deficits_projected(kept: Resources, rates: &[f64; 5], rolls: f64) -> f64 {
        [
            Resources::CITY,
            Resources::SETTLEMENT,
            Resources::DEVELOPMENT,
            Resources::ROAD,
        ]
        .iter()
        .map(|cost| {
            (0..5)
                .map(|r| {
                    let have = f64::from(kept.0[r]) + rates[r] * rolls;
                    (f64::from(cost.0[r]) - have).max(0.0)
                })
                .sum::<f64>()
        })
        .sum()
    }
    pub(super) fn bank_trades(&self, s: &State, p: u8, cap: usize) -> Vec<Action> {
        let own = s.players[p as usize].resources;
        let mut scored: Vec<(f64, Action)> = Vec::new();
        for give in Resource::ALL {
            let ratio = trade_ratio(s, p, give);
            if own.0[give as usize] < ratio {
                continue;
            }
            for get in Resource::ALL {
                if get == give || s.bank.0[get as usize] == 0 {
                    continue;
                }
                let after = Resources(std::array::from_fn(|r| {
                    own.0[r] - if r == give as usize { ratio } else { 0 }
                        + u8::from(r == get as usize)
                }));
                let gain = Self::deficits(own)
                    .iter()
                    .zip(Self::deficits(after))
                    .map(|(before, later)| before.1 - later.1)
                    .fold(0.0, f64::max);
                if gain > 0.0 {
                    scored.push((
                        gain * 10.0 - f64::from(ratio),
                        Action::BankTrade {
                            give: Resources::one(give, ratio),
                            receive: Resources::one(get, 1),
                        },
                    ));
                }
            }
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().take(cap).map(|(_, a)| a).collect()
    }
    /// Extra candidates for a hand over the discard limit: bank trades and roads chosen only
    /// to shed cards before another seat's roll. The leaf's `hand_risk` term prices shedding
    /// against holding; this switch only puts the choices on the list.
    pub(super) fn spend_down(&self, s: &State, p: u8, out: &mut Vec<Action>) {
        let own = s.players[p as usize];
        let hand = own.resources;
        let mut trades: Vec<(f64, Action)> = Vec::new();
        for give in Resource::ALL {
            let ratio = trade_ratio(s, p, give);
            if hand.0[give as usize] < ratio {
                continue;
            }
            for get in Resource::ALL {
                if get == give || s.bank.0[get as usize] == 0 {
                    continue;
                }
                let after = Resources(std::array::from_fn(|r| {
                    hand.0[r] - if r == give as usize { ratio } else { 0 }
                        + u8::from(r == get as usize)
                }));
                let gain = Self::deficits(hand)
                    .iter()
                    .zip(Self::deficits(after))
                    .map(|(before, later)| before.1 - later.1)
                    .fold(0.0, f64::max);
                // Rank by cards shed, then by the deficit change.
                trades.push((
                    f64::from(ratio - 1) * 10.0 + gain,
                    Action::BankTrade {
                        give: Resources::one(give, ratio),
                        receive: Resources::one(get, 1),
                    },
                ));
            }
        }
        trades.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut added = 0;
        for (_, a) in trades {
            if added == 3 {
                break;
            }
            if !out.contains(&a) {
                out.push(a);
                added += 1;
            }
        }
        if own.roads < 15 && hand.contains(Resources::ROAD) {
            let (base_best, _) = super::super::leaf::reach(s, p);
            let mut roads: Vec<(f64, u8)> = Vec::new();
            for e in 0..72u8 {
                if !road_allowed(s, p, e) {
                    continue;
                }
                let Ok(next) = apply(s, p, Action::BuildRoad { edge: e }) else {
                    continue;
                };
                let (best, others) = super::super::leaf::reach(&next, p);
                roads.push((best - base_best + f64::from(others) * 0.02, e));
            }
            roads.sort_by(|a, b| b.0.total_cmp(&a.0));
            let mut added = 0;
            for (_, e) in roads {
                if added == 2 {
                    break;
                }
                let a = Action::BuildRoad { edge: e };
                if !out.contains(&a) {
                    out.push(a);
                    added += 1;
                }
            }
        }
    }
    pub(super) fn plenty_candidates(s: &State, p: u8) -> Vec<Action> {
        let own = s.players[p as usize].resources;
        let count = s.bank.total().min(2);
        let mut out = Vec::new();
        for i in 0..5 {
            for j in i..5 {
                let mut r = Resources::default();
                r.0[i] += 1;
                if count == 2 {
                    r.0[j] += 1;
                } else if j != i {
                    continue;
                }
                if !s.bank.contains(r) {
                    continue;
                }
                let after = Resources(std::array::from_fn(|k| own.0[k] + r.0[k]));
                let gain = Self::deficits(own)
                    .iter()
                    .zip(Self::deficits(after))
                    .map(|(b, l)| b.1 - l.1)
                    .fold(0.0, f64::max);
                out.push((gain, Action::PlayYearOfPlenty { resources: r }));
                if count < 2 {
                    break;
                }
            }
        }
        out.sort_by(|a, b| b.0.total_cmp(&a.0));
        out.into_iter().take(2).map(|(_, a)| a).collect()
    }
    /// Discard choices for the root player: every multiset of the required size, ranked by
    /// how many target-build cards survive. Returns at most `cap`. With `project`, the kept
    /// hand is first credited with the seat's expected production over the rolls before its
    /// next turn, so a discard is valued by the target it preserves after that production
    /// rather than by the current hand alone.
    pub fn discard_candidates(s: &State, p: u8, cap: usize, project: bool) -> Vec<Action> {
        let hand = s.players[p as usize].resources;
        let k = hand.total() / 2;
        // Non-seven rolls before the seat's next turn, at most one per seat.
        let rolls = if project {
            f64::from(s.settings.players) * 5.0 / 6.0
        } else {
            0.0
        };
        let rates = if project {
            super::super::leaf::rates(s, p)
        } else {
            [0.0; 5]
        };
        let mut picks: Vec<Resources> = Vec::new();
        let mut pick = Resources::default();
        pub(super) fn rec(
            i: usize,
            left: u16,
            hand: Resources,
            pick: &mut Resources,
            out: &mut Vec<Resources>,
        ) {
            if i == 5 {
                if left == 0 {
                    out.push(*pick);
                }
                return;
            }
            let max = u16::from(hand.0[i]).min(left);
            for n in 0..=max {
                pick.0[i] = n as u8;
                rec(i + 1, left - n, hand, pick, out);
            }
            pick.0[i] = 0;
        }
        rec(0, k, hand, &mut pick, &mut picks);
        let mut out: Vec<(f64, Resources)> = picks
            .into_iter()
            .map(|pick| {
                let kept = Resources(std::array::from_fn(|r| hand.0[r] - pick.0[r]));
                let missing = if project {
                    Search::deficits_projected(kept, &rates, rolls)
                } else {
                    Search::deficits(kept).iter().map(|d| d.1).sum::<f64>()
                };
                let diversity = f64::from(kept.0.iter().filter(|&&n| n > 0).count() as u8);
                (-missing + diversity * 0.1, pick)
            })
            .collect();
        out.sort_by(|a, b| b.0.total_cmp(&a.0));
        out.into_iter()
            .take(cap)
            .map(|(_, r)| Action::Discard { resources: r })
            .collect()
    }
    /// Actions worth considering for the root player in world `s`.
    pub fn candidates(&self, s: &State, p: u8, steps: u32) -> Vec<Action> {
        let own = s.players[p as usize];
        let mut out = Vec::new();
        match s.phase {
            Phase::SetupSettlement { .. } => {
                if self.limits.opening != super::super::OpeningMode::Search {
                    return super::super::opening::rank_settlements(
                        s,
                        p,
                        1,
                        &super::super::opening::Bias::default(),
                    )
                    .into_iter()
                    .take(6)
                    .map(|plan| Action::BuildSettlement {
                        vertex: plan.vertex,
                    })
                    .collect();
                }
                let mut scored: Vec<(f64, u8)> = (0..54u8)
                    .filter(|&v| settlement_allowed(s, p, v, true))
                    .map(|v| {
                        let next = apply(s, p, Action::BuildSettlement { vertex: v }).ok();
                        let value = next.map_or(0.0, |n| self.value(&n, p).total());
                        (value, v)
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.total_cmp(&a.0));
                return scored
                    .into_iter()
                    .take(6)
                    .map(|(_, v)| Action::BuildSettlement { vertex: v })
                    .collect();
            }
            Phase::SetupRoad { vertex, .. } => {
                if self.limits.opening != super::super::OpeningMode::Search {
                    return super::super::opening::rank_roads(s, p, vertex)
                        .into_iter()
                        .take(3)
                        .map(|(edge, _, _)| Action::BuildRoad { edge })
                        .collect();
                }
                return topology().incident_edges[vertex as usize]
                    .iter()
                    .filter(|&&e| s.board.road(e).is_none())
                    .map(|&e| Action::BuildRoad { edge: e })
                    .collect();
            }
            Phase::FreeRoads { remaining, .. } => {
                let mut roads = self.useful_roads(s, p, 3);
                if roads.is_empty() {
                    roads.extend(
                        (0..72u8)
                            .filter(|&e| road_allowed(s, p, e))
                            .take(2)
                            .map(|e| Action::BuildRoad { edge: e }),
                    );
                }
                if remaining == 1 {
                    roads.push(Action::FinishRoadBuilding);
                }
                return roads;
            }
            Phase::Discard { .. } => {
                return Self::discard_candidates(s, p, 1, self.limits.discard_projection);
            }
            Phase::Robber { .. } => return self.robber_candidates(s, p, 4),
            Phase::Finished { .. } => return vec![],
            Phase::Roll => {
                out.push(Action::Roll);
                if !s.played_development && own.development[0] > 0 {
                    out.push(Action::PlayKnight);
                }
                return out;
            }
            Phase::Main => {}
        }
        if steps >= 12 {
            return vec![Action::EndTurn];
        }
        out.push(Action::EndTurn);
        if own.cities < 4 && own.resources.contains(Resources::CITY) {
            for v in 0..54u8 {
                if s.board.building(v) == p + 1 {
                    out.push(Action::BuildCity { vertex: v });
                }
            }
        }
        if own.settlements < 5 && own.resources.contains(Resources::SETTLEMENT) {
            for v in 0..54u8 {
                if settlement_allowed(s, p, v, false) {
                    out.push(Action::BuildSettlement { vertex: v });
                }
            }
        }
        if !s.played_development {
            if own.development[Development::Knight as usize] > 0
                && (!self.limits.hold_knight || self.robber_on_best(s, p))
            {
                out.push(Action::PlayKnight);
            }
            if own.development[Development::RoadBuilding as usize] > 0
                && apply(s, p, Action::PlayRoadBuilding).is_ok()
            {
                out.push(Action::PlayRoadBuilding);
            }
            if own.development[Development::YearOfPlenty as usize] > 0 {
                out.extend(Self::plenty_candidates(s, p));
            }
            if own.development[Development::Monopoly as usize] > 0 {
                let mut best = (0u16, Resource::Brick);
                for r in Resource::ALL {
                    let total: u16 = (0..s.settings.players)
                        .filter(|&o| o != p)
                        .map(|o| u16::from(s.players[o as usize].resources.0[r as usize]))
                        .sum();
                    if total > best.0 {
                        best = (total, r);
                    }
                }
                if best.0 > 0 {
                    out.push(Action::PlayMonopoly { resource: best.1 });
                }
            }
        }
        if s.deck_count() > 0
            && own.resources.contains(Resources::DEVELOPMENT)
            && (!self.limits.buy_tempo || !self.build_within_turn(s, p))
        {
            out.push(Action::BuyDevelopment);
        }
        out.extend(self.useful_roads(s, p, 4));
        out.extend(self.bank_trades(s, p, 4));
        if self.limits.spend_down && own.resources.total() > s.settings.discard_limit as u16 {
            self.spend_down(s, p, &mut out);
        }
        out
    }

    // ----- one planned turn ---------------------------------------------------------------
}
