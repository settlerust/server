//! Typed Rust REST/WebSocket client. Persist the token to retain the same seat after reconnects.
use futures_util::{SinkExt, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
pub use settlers_protocol::*;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP transport: {0}")]
    Http(#[from] reqwest::Error),
    #[error("WebSocket: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("server rejected request: {0:?}")]
    Server(ApiError),
    #[error("invalid client configuration: {0}")]
    Configuration(String),
}
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}
impl Client {
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Result<Self, ClientError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            base: base.into().trim_end_matches('/').into(),
            token: token.into(),
        })
    }
    pub async fn guest(base: &str, name: &str) -> Result<(Self, TokenResponse), ClientError> {
        let mut client = Self::new(base, "")?;
        let response: TokenResponse = client
            .post("/v1/auth/guest", &GuestRequest { name: name.into() })
            .await?;
        client.token = response.token.clone();
        Ok((client, response))
    }
    pub fn token(&self) -> &str {
        &self.token
    }
    pub async fn refresh(&mut self) -> Result<TokenResponse, ClientError> {
        let response: TokenResponse = self.post("/v1/auth/refresh", &()).await?;
        self.token = response.token.clone();
        Ok(response)
    }
    async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, ClientError> {
        if !response.status().is_success() {
            return Err(ClientError::Server(response.json().await?));
        }
        Ok(response.json().await?)
    }
    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        Self::decode(
            self.http
                .get(format!("{}{path}", self.base))
                .bearer_auth(&self.token)
                .send()
                .await?,
        )
        .await
    }
    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T, ClientError> {
        Self::decode(
            self.http
                .post(format!("{}{path}", self.base))
                .bearer_auth(&self.token)
                .json(body)
                .send()
                .await?,
        )
        .await
    }
    pub async fn create(&self, request: &CreateGame) -> Result<GameView, ClientError> {
        self.post("/v1/games", request).await
    }
    pub async fn game(&self, id: &str) -> Result<GameView, ClientError> {
        self.get(&format!("/v1/games/{id}")).await
    }
    pub async fn games(&self) -> Result<GameList, ClientError> {
        self.get("/v1/games").await
    }
    pub fn prepare(version: u64, command: Command) -> CommandRequest {
        CommandRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            expected_version: version,
            command,
        }
    }
    /// On an ambiguous failure, resend this exact request; do not generate another UUID.
    pub async fn submit(&self, id: &str, request: &CommandRequest) -> Result<Receipt, ClientError> {
        self.post(&format!("/v1/games/{id}/commands"), request)
            .await
    }
    pub async fn command(
        &self,
        id: &str,
        version: u64,
        command: Command,
    ) -> Result<Receipt, ClientError> {
        self.submit(id, &Self::prepare(version, command)).await
    }
    pub async fn history(
        &self,
        id: &str,
        after: u64,
        limit: usize,
    ) -> Result<EventPage, ClientError> {
        self.get(&format!(
            "/v1/games/{id}/events?after={after}&limit={limit}"
        ))
        .await
    }
    pub async fn connect(&self, id: &str) -> Result<GameSocket, ClientError> {
        let url = format!("{}/v1/games/{id}/ws", self.base.replacen("http", "ws", 1));
        let mut request = url.into_client_request()?;
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {}", self.token)
                .parse()
                .map_err(|_| ClientError::Configuration("invalid token header".into()))?,
        );
        request.headers_mut().insert(
            "sec-websocket-protocol",
            "settlers-v1"
                .parse()
                .map_err(|_| ClientError::Configuration("invalid protocol".into()))?,
        );
        let (socket, _) = connect_async(request).await?;
        Ok(GameSocket { socket })
    }
}
pub struct GameSocket {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}
impl GameSocket {
    pub async fn send(&mut self, frame: &ClientFrame) -> Result<(), ClientError> {
        self.socket
            .send(Message::Text(serde_json::to_string(frame)?.into()))
            .await?;
        Ok(())
    }
    pub async fn next(&mut self) -> Result<Option<ServerFrame>, ClientError> {
        while let Some(message) = self.socket.next().await {
            match message? {
                Message::Text(text) => return Ok(Some(serde_json::from_str(&text)?)),
                Message::Ping(data) => self.socket.send(Message::Pong(data)).await?,
                Message::Close(_) => return Ok(None),
                _ => {}
            }
        }
        Ok(None)
    }
    pub async fn close(&mut self) -> Result<(), ClientError> {
        self.socket.close(None).await?;
        Ok(())
    }
}
