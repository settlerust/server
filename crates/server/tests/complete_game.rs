mod common;
use common::*;
use server::{reducer, storage::Archive};
use settlers_policies::{Policy, eta_builder::EtaBuilder, fast_builder::FastBuilder};
use settlers_protocol::*;
#[tokio::test]
async fn remote_policy_contract_finishes_ten_point_game_and_replays_entire_room() {
    let h = Harness::new().await;
    h.start().await;
    let mut fast = FastBuilder;
    let mut eta = EtaBuilder;
    for step in 0..15000 {
        let room = h.room().await;
        let game = room.game.unwrap();
        if matches!(game.phase, Phase::Finished { .. }) {
            break;
        }
        let p = if let Phase::Discard { pending } = game.phase {
            pending.trailing_zeros() as u8
        } else {
            game.current
        };
        let view = h.service.view(&h.id, &identity(p).subject).await.unwrap();
        let command = if p % 2 == 0 {
            fast.choose(&view)
        } else {
            eta.choose(&view)
        }
        .unwrap();
        h.command(p, command).await;
        if step % 40 == 0 {
            h.service.flush_archive().await.unwrap();
        }
    }
    let room = h.room().await;
    assert!(matches!(room.game.unwrap().phase, Phase::Finished { .. }));
    h.service.flush_archive().await.unwrap();
    let records = h.archive.read(&h.id).await.unwrap();
    let replay = reducer::replay(&records).unwrap();
    assert_eq!(
        reducer::state_hash(&replay).unwrap(),
        reducer::state_hash(&room).unwrap()
    );
}
