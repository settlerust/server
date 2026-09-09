//! Table conversation for remote opponents; decisions use only their redacted view.
use crate::{Policy, planning};
use settlers_engine::{Action, Observation, Phase, Resources};
use settlers_protocol::{Command, Communication, GameView, Intent, PublicEvent};

pub struct TradingPolicy {
    inner: Box<dyn Policy>,
    offered_turn: Option<u32>,
    preferred_partner: Option<u8>,
}
impl TradingPolicy {
    pub fn new(inner: Box<dyn Policy>) -> Self {
        Self {
            inner,
            offered_turn: None,
            preferred_partner: None,
        }
    }
    /// Invite the local human first, giving them time to consider a swap.
    pub fn with_preferred_partner(mut self, seat: Option<u8>) -> Self {
        self.preferred_partner = seat;
        self
    }
}
impl Policy for TradingPolicy {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn choose_action(&mut self, observation: &Observation) -> Option<Action> {
        self.inner.choose_action(observation)
    }
    fn on_event(&mut self, event: &PublicEvent) {
        self.inner.on_event(event);
    }
    fn on_batch(&mut self, batch: &settlers_protocol::EventBatch) {
        self.inner.on_batch(batch);
    }
    fn committed(&mut self, command: &Command, view: &GameView) {
        self.inner.committed(command, view);
    }
    fn choose(&mut self, view: &GameView) -> Option<Command> {
        let next = self.inner.choose(view);
        let Some(o) = &view.game else {
            return next;
        };
        if o.phase != Phase::Main
            || o.seat != Some(o.current)
            || self.offered_turn == Some(o.turn)
            || !matches!(
                next,
                Some(Command::Act {
                    action: Action::EndTurn | Action::BankTrade { .. }
                })
            )
        {
            return next;
        }
        let Some(own) = o.own else {
            return next;
        };
        let cost = planning::target(o, self.inner.name().contains("eta")).1;
        let wanted = (0..5)
            .filter(|&r| own.resources.0[r] < cost.0[r])
            .max_by_key(|&r| cost.0[r] - own.resources.0[r]);
        let surplus = (0..5)
            .filter(|&r| own.resources.0[r] > cost.0[r])
            .max_by_key(|&r| own.resources.0[r] - cost.0[r]);
        let (Some(wanted), Some(surplus)) = (wanted, surplus) else {
            return next;
        };
        let mut to: Vec<u8> = o
            .players
            .iter()
            .filter(|p| p.seat != o.current && p.resource_count > 0)
            .map(|p| p.seat)
            .collect();
        if let Some(partner) = self.preferred_partner.filter(|p| to.contains(p)) {
            to = vec![partner];
        }
        if to.is_empty() {
            return next;
        }
        let mut give = Resources::default();
        let mut receive = Resources::default();
        give.0[surplus] = 1;
        receive.0[wanted] = 1;
        self.offered_turn = Some(o.turn);
        Some(Command::OfferTrade {
            to,
            give,
            receive,
            message: Some(Communication {
                intent: Intent::RequestTrade,
                resources: Some(receive),
                text: Some("A swap toward my next build?".into()),
            }),
        })
    }
}
