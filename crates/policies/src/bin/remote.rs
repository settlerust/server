use settlers_client::{Client, ClientError, Command, ErrorCode, ServerFrame};
use settlers_policies::expectimax::ExpectimaxSeat;
use settlers_policies::trading::TradingPolicy;
use settlers_policies::{Policy, eta_builder::EtaBuilder, fast_builder::FastBuilder};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 6 {
        return Err(
            "usage: remote BASE_URL GAME_ID fast|eta|v1|v2|v1:{json}|v2:{json} TOKEN_FILE PLAYER_NAME"
                .into(),
        );
    }
    let base = &args[1];
    let id = &args[2];
    let token_path = std::path::Path::new(&args[4]);
    let client = match std::fs::read_to_string(token_path) {
        Ok(token) => Client::new(base, token.trim())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (client, token) = Client::guest(base, &args[5]).await?;
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(token_path)?;
            file.write_all(token.token.as_bytes())?;
            file.sync_all()?;
            client
        }
        Err(e) => return Err(e.into()),
    };
    let human = std::env::var("SETTLERS_HUMAN_SEAT")
        .ok()
        .and_then(|v| v.parse::<u8>().ok());
    let mut policy: Box<dyn Policy> = match args[3].as_str() {
        "fast" => Box::new(TradingPolicy::new(Box::new(FastBuilder)).with_preferred_partner(human)),
        "eta" => Box::new(TradingPolicy::new(Box::new(EtaBuilder)).with_preferred_partner(human)),
        spec => Box::new(ExpectimaxSeat::parse(spec)?),
    };
    let move_delay = std::env::var("SETTLERS_MOVE_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(5000);
    let trade_wait = std::env::var("SETTLERS_TRADE_WAIT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(30000);
    loop {
        let view = client.game(id).await?;
        if view.members.iter().any(|m| m.is_you) {
            break;
        }
        match client
            .command(
                id,
                view.version,
                Command::JoinAgent {
                    name: args[5].clone(),
                },
            )
            .await
        {
            Ok(_) => break,
            Err(ClientError::Server(error)) if error.code == ErrorCode::Conflict => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let mut delay = 250;
    loop {
        match client.connect(id).await {
            Ok(mut socket) => {
                delay = 250;
                loop {
                    let frame = match socket.next().await {
                        Ok(Some(frame)) => frame,
                        Ok(None) => break,
                        Err(error) => {
                            eprintln!("reconnecting: {error}");
                            break;
                        }
                    };
                    match frame {
                        ServerFrame::Snapshot { .. } => {
                            let game = match client.game(id).await {
                                Ok(game) => game,
                                Err(error) => {
                                    eprintln!("reconnecting after refresh: {error}");
                                    break;
                                }
                            };
                            if game.game.is_none()
                                && game.members.iter().any(|m| {
                                    m.is_you && m.seat.is_some() && m.connected && !m.ready
                                })
                            {
                                match client
                                    .command(id, game.version, Command::SetReady { ready: true })
                                    .await
                                {
                                    Ok(_) => {}
                                    Err(ClientError::Server(e))
                                        if e.code == ErrorCode::Conflict => {}
                                    Err(error) => return Err(error.into()),
                                }
                                continue;
                            }
                            if game.game.as_ref().is_some_and(|o| {
                                matches!(o.phase, settlers_client::Phase::Finished { .. })
                            }) {
                                println!(
                                    "{}",
                                    serde_json::to_string(&game.game.as_ref().map(|g| g.phase))?
                                );
                                return Ok(());
                            }
                            if game
                                .game
                                .as_ref()
                                .is_some_and(|o| !o.legal_actions.is_empty())
                                && move_delay > 0
                            {
                                tokio::time::sleep(std::time::Duration::from_millis(move_delay))
                                    .await;
                            }
                            // Commands are chosen from a fresh view after pacing, never queued snapshots.
                            let game = match client.game(id).await {
                                Ok(game) => game,
                                Err(error) => {
                                    eprintln!("reconnecting after refresh: {error}");
                                    break;
                                }
                            };
                            if let Some(mut command) = policy.choose(&game) {
                                if let Command::OfferTrade { expires_in_ms, .. } = &mut command {
                                    *expires_in_ms = Some(trade_wait.clamp(1_000, 300_000) as u32);
                                }
                                let offering = matches!(command, Command::OfferTrade { .. });
                                let request = Client::prepare(game.version, command.clone());
                                let mut result = client.submit(id, &request).await;
                                // Storage pauses and ambiguous transport failures retry the same
                                // envelope; a fresh decision would lose idempotency after a commit.
                                for attempt in 0..20 {
                                    if !matches!(
                                        &result,
                                        Err(ClientError::Http(_))
                                            | Err(ClientError::Server(settlers_client::ApiError {
                                                code: ErrorCode::Backpressure
                                                    | ErrorCode::StorageUnavailable,
                                                ..
                                            }))
                                    ) {
                                        break;
                                    }
                                    tokio::time::sleep(std::time::Duration::from_millis(
                                        (100 * (attempt + 1)).min(1000),
                                    ))
                                    .await;
                                    result = client.submit(id, &request).await;
                                }
                                if result.is_ok() {
                                    policy.committed(&command, &game);
                                }
                                if let Err(error) = result {
                                    eprintln!("{error}");
                                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                    let _ =
                                        socket.send(&settlers_client::ClientFrame::Resync).await;
                                } else if offering && trade_wait > 0 {
                                    // Leave a real offer open long enough for a person to respond.
                                    tokio::time::sleep(std::time::Duration::from_millis(
                                        trade_wait,
                                    ))
                                    .await;
                                    let _ =
                                        socket.send(&settlers_client::ClientFrame::Resync).await;
                                }
                            }
                        }
                        ServerFrame::Events { batch } => policy.on_batch(&batch),
                        _ => {}
                    }
                }
            }
            Err(error) => eprintln!("reconnecting: {error}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        delay = (delay * 2).min(15000);
    }
}
