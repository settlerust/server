//! Run an authenticated four-policy match through a running HTTP server.
use settlers_client::*;
use settlers_policies::{Policy, eta_builder::EtaBuilder, fast_builder::FastBuilder};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:8080".into());
    let (host, _) = Client::guest(&base, "Arena observer").await?;
    let view = host
        .create(&CreateGame {
            research: true,
            name: "Heuristic arena".into(),
            settings: Settings::default(),
            host_as_player: false,
            player_name: None,
        })
        .await?;
    let id = view.id;
    let mut clients = vec![];
    for p in 0..4 {
        let (client, _) = Client::guest(&base, &format!("Policy {p}")).await?;
        let view = host.game(&id).await?;
        client
            .command(
                &id,
                view.version,
                Command::JoinAgent {
                    name: format!("Policy {p}"),
                },
            )
            .await?;
        clients.push(client);
    }
    for client in &clients {
        let mut socket = client.connect(&id).await?;
        socket.next().await?.ok_or("missing presence snapshot")?;
        client
            .command(
                &id,
                client.game(&id).await?.version,
                Command::SetReady { ready: true },
            )
            .await?;
        tokio::spawn(async move { while let Ok(Some(_)) = socket.next().await {} });
    }
    host.command(&id, host.game(&id).await?.version, Command::Start)
        .await?;
    println!("game_id={id}");
    let mut fast = FastBuilder;
    let mut eta = EtaBuilder;
    for _ in 0..20000 {
        let view = host.game(&id).await?;
        let game = view.game.as_ref().ok_or("missing game")?;
        if matches!(game.phase, Phase::Finished { .. }) {
            println!("{}", serde_json::to_string(game)?);
            return Ok(());
        }
        let p = if let Phase::Discard { pending } = game.phase {
            pending.trailing_zeros() as u8
        } else {
            game.current
        };
        let view = clients[p as usize].game(&id).await?;
        let command = if p % 2 == 0 {
            fast.choose(&view)
        } else {
            eta.choose(&view)
        }
        .ok_or("policy stalled")?;
        let request = Client::prepare(view.version, command);
        loop {
            match clients[p as usize].submit(&id, &request).await {
                Ok(_) => break,
                Err(ClientError::Server(e)) if e.code == ErrorCode::Backpressure => {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    Err("match exceeded 20000 actions".into())
}
