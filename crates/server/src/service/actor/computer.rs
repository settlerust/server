//! Blocking policies work from a captured view while the actor continues processing requests.
use super::*;
type WorkerResult = std::result::Result<super::super::expert::Decision, tokio::task::JoinError>;
impl Actor {
    pub(super) async fn wait_for_computer(&mut self) -> WorkerResult {
        match self.computer.as_mut() {
            Some(task) => task.await,
            None => std::future::pending().await,
        }
    }
    pub(super) async fn finish_computer(&mut self, result: WorkerResult) {
        self.computer = None;
        let Ok(Ok((seat, request))) = result else {
            return;
        };
        // Intervening commands invalidate decisions made from an earlier snapshot.
        if request.expected_version != self.room.version {
            return;
        }
        let offered = matches!(request.command, Command::OfferTrade { .. });
        let input = Input::ComputerCommand { seat, request };
        if commit(
            &mut self.room,
            &self.store,
            &self.events,
            input,
            crate::config::now_ms(),
        )
        .await
        .is_ok()
            && offered
        {
            self.offered
                .insert(seat, self.room.game.map_or(0, |game| game.turn));
        }
    }
}
