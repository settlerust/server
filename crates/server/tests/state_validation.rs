mod common;
use common::*;
use server::{browser::BrowserRoom, model::Room, reducer, service::GameService, storage::Store};
fn broken(room: &Room) -> Vec<Room> {
    let mut duplicate_identity = room.clone();
    duplicate_identity.members.push(room.members[0].clone());
    duplicate_identity.members[1].seat = None;
    let mut duplicate_seat = room.clone();
    let mut member = room.members[0].clone();
    member.identity.subject = "other".into();
    duplicate_seat.members.push(member);
    let mut outside_seat = room.clone();
    outside_seat.members[0].seat = Some(255);
    let mut bad_settings = room.clone();
    bad_settings.settings.players = 5;
    let mut schema = room.clone();
    schema.schema = 2;
    let mut mismatch = room.clone();
    mismatch.game = Some(settlers_engine::State::new(Default::default(), [0; 32]).unwrap());
    mismatch.settings.points_to_win = 11;
    vec![
        duplicate_identity,
        duplicate_seat,
        outside_seat,
        bad_settings,
        schema,
        mismatch,
    ]
}
#[tokio::test]
async fn recovery_rejects_corrupt_rooms_before_spawning_actors() {
    let h = Harness::new().await;
    let room = h.room().await;
    for corrupt in broken(&room) {
        assert!(corrupt.validate().is_err());
    }
    let mut corrupt = room.clone();
    corrupt.members[0].seat = Some(255);
    corrupt.version += 1;
    let mut record = h.store.pending(&h.id).await.unwrap()[0].clone();
    record.sequence = corrupt.version;
    record.state_hash = reducer::state_hash(&corrupt).unwrap();
    h.store
        .commit(room.version, &corrupt, &record)
        .await
        .unwrap();
    assert!(
        GameService::restore(h.store.clone(), h.archive.clone(), 0)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn checkpoint_restore_validates_before_projection_or_clocks() {
    let h = Harness::new().await;
    let room = h.room().await;
    for corrupt in broken(&room) {
        let checkpoint = serde_json::json!({"room":corrupt,"records":[]}).to_string();
        assert!(BrowserRoom::restore(&checkpoint, 0.0, 1.0).is_err());
    }
    assert!(
        BrowserRoom::restore(
            &serde_json::json!({"room":room,"records":[]}).to_string(),
            0.0,
            1.0
        )
        .is_ok()
    );
}
