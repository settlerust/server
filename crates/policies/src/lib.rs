//! Fair heuristic baselines. Every policy consumes the same redacted view as a remote player.
pub mod eta_builder;
pub mod expectimax;
pub mod fast_builder;
mod planning;
pub mod trading;
use settlers_engine::Observation;
use settlers_protocol::*;
/// Hooks for game decisions, offer evaluation, structured conversation, and public observations.
pub trait Policy: Send {
    fn name(&self) -> &'static str;
    fn choose_action(&mut self, observation: &Observation) -> Option<Action>;
    fn on_event(&mut self, _event: &PublicEvent) {}
    /// A delivered event batch with its sequence; the default forwards each event.
    fn on_batch(&mut self, batch: &EventBatch) {
        for event in &batch.events {
            self.on_event(event);
        }
    }
    fn accept_trade(&mut self, observation: &Observation, offer: &TradeOffer) -> bool {
        let Some(own) = observation.own else {
            return false;
        };
        let target = planning::target(observation, false).1;
        if !own.resources.contains(offer.receive) {
            return false;
        }
        let mut after = own.resources;
        for i in 0..5 {
            after.0[i] = after.0[i] - offer.receive.0[i] + offer.give.0[i];
        }
        planning::deficit(after, target) < planning::deficit(own.resources, target)
    }
    fn conversation(&mut self, _observation: &Observation) -> Option<Communication> {
        None
    }
    /// The server accepted a command chosen by this policy.
    fn committed(&mut self, _command: &Command, _view: &GameView) {}
    fn choose(&mut self, view: &GameView) -> Option<Command> {
        let observation = view.game.as_ref()?;
        let seat = observation.seat?;
        for offer in &view.offers {
            if offer.to.contains(&seat)
                && offer.status == OfferStatus::Open
                && self.accept_trade(observation, offer)
            {
                return Some(Command::AcceptTrade {
                    offer_id: offer.id.clone(),
                });
            }
        }
        if let Some(message) = self.conversation(observation) {
            return Some(Command::Communicate { to: None, message });
        }
        self.choose_action(observation)
            .map(|action| Command::Act { action })
    }
}
