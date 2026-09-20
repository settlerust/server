use super::{ApiJson, App};
use crate::{
    auth::Identity,
    config::now_ms,
    error::{Error, Result},
};
use axum::{
    Extension, Json,
    extract::rejection::QueryRejection,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use settlers_protocol::*;
pub(super) async fn login(
    State(app): State<App>,
    Path(provider): Path<String>,
    ApiJson(credential): ApiJson<serde_json::Value>,
) -> Result<Json<TokenResponse>> {
    Ok(Json(
        app.auth
            .login(&provider, &credential, now_ms() / 1000)
            .await?,
    ))
}
pub(super) async fn refresh(
    State(app): State<App>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<TokenResponse>> {
    Ok(Json(app.auth.issue(&identity, now_ms() / 1000)?))
}
pub(super) async fn health() -> Json<Health> {
    Json(Health {
        status: HealthStatus::Ok,
        protocol: PROTOCOL_VERSION,
    })
}
pub(super) async fn topology() -> Json<&'static settlers_engine::Topology> {
    Json(settlers_engine::topology())
}
pub(super) async fn schema() -> Result<Json<serde_json::Value>> {
    Ok(Json(
        serde_json::from_str(include_str!("../../../../docs/protocol.schema.json"))
            .map_err(Error::storage)?,
    ))
}
pub(super) async fn create(
    State(app): State<App>,
    Extension(identity): Extension<Identity>,
    grant: Option<Extension<crate::managed_auth::Grant>>,
    ApiJson(mut request): ApiJson<CreateGame>,
) -> Result<Json<GameView>> {
    if let Some(Extension(grant)) = grant {
        request.player_name = Some(identity.name.clone());
        let view = app
            .games
            .create_assigned(&grant.game_id, identity.clone(), request, now_ms())
            .await?;
        if let Some(revision) = grant.name_revision {
            app.games
                .display_name(&grant.game_id, identity.clone(), revision, now_ms())
                .await?;
            return Ok(Json(
                app.games.view(&grant.game_id, &identity.subject).await?,
            ));
        }
        return Ok(Json(view));
    }
    Ok(Json(app.games.create(identity, request, now_ms()).await?))
}
pub(super) async fn display_name(
    grant: Option<Extension<crate::managed_auth::Grant>>,
) -> Result<Json<serde_json::Value>> {
    if grant.is_none_or(|Extension(grant)| grant.name_revision.is_none()) {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "a server-signed display name is required",
        ));
    }
    // Authentication middleware has already committed the grant's own name.
    Ok(Json(serde_json::json!({"updated": true})))
}
pub(super) async fn game(
    State(app): State<App>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> Result<Json<GameView>> {
    Ok(Json(app.games.view(&id, &identity.subject).await?))
}
pub(super) async fn players(
    State(app): State<App>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> Result<Json<PlayersView>> {
    let view = app.games.view(&id, &identity.subject).await?;
    Ok(Json(PlayersView {
        members: view.members,
        players: view
            .game
            .as_ref()
            .map_or_else(Vec::new, |g| g.players.clone()),
        own: view.game.and_then(|g| g.own),
    }))
}
pub(super) async fn command(
    State(app): State<App>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    grant: Option<Extension<crate::managed_auth::Grant>>,
    ApiJson(request): ApiJson<CommandRequest>,
) -> Result<Json<Receipt>> {
    if let Some(Extension(grant)) = grant {
        grant.authorize_command(&request.command)?;
    }
    Ok(Json(
        app.games.execute(&id, identity, request, now_ms()).await?,
    ))
}
pub(super) async fn admission(
    State(app): State<App>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    grant: Option<Extension<crate::managed_auth::Grant>>,
    ApiJson(request): ApiJson<AdmissionUpdate>,
) -> Result<Json<serde_json::Value>> {
    if !grant.is_some_and(|Extension(g)| g.role == crate::managed_auth::Role::Host) {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "an external host grant is required",
        ));
    }
    app.games
        .admission(&id, identity, request, now_ms())
        .await?;
    Ok(Json(serde_json::json!({"updated": true})))
}
#[derive(Deserialize)]
pub(super) struct ListQuery {
    after: Option<String>,
    limit: Option<usize>,
}
#[derive(Deserialize)]
pub(super) struct HistoryQuery {
    after: Option<u64>,
    limit: Option<usize>,
}
pub(super) async fn list(
    State(app): State<App>,
    query: std::result::Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<GameList>> {
    let Query(query) = query.map_err(|e| Error::new(ErrorCode::InvalidRequest, e.body_text()))?;
    Ok(Json(
        app.games
            .list(query.after.as_deref(), query.limit.unwrap_or(50))
            .await?,
    ))
}
pub(super) async fn history(
    State(app): State<App>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    query: std::result::Result<Query<HistoryQuery>, QueryRejection>,
) -> Result<Json<EventPage>> {
    let Query(query) = query.map_err(|e| Error::new(ErrorCode::InvalidRequest, e.body_text()))?;
    Ok(Json(
        app.games
            .history(
                &id,
                &identity.subject,
                query.after.unwrap_or(0),
                query.limit.unwrap_or(50),
            )
            .await?,
    ))
}
