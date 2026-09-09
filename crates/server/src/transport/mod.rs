mod routes;
mod websocket;
use crate::{
    auth::{Auth, Identity},
    error::{Error, Result},
    service::GameService,
};
use axum::{
    Router,
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use settlers_protocol::ErrorCode;
#[derive(Clone)]
pub struct App {
    pub auth: Auth,
    pub games: GameService,
}
#[derive(Clone)]
struct Bearer(String);
pub fn router(app: App) -> Router {
    let protected = Router::new()
        .route("/v1/auth/refresh", post(routes::refresh))
        .route("/v1/health", get(routes::health))
        .route("/v1/topology", get(routes::topology))
        .route("/v1/schema", get(routes::schema))
        .route("/v1/games", get(routes::list).post(routes::create))
        .route("/v1/games/{id}", get(routes::game))
        .route("/v1/games/{id}/state", get(routes::game))
        .route("/v1/games/{id}/players", get(routes::players))
        .route("/v1/games/{id}/commands", post(routes::command))
        .route("/v1/games/{id}/admission", post(routes::admission))
        .route("/v1/games/{id}/events", get(routes::history))
        .route("/v1/games/{id}/ws", get(websocket::upgrade))
        .route_layer(middleware::from_fn_with_state(app.clone(), authenticate));
    Router::new()
        .merge(protected)
        .route("/healthz", get(routes::health))
        .route("/v1/auth/{provider}", post(routes::login))
        .fallback(|| async { Error::new(ErrorCode::NotFound, "route not found") })
        .method_not_allowed_fallback(|| async {
            Error::new(ErrorCode::InvalidRequest, "method not allowed")
        })
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024))
        .layer(cors())
        .with_state(app)
}
async fn authenticate(
    State(app): State<App>,
    mut request: Request,
    next: Next,
) -> Result<Response> {
    let token = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::to_owned)
        .or_else(|| {
            request
                .uri()
                .path()
                .ends_with("/ws")
                .then(|| {
                    request
                        .headers()
                        .get("sec-websocket-protocol")
                        .and_then(|h| h.to_str().ok())
                        .and_then(|h| {
                            h.split(',')
                                .find_map(|p| p.trim().strip_prefix("bearer.").map(str::to_owned))
                        })
                })
                .flatten()
        })
        .ok_or_else(|| Error::new(ErrorCode::Unauthorized, "Bearer JWT is required"))?;
    let now = crate::config::now_ms() / 1000;
    let identity: Identity = if let Some(grant) = app.auth.external_grant(&token, now)? {
        grant.authorize_path(
            request.uri().path(),
            request.method() == axum::http::Method::POST,
        )?;
        let identity = grant.identity();
        request.extensions_mut().insert(grant);
        identity
    } else {
        app.auth.verify(&token, now)?
    };
    request.extensions_mut().insert(identity);
    request.extensions_mut().insert(Bearer(token));
    Ok(next.run(request).await)
}
/// Normalize malformed JSON into the same strongly typed error envelope as rule errors.
pub struct ApiJson<T>(pub T);
impl<S, T> axum::extract::FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned,
{
    type Rejection = Error;
    async fn from_request(request: Request, state: &S) -> Result<Self> {
        let axum::Json(value) = axum::Json::<T>::from_request(request, state)
            .await
            .map_err(|e| Error::new(ErrorCode::InvalidRequest, e.body_text()))?;
        Ok(Self(value))
    }
}

fn cors() -> tower_http::cors::CorsLayer {
    use axum::http::{HeaderValue, Method, header};
    let origins: Vec<HeaderValue> = std::env::var("SETTLERS_ALLOWED_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|origin| origin.trim().parse().ok())
        .collect();
    tower_http::cors::CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}
