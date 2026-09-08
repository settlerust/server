# Client libraries

All clients send the same typed commands and receive the same redacted views. Keep one credential per participant. Save tokens privately; requesting a new guest token does not reclaim an existing seat. The [API document](api.md) defines error handling, lifecycle, reconnect, and event cursors.

## Python

```sh
python3 -m venv .venv
.venv/bin/pip install -e clients/python
```

```python
from settlers_client import Client

client = Client.guest("http://127.0.0.1:8080", "Research observer")
# Persist client.token; later: Client(base_url, saved_token).
lobby = client.create({"name": "Research match", "host_as_player": False})
game_id = lobby["id"]

# Each separate player does this with its own saved JWT before the host starts:
player = Client.guest("http://127.0.0.1:8080", "Python policy")
view = player.game(game_id)
player.submit(game_id, player.prepare(view["version"],
    {"type": "join", "name": "Python policy", "spectator": False}))
```

After all seats join, the host submits `{"type":"start"}`. An asynchronous player consumes frames as follows:

```python
import asyncio

async def play(client, game_id, policy):
    async for frame in client.stream(game_id):
        if frame["type"] == "events":
            policy.observe(frame["batch"])
        elif frame["type"] == "snapshot":
            view = frame["game"]
            command = policy.choose(view)
            if command is not None:
                request = client.prepare(view["version"], command)
                await asyncio.to_thread(client.submit, game_id, request)
```

REST methods are synchronous; use `asyncio.to_thread` in an async player. `stream` reconnects with the same saved identity. Recover missed history after reconnect when policy memory depends on events. The generated `types.py` contains discriminated `TypedDict` unions; the server validates numeric bounds and legal actions. Renew the token before its seven-day expiry and persist the replacement.

## TypeScript

The source package supports Node 24 and browser bundlers:

```sh
cd clients/typescript
npm ci
npm run check
```

```ts
import { SettlersClient } from './clients/typescript/src/index.ts';
const client = await SettlersClient.guest('http://127.0.0.1:8080', 'TypeScript policy');
const view = await client.game(gameId);
await client.submit(gameId, client.prepare(view.version,
  { type: 'join', name: 'TypeScript policy', spectator: false }));
const stop = client.connect(gameId, frame => {
  if (frame.type === 'snapshot') {
    const game = frame.game.game;
    if (game?.seat === game?.current && game?.phase.type === 'roll') {
      const request = client.prepare(frame.game.version,
        { type: 'act', action: { type: 'roll' } });
      void client.submit(gameId, request).catch(console.error);
    }
  }
});
// stop() closes the socket and cancels reconnect attempts.
```

`connect` retries with bounded exponential delays. It delivers snapshots and event batches through a typed callback. Application code owns policy state, history cursor, pending logical commands, and token renewal. Serialize policy submissions to avoid unintentionally issuing several actions from overlapping callbacks. For browser REST, configure `SETTLERS_ALLOWED_ORIGINS` on the server.

## Rust

Depend on `clients/rust` as package `settlers-client`. It re-exports the shared protocol DTOs.

```rust
use settlers_client::{Client,Command};
# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let (client, token) = Client::guest("http://127.0.0.1:8080", "Rust policy").await?;
// Persist token.token and reuse Client::new(base, saved_token) next run.
let game_id = "GAME_UUID";
let view = client.game(game_id).await?;
let request = Client::prepare(view.version,
    Command::Join { name: "Rust policy".into(), spectator: false });
client.submit(game_id, &request).await?;
let mut socket = client.connect(game_id).await?;
while let Some(frame) = socket.next().await? {
    // Handle snapshots, events, acknowledgments, and typed errors.
}
# Ok(()) }
```

`GameSocket` handles transport pings. Reconnect by calling `connect` with the same client/token and recover history from the last processed event sequence. `Client::prepare` plus `submit` separates logical command creation from retries.

## Existing policies on separate machines

Create a lobby, then run this on each policy node:

```sh
cargo run --release -p settlers-policies --bin remote --   http://SERVER_LAN_IP:8080 GAME_UUID fast ./policy-a.jwt "Policy A"
```

Use `eta` to select the analytical baseline. Each process needs a distinct token file. The file is created once and retained for reconnect/restart. The host starts after all seats are occupied. A human observer's credential remains independent from the policies. The server's `/players` route reports each seat's connection status.

For a local smoke match with automatic lobby setup, `arena` creates a host observer and four guest players, then drives their decisions through REST. `remote` is the live WebSocket-driven policy runner.
