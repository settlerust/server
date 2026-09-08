use super::{App, Bearer};
use crate::{
    auth::Identity,
    config::now_ms,
    error::{Error, Result},
};
use axum::{
    Extension,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use settlers_protocol::*;
use std::time::Duration;
use tokio::sync::broadcast;
pub(super) async fn upgrade(
    State(app): State<App>,
    Extension(identity): Extension<Identity>,
    Extension(bearer): Extension<Bearer>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response> {
    let receiver = app.games.subscribe(&id, &identity.subject).await?;
    Ok(ws
        .protocols(["settlers-v1"])
        .max_message_size(16 * 1024)
        .max_frame_size(16 * 1024)
        .on_upgrade(move |socket| run(socket, app, id, identity, bearer, receiver)))
}
async fn send(socket: &mut WebSocket, frame: ServerFrame) -> Result<()> {
    let text = serde_json::to_string(&frame).map_err(Error::storage)?;
    send_message(socket, Message::Text(text.into())).await
}
async fn send_message(socket: &mut WebSocket, message: Message) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), socket.send(message))
        .await
        .map_err(Error::storage)?
        .map_err(Error::storage)
}
async fn snapshot(socket: &mut WebSocket, app: &App, id: &str, subject: &str) -> Result<u64> {
    let game = app.games.view(id, subject).await?;
    let version = game.version;
    send(
        socket,
        ServerFrame::Snapshot {
            protocol: PROTOCOL_VERSION,
            game: Box::new(game),
        },
    )
    .await?;
    Ok(version)
}
async fn run(
    mut socket: WebSocket,
    app: App,
    id: String,
    identity: Identity,
    bearer: Bearer,
    mut receiver: broadcast::Receiver<std::sync::Arc<crate::model::Record>>,
) {
    let session = uuid::Uuid::new_v4().to_string();
    if app
        .games
        .presence(
            &id,
            identity.subject.clone(),
            session.clone(),
            true,
            now_ms(),
        )
        .await
        .is_err()
    {
        let _ = app
            .games
            .presence(
                &id,
                identity.subject.clone(),
                session.clone(),
                false,
                now_ms(),
            )
            .await;
        return;
    }
    let result = stream(&mut socket, &app, &id, &identity, &bearer, &mut receiver).await;
    if let Err(error) = result {
        tracing::debug!(code=?error.0.code,game_id=%id,"socket ended");
    }
    let _ = app
        .games
        .presence(&id, identity.subject, session, false, now_ms())
        .await;
}
async fn stream(
    socket: &mut WebSocket,
    app: &App,
    id: &str,
    identity: &Identity,
    bearer: &Bearer,
    receiver: &mut broadcast::Receiver<std::sync::Arc<crate::model::Record>>,
) -> Result<()> {
    let mut event_cursor = snapshot(socket, app, id, &identity.subject).await?;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    let mut last_heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                app.auth.verify(&bearer.0, now_ms() / 1000)?;
                if last_heard.elapsed() > Duration::from_secs(20) {
                    return Ok(());
                }
                send_message(socket, Message::Ping(vec![1].into())).await?;
            },
            incoming = socket.recv() => {
                let Some(message) = incoming else { return Ok(()); };
                let message = message.map_err(Error::storage)?;
                last_heard = tokio::time::Instant::now();
                app.auth.verify(&bearer.0, now_ms() / 1000)?;
                match message {
                    Message::Text(text) => {
                        if let Some(version) = handle_text(socket, app, id, identity, bearer, &text).await? {
                            event_cursor = version;
                        }
                    }
                    Message::Close(_) => return Ok(()),
                    Message::Ping(bytes) => send_message(socket, Message::Pong(bytes)).await?,
                    Message::Pong(_) => {}
                    Message::Binary(_) => {
                        send(socket, invalid_frame("JSON text frames are required")).await?;
                    }
                }
            },
            notification = receiver.recv() => {
                match notification {
                    Ok(record) => {
                        if record.sequence <= event_cursor {
                            continue;
                        }
                        let room = app.games.room(id).await?;
                        let batch = crate::reducer::batch(&room, &identity.subject, &record)?;
                        send(socket, ServerFrame::Events { batch }).await?;
                        event_cursor = record.sequence;
                        // A newer private snapshot must not skip queued public event batches.
                        snapshot(socket, app, id, &identity.subject).await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        send(socket, ServerFrame::ResyncRequired).await?;
                        event_cursor = snapshot(socket, app, id, &identity.subject).await?;
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        }
    }
}
fn invalid_frame(message: &str) -> ServerFrame {
    ServerFrame::Error {
        request_id: None,
        error: Error::new(ErrorCode::InvalidRequest, message).0,
    }
}
/// Only explicit resync replaces the public event cursor with the current snapshot version.
async fn handle_text(
    socket: &mut WebSocket,
    app: &App,
    id: &str,
    identity: &Identity,
    bearer: &Bearer,
    text: &str,
) -> Result<Option<u64>> {
    let frame = match serde_json::from_str::<ClientFrame>(text) {
        Ok(frame) => frame,
        Err(_) => {
            send(socket, invalid_frame("invalid client frame")).await?;
            return Ok(None);
        }
    };
    let response = match frame {
        ClientFrame::Command { request } => {
            let request_id = request.request_id.clone();
            if let Some(grant) = app.auth.external_grant(&bearer.0, now_ms() / 1000)?
                && let Err(error) = grant.authorize_command(&request.command)
            {
                send(
                    socket,
                    ServerFrame::Error {
                        request_id: Some(request_id),
                        error: error.0,
                    },
                )
                .await?;
                return Ok(None);
            }
            match app
                .games
                .execute(id, identity.clone(), request, now_ms())
                .await
            {
                Ok(receipt) => ServerFrame::Ack { receipt },
                Err(error) => ServerFrame::Error {
                    request_id: Some(request_id),
                    error: error.0,
                },
            }
        }
        ClientFrame::Ping { nonce } => {
            if nonce.len() > 128 {
                return Err(Error::new(ErrorCode::InvalidRequest, "ping nonce too long"));
            }
            ServerFrame::Pong { nonce }
        }
        ClientFrame::Resync => {
            return snapshot(socket, app, id, &identity.subject).await.map(Some);
        }
    };
    send(socket, response).await?;
    Ok(None)
}
