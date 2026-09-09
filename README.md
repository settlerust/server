# Settlers research server

A Rust implementation of the CATAN base game with authenticated lobbies, remote policies, typed communication, deterministic replay, and durable storage.

The pure engine uses a **54-byte board** and a **240-byte copyable state**. Server identity, timers, messaging, storage, and policy code are separate crates. Tests live in their own directories.

## Run inside a browser

The `server` library also builds to WebAssembly:

```sh
rustup target add wasm32-unknown-unknown
cargo build -p server --lib --release --locked --target wasm32-unknown-unknown
```

The browser entry point is `browser::BrowserRoom`. It uses the same room reducer,
protocol projections, engine, UUID validation, and idempotent receipts as the
native service. Native HTTP/WebSocket, JWT signing, SQLite, and AWS dependencies
are excluded on the WASM target. The browser host provides secure random seed
bytes, a clock, serialized message handling, and an atomic IndexedDB checkpoint
before acknowledgments or live notifications. Checkpoints contain private room
state and remain inside the local server worker. Participant responses use the
normal redacted projections.

Browser games are local and unverified; their identities and checkpoints cannot
establish a trusted multiplayer result. The native server remains responsible for
networked multiplayer and externally validated results. Use `cargo test -p server
--test browser --locked` for the browser adapter regressions, including a complete
ten-point game.

## Run locally

Requires Rust 1.96 or newer. From this repository:

```sh
cargo run --release -p server --bin server
```

In another terminal, run four policies through the authenticated HTTP API:

```sh
cargo run --release -p settlers-policies --bin arena -- http://127.0.0.1:8080
```

The arena prints its game ID and the final public state. A faster simulation that bypasses the server is:

```sh
cargo run --release -p settlers-policies --bin simulate -- 100
cargo run --release -p settlers-engine --bin bench
```

Local mode stores current state and pending archive events in `data/state.sqlite3`, archived events in `data/archive/game_id=UUID/`, and the persistent JWT signing key in `data/auth.key`. Keep the data directory and each participant's token across restarts.

## Build a client or policy

- [API routes and WebSocket protocol](docs/api.md)
- [Runnable client examples: Rust, TypeScript, Python](docs/clients.md)
- [Game rules, settings, and timeout behavior](docs/rules.md)
- [Engine representation and architecture](docs/architecture.md)
- [Simulation, chance branches, and game-theory research](docs/research.md)
- [Durability, replay, and archive queries](docs/persistence.md)
- [AWS deployment configuration](infra/README.md)
- [Development and tests](docs/development.md)
- [Recorded validation results and measurements](docs/validation.md)
- [Generated protocol schema](docs/protocol.schema.json)

For LAN policies, set `SETTLERS_BIND=0.0.0.0:8080` and connect clients to the server's LAN address. The server issues each guest a distinct identity; sharing display names does not share seats. A human can host as an observer with `host_as_player: false` and leave all seats available for policies.

## Scope

The base game supports 3–4 players. Two seats use an explicit research variant with the same board and base rules. Default victory target is 10, discard threshold 7, and turn time 120 seconds. The timeout fallback returns the robber to the desert without stealing, as requested for unattended games.

Local guest authentication is implemented. `IdentityProvider` is the extension point for GitHub, Google, and email/password authentication. Production mode disables guest issuance; those provider integrations are future work. AWS infrastructure is supplied as Terraform; applying it and running the server in a cloud account are deployment steps.

## Hosted external authentication

An independently operated lobby can run this server in verification-only mode.
Set `SETTLERS_JWT_PUBLIC_KEY` to an RSA public PEM, `SETTLERS_JWT_ISSUER` to the
trusted issuer, and `SETTLERS_SERVER_ID` to this process's ownership UUID. A shared
process hosts multiple games. No private
signing key or OAuth secret belongs on that server. Guest issuance and refresh are
disabled. The public `/healthz` endpoint reports readiness without game data.

External JWTs use RS256 and audience `settlers-v1`, with `iss`, `sub`, `name`, `iat`,
`nbf`, `exp`, `server_id`, `game_id`, and `role` (`host`, `player`, or `spectator`). Their maximum
lifetime is one hour. Tokens must match this server and the requested game; both HTTP and WebSocket
commands enforce their role. External hosts cannot change game settings after
admission. The lobby issuer is responsible for admitting players and selecting the
host before signing grants. POST `/v1/games` creates the assigned UUID once and
recovers retries from the same host.

When using AWS storage, the process restores only games in its DynamoDB ownership
manifest. State writes require both the expected version and matching owner.
An orchestrator assigns and withdraws ownership transactionally, as described in
[persistence](docs/persistence.md). Your orchestrator must guarantee one live process
per ownership UUID and confirm the previous process stopped before replacing it.
`SETTLERS_MAX_LOADED_GAMES` bounds resident actors. On Fargate, setting
`SETTLERS_TASK_VCPU` and `SETTLERS_TASK_MEMORY_MIB` enables 30-second CPU, memory,
command-latency and timer-lag reports in DynamoDB for capacity-aware placement.
Legacy single-game deployments may use `SETTLERS_GAME_ID` instead of
`SETTLERS_SERVER_ID`; their tokens omit `server_id`. The server handles
SIGTERM gracefully. `Dockerfile` builds an AWS-capable image; it contains only the
public server source and dependencies, with no private orchestration code.


## Research Firehose routing

AWS storage requires `SETTLERS_FIREHOSE_STREAM`, the raw replay Firehose destination.
Optional `SETTLERS_RESEARCH_FIREHOSE_STREAM` sends every durable record to a separate
research stream before raw archival. It also works with SQLite/local archives when
built with `--features aws`, so a research machine can submit its games using the
AWS SDK credential chain. Either variable accepts one stream name or comma-separated
shards; SHA-256 of the game UUID selects a stable shard. Do not include empty names.

Research records use a stable `event_id` (`game_id:sequence`), `source=game`, timestamp,
actor, and `payload_json` containing the full replay record. Configure Firehose's
Glue schema to convert this JSON envelope to Parquet, partition by `game_id`, and
preserve the raw replay stream's existing `events/game_id=UUID/` gzip JSON path.
The server does not read Parquet for replay. Research delivery failures retain the
outbox, and archive pruning still requires identical raw S3/local read-back.
