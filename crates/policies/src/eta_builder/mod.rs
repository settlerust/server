use crate::Policy;
use settlers_engine::{Action, Observation};
/// Analytical baseline: rank construction by estimated acquisition time and production gain.
#[derive(Default)]
pub struct EtaBuilder;
impl Policy for EtaBuilder {
    fn name(&self) -> &'static str {
        "eta_builder"
    }
    fn choose_action(&mut self, observation: &Observation) -> Option<Action> {
        crate::planning::choose(observation, true)
    }
}
