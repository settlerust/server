//! Opponent rounds aggregate weighted worlds without revealing hidden hands.
use super::*;
impl<'a> Search<'a> {
    /// An opponent's move under the leaf-guided model: the candidate whose immediate
    /// static value from that seat's perspective is highest, with `EndTurn` after long turns.
    pub(super) fn leaf_action(&mut self, s: &State, p: u8, steps: u32) -> Action {
        if steps >= 12 && s.phase == Phase::Main {
            return Action::EndTurn;
        }
        let candidates = match s.phase {
            Phase::Main | Phase::Roll | Phase::Robber { .. } | Phase::FreeRoads { .. } => {
                self.candidates(s, p, steps)
            }
            _ => vec![opponent::act(s, p, steps)],
        };
        let mut best: Option<(f64, Action)> = None;
        for a in candidates {
            let value = match a {
                Action::Roll => f64::INFINITY,
                Action::EndTurn => self.value(s, p).total(),
                _ => match apply(s, p, a) {
                    Ok(next) => self.value(&next, p).total(),
                    Err(_) => continue,
                },
            };
            self.stats.leaves += 1;
            if best.as_ref().is_none_or(|b| value > b.0) {
                best = Some((value, a));
            }
        }
        best.map_or_else(|| opponent::act(s, p, steps), |b| b.1)
    }

    // ----- opponents' round under common random scenarios ------------------------------------

    /// Arrival positions after every other seat's turn, for every scenario of the level, as
    /// (mass share, group) in scenario then group order. Cached per afterstate group and level
    /// across the depth passes of a decision; `None` when the search aborted mid-round.
    pub(super) fn arrivals(
        &mut self,
        worlds: &[World],
        level: u8,
    ) -> Result<Option<Arrivals>, RuleError> {
        let count = if level <= 1 {
            self.limits.scenarios
        } else {
            self.limits.inner_scenarios
        }
        .max(1);
        let key = (information_set(&worlds[0].state, self.seat), level);
        if let Some((arrivals, generation, ticks)) = self.cache.rounds.get(&key) {
            let (arrivals, generation, ticks) = (arrivals.clone(), *generation, *ticks);
            self.stats.scenarios += count as u64;
            self.reuse(generation, ticks);
            return Ok(Some(arrivals));
        }
        let ticks_before = self.stats.transitions;
        let players = worlds[0].state.settings.players as usize;
        let mut scenarios = Scenario::set(count, u32::from(level), players);
        let mut arrivals: Arrivals = Vec::new();
        for scenario in &mut scenarios {
            self.stats.scenarios += 1;
            let mut arrived = Vec::with_capacity(worlds.len());
            for w in worlds {
                scenario.reset();
                let mut s = w.state;
                let mut steps = 0u32;
                let mut turn_steps = 0u32;
                let mut turn_key = (s.turn, s.current);
                while !(matches!(s.phase, Phase::Finished { .. })
                    || s.current == self.seat && s.phase == Phase::Main && actor(&s) == self.seat)
                    && steps < MAX_STEPS
                {
                    if (s.turn, s.current) != turn_key {
                        turn_key = (s.turn, s.current);
                        turn_steps = 0;
                    }
                    let p = actor(&s);
                    let a = if p != self.seat && self.limits.predict_opponents {
                        super::super::predict::act(&s, p, turn_steps)
                    } else if self.limits.leaf_opponents && p != self.seat {
                        self.leaf_action(&s, p, turn_steps)
                    } else {
                        opponent::act(&s, p, turn_steps)
                    };
                    let next = match a {
                        Action::Roll => {
                            let total = scenario.roll();
                            apply_with_chance(&s, p, a, dice_chance(total))
                        }
                        Action::BuyDevelopment
                        | Action::MoveRobber {
                            victim: Some(_), ..
                        } => {
                            let branches = chance_outcomes(&s, p, a)?;
                            let weights: Vec<u32> = branches.iter().map(|b| b.numerator).collect();
                            let i = pick_weighted(&mut scenario.rng, &weights);
                            apply_with_chance(&s, p, a, branches[i].outcome)
                        }
                        _ => apply(&s, p, a),
                    };
                    match next {
                        Ok(n) => s = n,
                        Err(_) => {
                            // The builder proposed an illegal move in this world; end the turn.
                            let fallback = match s.phase {
                                Phase::Main => Action::EndTurn,
                                Phase::Roll => Action::Roll,
                                _ => *legal_actions(&s, p).first().ok_or(RuleError::WrongPhase)?,
                            };
                            s = match fallback {
                                Action::Roll => apply_with_chance(
                                    &s,
                                    p,
                                    fallback,
                                    dice_chance(scenario.roll()),
                                )?,
                                _ => apply(&s, p, fallback)?,
                            };
                        }
                    }
                    steps += 1;
                    turn_steps += 1;
                }
                self.tick(u64::from(steps));
                arrived.push(World {
                    state: s,
                    weight: w.weight,
                });
                if self.aborted {
                    return Ok(None);
                }
            }
            for (_, group) in self.regroup(arrived) {
                let share = Self::mass(&group) / count as f64;
                arrivals.push((share, group));
            }
        }
        let ticks = self.stats.transitions - ticks_before;
        self.cache
            .rounds
            .insert(key, (arrivals.clone(), self.generation, ticks));
        Ok(Some(arrivals))
    }
    /// Simulate from an afterstate through every other seat's turn to the root player's next
    /// main phase, then plan the next level. Averages over the level's scenarios.
    pub fn round(&mut self, worlds: &[World], level: u8) -> Result<Terms, RuleError> {
        let Some(arrivals) = self.arrivals(worlds, level)? else {
            return Ok(Terms::default());
        };
        let mut total = Terms::default();
        for (share, group) in arrivals {
            let finished = matches!(group[0].state.phase, Phase::Finished { .. });
            let value = if finished || level + 1 > self.limits.depth {
                let state = group[0].state;
                let value = self.leaf(&Self::normalized(group));
                if finished && level < self.limits.depth {
                    self.note_outcome(level + 1, share, value.total(), state);
                }
                value
            } else {
                let saved = self.path_mass;
                self.path_mass *= share;
                let planned = self.plan(group, level + 1);
                self.path_mass = saved;
                planned?.0
            };
            total.add_scaled(&value, share);
        }
        Ok(total)
    }
}
