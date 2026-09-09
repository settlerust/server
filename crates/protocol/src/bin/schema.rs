use schemars::{JsonSchema, schema_for};
use serde::Serialize;
use settlers_protocol::*;
#[derive(Serialize, JsonSchema)]
struct Protocol {
    health: Health,
    client_frame: ClientFrame,
    server_frame: ServerFrame,
    create_game: CreateGame,
    admission_update: AdmissionUpdate,
    guest_request: GuestRequest,
    token_response: TokenResponse,
    command_request: CommandRequest,
    game_view: GameView,
    event_page: EventPage,
    game_list: GameList,
    players_view: PlayersView,
    topology: settlers_engine::Topology,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(&schema_for!(Protocol))?);
    Ok(())
}
