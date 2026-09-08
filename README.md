# Settlers research server

A Rust implementation of the CATAN base game with authenticated lobbies, remote policies, typed communication, deterministic replay, and durable storage.

The pure engine uses a **54-byte board** and a **240-byte copyable state**. Server identity, timers, messaging, storage, and policy code are separate crates. Tests live in their own directories.

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
trusted issuer, and `SETTLERS_GAME_ID` to this process's assigned UUID. No private
signing key or OAuth secret belongs on that server. Guest issuance and refresh are
disabled. The public `/healthz` endpoint reports readiness without game data.

External JWTs use RS256 and audience `settlers-v1`, with `iss`, `sub`, `name`, `iat`,
`nbf`, `exp`, `game_id`, and `role` (`host`, `player`, or `spectator`). Their maximum
lifetime is one hour. Tokens must match the assigned game; both HTTP and WebSocket
commands enforce their role. External hosts cannot change game settings after
admission. The lobby issuer is responsible for admitting players and selecting the
host before signing grants. POST `/v1/games` creates the assigned UUID once and
recovers retries from the same host.

When using AWS storage, the assigned process restores only that UUID from the
shared DynamoDB table. Your orchestrator must guarantee one live process per game
and confirm the previous process stopped before replacing it. The server handles
SIGTERM gracefully. `Dockerfile` builds an AWS-capable image; it contains only the
public server source and dependencies, with no private orchestration code.
