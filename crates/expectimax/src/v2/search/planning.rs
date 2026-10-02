//! Own-turn planning shares transpositions across iterative depths.
use super::*;
impl<'a> Search<'a> {
    /// Actions to keep at a given step of a turn. The current turn keeps every first action,
    /// then `EndTurn` plus the best few by static value; later turns follow one greedy line.
    /// `None` means keep everything; `Some(0)` means the single statically best action.
    pub(super) fn step_cap(&self, level: u8, steps: u32) -> Option<usize> {
        if level > 1 {
            return Some(0);
        }
        match steps {
            0 => None,
            1 => Some(3),
            2 => Some(2),
            3 => Some(1),
            _ => Some(usize::MAX),
        }
    }
    /// The statically best continuation of the root player's turn from a group: the maximum
    /// over own actions of the expectation over chance outcomes, valued by the leaf at every
    /// end-of-turn position. Returns the value and the afterstate groups of the best line,
    /// each with its probability mass (chance inside the turn splits the line).
    pub fn best_line(
        &mut self,
        worlds: Vec<World>,
        level: u8,
        steps: u32,
    ) -> Result<(f64, Line), RuleError> {
        let s = worlds[0].state;
        let key = information_set(&s, self.seat);
        if let Some((value, line, generation, ticks)) = self.cache.lines.get(&key) {
            let (hit, generation, ticks) = ((*value, line.clone()), *generation, *ticks);
            self.stats.transpositions += 1;
            self.reuse(generation, ticks);
            return Ok(hit);
        }
        let ticks_before = self.stats.transitions;
        let worlds = Self::normalized(worlds);
        let p = actor(&s);
        let ended = matches!(s.phase, Phase::Finished { .. })
            || (p != self.seat && !matches!(s.phase, Phase::Discard { .. }));
        if ended || self.aborted {
            let value = self.leaf(&worlds).total();
            return Ok((value, vec![(1.0, worlds)]));
        }
        if p != self.seat {
            // Another seat must discard before our turn continues.
            let mut children = Vec::with_capacity(worlds.len());
            for w in &worlds {
                let a = opponent::act(&w.state, p, 0);
                children.push(World {
                    state: apply(&w.state, p, a)?,
                    weight: w.weight,
                });
                self.tick(1);
            }
            let mut value = 0.0;
            let mut lines = Vec::new();
            for (_, group) in self.regroup(children) {
                let mass = Self::mass(&group);
                let (v, sub) = self.best_line(group, level, steps)?;
                value += mass * v;
                lines.extend(sub.into_iter().map(|(m, g)| (m * mass, g)));
            }
            return Ok((value, lines));
        }
        self.note_node(level);
        let actions = match self.step_cap(level, steps) {
            None => self.candidates(&s, p, steps),
            Some(usize::MAX) => vec![Action::EndTurn],
            Some(cap) => {
                let all = self.candidates(&s, p, steps);
                self.narrow(&s, p, all, cap)
            }
        };
        let mut best: Option<(f64, Line)> = None;
        for a in actions {
            let children = match self.step(&worlds, p, a) {
                Ok(children) => children,
                Err(_) => continue,
            };
            let mut value = 0.0;
            let mut lines = Vec::new();
            for (_, group) in self.regroup(children) {
                let mass = Self::mass(&group);
                let (v, sub) = self.best_line(group, level, steps + 1)?;
                value += mass * v;
                lines.extend(sub.into_iter().map(|(m, g)| (m * mass, g)));
            }
            if best.as_ref().is_none_or(|b| value > b.0) {
                best = Some((value, lines));
            }
        }
        let result = best.unwrap_or_else(|| (self.leaf(&worlds).total(), vec![(1.0, worlds)]));
        if !self.aborted {
            let ticks = self.stats.transitions - ticks_before;
            self.cache
                .lines
                .insert(key, (result.0, result.1.clone(), self.generation, ticks));
        }
        Ok(result)
    }
    /// Keep the `cap` actions whose immediate static value is highest, always keeping
    /// `EndTurn` when `cap` is positive; `cap == 0` keeps only the single best action.
    pub(super) fn narrow(
        &mut self,
        s: &State,
        p: u8,
        actions: Vec<Action>,
        cap: usize,
    ) -> Vec<Action> {
        if actions.len() <= cap + 1 {
            return actions;
        }
        let here = self.value(s, self.seat).total();
        self.stats.leaves += 1;
        let mut scored: Vec<(f64, Action)> = actions
            .into_iter()
            .map(|a| {
                let value = if a == Action::EndTurn {
                    if cap > 0 { f64::INFINITY } else { here }
                } else {
                    match apply(s, p, a) {
                        Ok(next) => {
                            self.stats.leaves += 1;
                            self.value(&next, self.seat).total()
                        }
                        Err(_) => f64::NEG_INFINITY,
                    }
                };
                (value, a)
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
            .into_iter()
            .take(cap.max(1) + usize::from(cap > 0))
            .map(|(_, a)| a)
            .collect()
    }
    /// Value of the root player's turn with lookahead: the statically best line's afterstates
    /// are followed through the opponents' round and the next planned level. At the deepest
    /// level the static value is the answer.
    pub fn turn_value(
        &mut self,
        worlds: Vec<World>,
        level: u8,
        steps: u32,
    ) -> Result<(Terms, Option<Action>), RuleError> {
        let key = (
            information_set(&worlds[0].state, self.seat),
            level,
            self.limits.depth,
        );
        if let Some((v, recorded, generation, ticks)) = self.cache.memo.get(&key) {
            let (v, recorded, generation, ticks) =
                (v.clone(), recorded.clone(), *generation, *ticks);
            self.stats.transpositions += 1;
            self.reuse(generation, ticks);
            if self.record {
                for (share, value, state) in recorded {
                    self.note_outcome(level, share, value, state);
                }
            }
            return Ok((v, None));
        }
        let ticks_before = self.stats.transitions;
        let worlds = Self::normalized(worlds);
        let (_, lines) = self.best_line(worlds.clone(), level, steps)?;
        let mass_total: f64 = lines
            .iter()
            .filter(|(m, _)| *m > 1e-12)
            .map(|(m, _)| m)
            .sum();
        let mut total = Terms::default();
        let mut recorded: Recorded = Vec::new();
        for (mass, group) in lines {
            if mass <= 1e-12 {
                continue;
            }
            let group = Self::normalized(group);
            let share = mass / mass_total;
            let saved = self.path_mass;
            self.path_mass *= share;
            let value = self.afterstate_value(&group, level);
            self.path_mass = saved;
            let value = value?;
            if self.record {
                recorded.push((share, value.total(), group[0].state));
                self.note_outcome(level, share, value.total(), group[0].state);
            }
            total.add_scaled(&value, mass);
        }
        if mass_total > 0.0 {
            total.scale(1.0 / mass_total);
        } else {
            total = self.leaf(&worlds);
        }
        if !self.aborted {
            let ticks = self.stats.transitions - ticks_before;
            self.cache
                .memo
                .insert(key, (total.clone(), recorded, self.generation, ticks));
        }
        Ok((total, None))
    }
    pub(super) fn afterstate_value(
        &mut self,
        worlds: &[World],
        level: u8,
    ) -> Result<Terms, RuleError> {
        self.note_afterstate(level);
        if level >= self.limits.depth
            || matches!(worlds[0].state.phase, Phase::Finished { .. })
            || self.aborted
        {
            return Ok(self.leaf(worlds));
        }
        self.stats.expanded += 1;
        self.round(worlds, level)
    }
    /// Plan a whole turn from its first decision.
    pub fn plan(
        &mut self,
        worlds: Vec<World>,
        level: u8,
    ) -> Result<(Terms, Option<Action>), RuleError> {
        self.turn_value(worlds, level, 0)
    }
    pub(super) fn note_node(&mut self, level: u8) {
        let i = level as usize;
        if self.stats.per_level_nodes.len() <= i {
            self.stats.per_level_nodes.resize(i + 1, 0);
            self.stats.per_level_afterstates.resize(i + 1, 0);
        }
        self.stats.per_level_nodes[i] += 1;
        self.stats.own_nodes += 1;
    }
    pub(super) fn note_afterstate(&mut self, level: u8) {
        let i = level as usize;
        if self.stats.per_level_nodes.len() <= i {
            self.stats.per_level_nodes.resize(i + 1, 0);
            self.stats.per_level_afterstates.resize(i + 1, 0);
        }
        self.stats.per_level_afterstates[i] += 1;
        self.stats.afterstates += 1;
    }
}
