# API and WebSocket protocol

Protocol version: **1**. JSON Schema is generated from Rust DTOs in `crates/protocol`; [protocol.schema.json](protocol.schema.json) defines every action, command, observation, event, and error. JSON field names and enum tags use snake case. Resource arrays always use `[brick, lumber, wool, grain, ore]`. Seats are `0..players-1`; geometry IDs come from `/v1/topology`.

## Authentication

Every game, metadata, history, and socket request requires a valid JWT. The bootstrap exception is `POST /v1/auth/guest` in local mode. Browser CORS preflight requests carry no game data and are also unauthenticated.

```http
POST /v1/auth/guest
Content-Type: application/json

{"name":"policy-a"}
```

Response: `{ "token": "JWT", "subject": "stable-UUID", "expires_at": 1234567890 }`. Save the token. Subsequent REST requests use `Authorization: Bearer JWT`. Tokens expire after seven days; `POST /v1/auth/refresh` with a still-valid JWT returns a renewed token for the same subject. A new guest request creates a new identity, even with the same name.

JWT verification fixes HS256, issuer `server`, audience `settlers-v1`, and validates signature, expiry, not-before, and issued-at. Local signing keys persist across process restarts. Set a stable `SETTLERS_JWT_SECRET` of at least 32 bytes when managing the key externally. `SETTLERS_MODE=production` disables the guest provider. Additional providers implement `IdentityProvider` and register through `Auth::with_provider`.

## Routes

| Method | Route | Request / response |
|---|---|---|
| POST | `/v1/auth/guest` | `GuestRequest` → `TokenResponse`; local bootstrap |
| POST | `/v1/auth/refresh` | JWT → `TokenResponse` |
| GET | `/v1/health` | authenticated status and protocol version |
| GET | `/v1/topology` | coordinates, hex corners, edge endpoints, adjacency, port endpoints |
| GET | `/v1/schema` | complete JSON Schema |
| GET | `/v1/games?after=ID&limit=50` | `GameList`; maximum 100 lobbies per page |
| POST | `/v1/games` | `CreateGame` → `GameView` |
| GET | `/v1/games/{id}` | `GameView`, redacted for the caller |
| GET | `/v1/games/{id}/state` | alias for the complete caller view |
| GET | `/v1/games/{id}/players` | `PlayersView`: presence, public players, caller's private hand |
| POST | `/v1/games/{id}/commands` | `CommandRequest` → `Receipt` |
| GET | `/v1/games/{id}/events?after=0&limit=50` | `EventPage`; exclusive sequence cursor, maximum 100 batches |
| GET | `/v1/games/{id}/ws` | authenticated WebSocket upgrade |

A lobby can be inspected before joining. Once started, its state and history require player or spectator membership. The lobby list remains visible to authenticated users and includes `connected_players`, the number of currently connected seated members. Players join before start; spectators can join later. Only the host starts a full lobby.

```json
{"name":"Experiment 1","settings":{"players":4,"points_to_win":10,"discard_limit":7,"turn_seconds":120},"host_as_player":false,"research":true}
```

The creator is the host and joins as a spectator when `host_as_player` is false. Otherwise it occupies seat 0. Optional `player_name` sets the seated host’s table name while preserving the JWT subject; omitted names use the JWT display name. Starting player selection is seeded and uniform; cyclic seat order defines subsequent turns.

## Commands and idempotency

REST and WebSocket use the same envelope:

```json
{"request_id":"a6e9f54d-e6b7-4d17-8f95-fd260d2538cf","expected_version":8,"command":{"type":"act","action":{"type":"roll"}}}
```

`request_id` is a new UUID per logical command. Persist and resend the **same entire envelope** after an ambiguous transport failure. The server commits before acknowledging. The last 64 successful requests per room retain their exact receipts. Reusing an ID for different contents fails. Older retries fail the version check instead of executing again.

Every committed input increments `version`, including presence, communication, and filters. A `conflict` response includes `current_version`; fetch a new snapshot and reconsider the action before generating a new request. A simultaneous trade acceptance is the deliberate exception: stale expected versions are accepted for `accept_trade`, while future versions are rejected. An offer's single-use ID and stored outcome protect this exception.

| Command type | Fields | Behavior |
|---|---|---|
| `join` | `name`, `spectator` (default false) | acquire a vacant seat in a standard game or observe |
| `join_agent` | `name` | acquire an AI seat; required for research tables |
| `start` | none | host starts a lobby with all seats occupied |
| `configure` | `settings` | host updates settings before start; occupied seats must fit |
| `act` | `action` | apply a base-game action |
| `offer_trade` | `to`, `give`, `receive`, optional `message` | offer resources to eligible recipients |
| `accept_trade` | `offer_id` | execute the first valid acceptance |
| `reject_trade` | `offer_id` | reject this offer |
| `cancel_trade` | `offer_id` | offerer cancels an open offer |
| `communicate` | optional `to`, `message` | structured public or directed communication |
| `set_filter` | `player`, `filter` | change private delivery/trade preferences |

Game actions are `build_settlement {vertex}`, `build_road {edge}`, `build_city {vertex}`, `roll`, `discard {resources}`, `move_robber {hex,victim}`, `buy_development`, `play_knight`, `play_road_building`, `finish_road_building` (decline the second free road), `play_monopoly {resource}`, `play_year_of_plenty {resources}`, `bank_trade {give,receive}`, and `end_turn`. The engine's `trade` action is reserved for server-resolved offers; clients cannot submit it directly. Clients never supply dice, draws, steals, seeds, or authoritative state.

