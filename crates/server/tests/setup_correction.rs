mod common;
use common::*;
use server::{reducer, storage::Store};
use settlers_engine::{Action, legal_actions};
use settlers_protocol::*;

#[tokio::test]
async fn setup_corrections_keep_the_deadline_and_replay_exactly() {
    let h = Harness::new().await;
    h.start().await;
    let room = h.room().await;
    let game = room.game.unwrap();
    h.command(
        game.current,
        Command::Act {
            action: legal_actions(&game, game.current)[0],
        },
    )
    .await;
    let placed = h.room().await;
    let game = placed.game.unwrap();
    let action = legal_actions(&game, game.current)
        .into_iter()
        .find(|a| matches!(a, Action::RelocateSettlement { .. }))
        .unwrap();
    h.service
        .execute(
            &h.id,
            identity(game.current),
            request(placed.version, Command::Act { action }),
            5000,
        )
        .await
        .unwrap();
    let moved = h.room().await;
    assert_eq!(moved.deadline_ms, placed.deadline_ms);
    let replay = reducer::replay(&h.store.pending(&h.id).await.unwrap()).unwrap();
    assert_eq!(
        reducer::state_hash(&replay).unwrap(),
        reducer::state_hash(&moved).unwrap()
    );
}
