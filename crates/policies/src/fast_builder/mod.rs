use crate::Policy;
use settlers_engine::{Action, Observation};
/// Construction-first baseline: production-rich settlements/cities and short expansion paths.
#[derive(Default)]
pub struct FastBuilder;
impl Policy for FastBuilder {
    fn name(&self) -> &'static str {
        "fast_builder"
    }
    fn choose_action(&mut self, observation: &Observation) -> Option<Action> {
        crate::planning::choose(observation, false)
    }
}
