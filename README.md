# Settlers Catan server

A Rust server for running Catan base-game matches and testing agentic players.
Agents connect through an authenticated HTTP and WebSocket API. The server
enforces the rules, gives each player a redacted view, saves every accepted
command, and supports deterministic replay.

## Run locally

Install Rust 1.96 or newer, then run from this repository:

```sh
cargo run --release --locked -p server --bin server
```

The default server listens on `http://127.0.0.1:8080`, uses SQLite, and issues
local guest credentials. No cloud account or external identity service is needed.
Check that it is running:

```sh
curl http://127.0.0.1:8080/healthz
```

In another terminal, run a complete match with four example agents:

```sh
cargo run --release --locked -p settlers-policies --bin arena -- http://127.0.0.1:8080
```

The runner prints the game ID and final public state. For faster experiments
without networking or persistence, run the engine directly:

```sh
cargo run --release --locked -p settlers-policies --bin simulate -- 100
```

## Connect your own agent

Use the [Rust, TypeScript, or Python client examples](docs/clients.md) to
authenticate, create or join a match, read your seat's observation, and submit
commands. The [API guide](docs/api.md) describes command IDs, version conflicts,
WebSocket events, and history pagination. Keep the same credentials and request
ID when retrying a command whose acknowledgment was lost.

The engine provides legal actions in the seated observation. Opponent hands,
the random seed, and future development cards stay private. An explicitly enabled
agent-only research match can reveal current hands to spectators; seated agents
still receive only their own hand.

Example policies, expectimax search, and paired-seed offline tournaments are
included. See the [policy and simulation guide](docs/research.md) and
[search and tournament guide](docs/expectimax.md) for their configuration.

## Local configuration and saved games

| Variable | Default | Purpose |
| --- | --- | --- |
| `SETTLERS_BIND` | `127.0.0.1:8080` | Listening address; use `0.0.0.0:8080` for LAN clients. |
| `SETTLERS_DATA_DIR` | `data` | SQLite database, archives, and local signing key. |
| `SETTLERS_ALLOWED_ORIGINS` | Empty | Comma-separated origins allowed for browser HTTP clients. |

Local state is stored in `data/state.sqlite3`, replay records in
`data/archive/game_id=UUID/`, and the generated signing key in `data/auth.key`.
Preserve the data directory and participant tokens across restarts. This
directory contains private state and credentials and is excluded from Git.

The base game supports three or four players, with a two-player research variant.
Defaults are ten victory points, a discard threshold of seven cards, and
120-second turns. A turn duration of zero disables turn deadlines. Read the
[rules guide](docs/rules.md) for setup, trades, development cards, and unattended
timeout behavior.

## Code and validation

The rule engine is independent of networking and storage. Its transitions return
a new state without changing the input. The server processes each game's commands
in order and commits state together with its replay record before acknowledging
or broadcasting a change.

- [Architecture and state representation](docs/architecture.md)
- [Development, tests, and process smoke checks](docs/development.md)
- [Persistence, archives, and replay](docs/persistence.md)
- [Optional AWS storage and deployment](infra/README.md)
- [JSON protocol schema](docs/protocol.schema.json)

Run the Rust checks with:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```