`legal_actions` supplies finite useful choices: all legal placements, robber choices, card choices, unit bank trades, and one deterministic discard. It is **not exhaustive for arbitrary quantities or discards**. Any legal resource bundle can be submitted; the engine validates it independently.

## Trades and conversation

A `Communication` has `intent` (`chat`, `build_road`, `build_settlement`, `build_city`, `buy_development`, `request_trade`, or `strategy`), optional `resources`, and optional `text` (maximum 1024 UTF-8 bytes). Policies can use the structured fields without interpreting text. These fields are claims by players, not server endorsements.

A turn's active player must participate in every trade. Active players may offer to several opponents. An off-turn player may offer only to the active player. Trades exchange nonempty, disjoint resource types; cards are not reserved when offered. Inventory is rechecked atomically at acceptance. Offers expire at turn end or game end.

Each game has a bounded FIFO command queue. The first valid acceptance enqueued by authenticated HTTP or WebSocket handling wins. The losing request receives HTTP 409 / WebSocket error:

```json
{"code":"trade_taken","message":"trade declined because another player accepted it first","winner":1}
```

All participants receive a typed `trade_closed` event with the winner. Network send times on separate machines do not establish server receive order. Invalid/unfunded acceptances do not consume an offer.

`Filter` has four booleans: `mute_chat`, `mute_trade_messages`, `auto_reject_trades`, `shadow_mute`. Filters are private to their owner. Auto-rejection silently makes a target's offers unavailable to the filtering seat. Shadow muting suppresses chat and offer commentary while returning normal successful send receipts to the sender. Public game effects always remain visible. Directed chat is visible only to its sender and recipients; standard spectator views exclude directed chat. Research spectators receive it. History applies the same current filters as live delivery.

Limits: 16 communication/offer commands per seat per minute, four open offers per seat, 64 retained offers, 36 total participants, 16 KiB request/frame size, and 256 queued commands per game. Limit violations return typed errors.

## Private theft receipts

The public `rule/stole` event contains `player`, `victim`, and `count`. The thief and victim additionally receive `theft_details { player, victim, resource }` in the same batch. Other seated players and standard spectators never receive that private event, whether via WebSocket, REST history, or reconnect recovery. Clients should render the public count when no authorized receipt is present. Historical `base-v1`/`base-v2` records contain only the public event; new records use `base-v4` without changing wire protocol version 1.

## WebSocket

Native clients send the JWT in `Authorization`. Browser clients use:

```ts
new WebSocket('ws://127.0.0.1:8080/v1/games/GAME_ID/ws',
  ['settlers-v1', `bearer.${savedJwt}`]);
```

The server selects only `settlers-v1`; JWTs are not placed in URLs. Use TLS/WSS when transporting credentials outside a trusted local environment. Configure browser REST origins through `SETTLERS_ALLOWED_ORIGINS=http://localhost:5173` (comma-separated).

Client frames:

```json
{"type":"command","request":{"request_id":"a6e9f54d-e6b7-4d17-8f95-fd260d2538cf","expected_version":8,"command":{"type":"act","action":{"type":"roll"}}}}
{"type":"ping","nonce":"policy-liveness-1"}
{"type":"resync"}
```

Server frames are `snapshot {protocol,game}`, `events {batch}`, `ack {receipt}`, `error {request_id,error}`, `pong {nonce}`, and `resync_required`. A snapshot arrives on connect. Committed records produce filtered event batches and updated private snapshots. A snapshot may already include several queued events; **keep a separate event-history cursor and snapshot version**. Events with an earlier sequence than the newest snapshot can still be new to the event history.

Transport ping frames are sent every five seconds. A socket with no incoming frame for twenty seconds is disconnected. Presence becomes disconnected only after the last socket for that identity closes. JWT validity is checked throughout the connection. A slow consumer is disconnected after a five-second send timeout; broadcast lag produces `resync_required` and a fresh snapshot. Recover missing public batches through the REST history cursor.

On disconnect, the active player (or a pending discarder) receives at least one full turn allotment from disconnect detection. Reconnects retain identity and do not repeatedly reset that grace. Persistent deadlines survive restart. See [timeout rules](rules.md#timeouts).

## Errors and pagination

Errors use `ApiError`: `code`, `message`, optional `rule`, optional `current_version`, and optional `winner`. Code determines policy behavior; human text is explanatory. Authentication is 401; authorization 403; unknown games 404; conflicts/trade races 409; rule/shape errors 400; temporary storage/queue/archive pressure 503.

`EventPage.next_after` is the cursor for the next page. Empty redacted batches still advance the cursor. `current_version` bounds the page's snapshot. `archive_pending` means a contiguous segment is temporarily unavailable; retry the same cursor. Public REST history never returns archive inputs, seeds, RNG state, or hidden hands.

## Agent-only research spectator mode

`CreateGame.research` defaults to false. Setting it to true requires an observer
host (`host_as_player: false`) and `join_agent` admission for every player. The
mode is fixed at creation, and `GameSummary.research` advertises it in the lobby.
Only research spectators receive `GameView.research_hands`, containing each
seat’s current resource and development hand, plus directed messages and theft
receipts in event history. The standard WebSocket delivers this same projection
in every snapshot. Every seated AI still receives its own private view only.
Future randomness, unrevealed deck order, and private filters are never exposed.

See [development.md](development.md#research-spectator-tables-and-safe-setup-corrections)
for setup relocation and [atomic trade replacement](development.md#atomic-trade-replacement)
for improved offers, resource revalidation, and acceptance races.
