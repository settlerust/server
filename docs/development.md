# Development and validation

Use Rust 1.96+, Node 24 for TypeScript tests, Python 3.11+, and Terraform 1.5+. AWS SDK crates are behind `--features aws`; the default server runs entirely locally. Dependencies are locked in Cargo/npm/Terraform lock files. Handwritten tests live in each crate's separate test directory, including private-helper tests attached with `#[path]`.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

Transport and AWS SDK fixture tests open temporary localhost sockets. The DynamoDB integration test is ignored by default because it needs an explicit local endpoint; [infra/README.md](../infra/README.md) shows how to run it. CI runs it with DynamoDB Local.

```sh
cd clients/typescript
npm ci
npm run check
npm test
```

```sh
python3 -m venv .venv
.venv/bin/pip install -e clients/python mypy
.venv/bin/mypy --strict clients/python/settlers_client
.venv/bin/python -m unittest discover -s clients/python/tests
```

```sh
terraform -chdir=infra fmt -check
terraform -chdir=infra init -backend=false
terraform -chdir=infra validate
```

## Protocol changes

Edit Rust DTOs first, then regenerate and review:

```sh
cargo run -q -p settlers-protocol --bin schema > docs/protocol.schema.json
python3 scripts/generate_clients.py
```

The generator rejects unsupported schema shapes. TypeScript uses discriminated unions and fixed-length resource tuples. Python uses named `TypedDict` unions and resource lists; JSON Schema/server validation enforces runtime bounds. Rust clients directly share the protocol crate. CI verifies regeneration does not leave a diff.

## Coverage

The suite includes geometry and exhaustive bit packing, setup order and distance/connection rules, production shortages, robber/discard choices, all development cards and timing, piece limits, road trails, awards, win timing, hidden views, deterministic replay, full two/three/four-player policy games, a durable ten-point game, JWT tampering/expiry/refresh, authenticated HTTP and WebSocket behavior, half-open sockets, multiple sockets per seat, reconnect grace, simultaneous trade acceptance, auto-reject/shadow filtering, private messages, stale writes, ambiguous commits, archive lag, pagination, and DynamoDB transactions.

`engine/tests/transition_invariants.rs` checks every advertised action in sampled
two-, three-, and four-player games, in both random modes, against inventory
conservation and the recorded transition. It also exhausts out-of-range seat,
vertex, edge, and robber IDs. `server/tests/local_storage.rs` covers transaction
rollback, outbox capacity and recovery, full-record archive retry equality,
corrupt archive files, and path validation. Research visibility tests cover retired
and replaced agents: losing control of a seat never grants research spectator
access, including when projecting replayed records.

`server/tests/s3_history.rs` exercises the AWS SDK against a local HTTP fixture:
sequence-addressed gzip batches, concurrent conditional updates, identical retries,
lost acknowledgments, read-back failures, gaps, maximum cursors, bounded object
decoding, fixed-fixture import/export, and optional Firehose partial failures.
Online reads never list objects or call DynamoDB for historical records.

`server/tests/config/mod.rs` tests configuration parsing without mutating the
process environment. It checks defaults, explicit settings, invalid values,
durable signing-key reuse, Unix key permissions, and preservation of invalid
existing keys and paths.

CI runs the two DynamoDB integration targets explicitly against a disposable
local instance; ownership fencing is included, rather than left ignored:

```sh
SETTLERS_TEST_DYNAMODB_URL=http://127.0.0.1:18000 \
  cargo test -p server --features aws --test aws --test ownership_aws --locked -- --ignored
```

These checks exercise behavior and sampled invariants; they are not a measured
line or branch coverage percentage. To measure Rust coverage on a machine with
`cargo-llvm-cov` and the `llvm-tools-preview` toolchain component:

```sh
cargo llvm-cov --workspace --all-features --locked --html
```

The ordinary coverage command excludes ignored local-service tests. Run those
with their endpoint configured when measuring the AWS paths. Review uncovered
error paths and replay dispatch before setting a coverage threshold.

The secret-scan workflow scans reachable Git history with a pinned Gitleaks
release. Its only private-key exception is the documented public test fixture.
Run the same scan locally after installing that release:

```sh
gitleaks git . --log-opts=--all --redact --no-banner
```

The security workflow also checks all feature dependencies against RustSec with
`cargo-deny`. After installing the workflow's pinned version, run:

```sh
cargo deny --all-features --locked check advisories
```

Failure-injection adapters test both pre-commit failure and a successful commit whose acknowledgment is lost. Archive tests distinguish accepted delivery from durable archive read-back. Replays verify the complete room hash after every record, including metadata and communication settings, not just board occupancy.

## Real processes and cross-language clients

The disposable smoke test launches the actual release server, four independent Rust policy processes, and Python/TypeScript clients. It validates WebSocket frames against JSON Schema, restarts a policy with saved credentials, finishes a ten-point game, reads paginated history, then kills and restarts the server to verify state, JWT continuity, and an idempotent retry. It removes its temporary data and terminates its own processes. Recovery records each stale connection as disconnected; game state and the original command receipt must stay unchanged.

```sh
cargo build --workspace --release
.venv/bin/pip install jsonschema
.venv/bin/python scripts/process_smoke.py
```

To exercise an already running server, use `scripts/network_smoke.py URL` or `node --experimental-strip-types clients/typescript/tests/live.ts URL`. These create test lobbies in that server.

## Performance

```sh
cargo run --release -p settlers-engine --bin bench
cargo run --release -p settlers-policies --bin simulate -- 100
```

```sh
cargo run --release -p settlers-arena --bin tournament -- --seat v2 --seat eta --seat fast --seat eta --seeds 0-15
cargo run --release -p settlers-arena --bin structure -- --seeds 0-7 --depths 1,2,3 --out structure.jsonl
cargo run --release -p settlers-arena --bin train -- --out /tmp/ntuple --wall-seconds 600 --plateau-window 3
```

`tournament` plays paired-seed games between seat policies on every core and
prints per-slot win rates; `structure` samples positions from builder games and
records legal-action widths and the tunable search's afterstates, transitions,
and time by depth. Both are documented in [expectimax.md](expectimax.md#offline-engine-arena).

`bench` reports board/state sizes, a deterministic roll transition, and packed versus byte-array edge reads. The transition fixture is small and must not be treated as a bound on longest-road recomputation or a full expectimax workload. `simulate` includes legal-action/view construction and policies. `arena` measures a different path involving HTTP and durable storage.

Use the vendored [Rust skill](../.agents/skills/rust-best-practices/SKILL.md) and [AGENTS.md](../AGENTS.md). Pure rule changes need focused regression tests; representation changes need equivalence tests and workload benchmarks. Protocol and persistence changes need replay/visibility checks before performance tuning.

## Opening ceremony and remote table play

`roll_for_order` is an optional command for a seated player or the host after all seats are connected and ready. It persists public `OpeningRoll` values and clockwise `Opening.order` in `GameView.opening`, emitting `order_decided`. Only tied highest rollers reroll. It does not create a game or a turn deadline; `start` begins setup using the recorded winner and advances the private RNG past those rolls. Configure and reroll requests are rejected once order is fixed. Without an opening ceremony, `start` uses the seed-selected first seat.

The `remote` binary wraps its selected builder policy with a trade proposer. It offers one surplus card for one missing target resource, at most once per turn, using only its redacted view. `SETTLERS_HUMAN_SEAT` can prioritize a seated human, `SETTLERS_MOVE_DELAY_MS` paces moves (0–5000 ms), and `SETTLERS_TRADE_WAIT_MS` keeps an offer open before continuing (0–30000 ms). Defaults are zero for CLI benchmark runs.

## Timed, staged discards

`Settings.turn_seconds` accepts 0 for unlimited turns, or 1–86400 seconds for
timed play. Untimed games keep `GameView.deadline_ms` null through setup, turns,
presence changes, discards, and restoration. An untimed `DiscardView` uses
`deadline_ms: 0`; clients must keep its controls enabled and omit its countdown.
Trade offers may still have their own explicit expiry independently of the turn.
A seven with pending discards pauses the roller's remaining turn time and starts one shared deadline of `max(turn_seconds * 1000 / 8, 10000)` milliseconds. Presence changes and choices cannot extend it. After all discards resolve, the roller receives the remaining turn time to choose the robber move.

The owning `GameView.discard` contains the round `id`, fixed `required` count, editable `draft`, confirmed `locked` cards, and `deadline_ms`. It is absent for spectators and only contains the recipient's choices. Send `select_discard { discard_id, resources }` to replace the draft, then `confirm_discard { discard_id }` to lock it. Multiple confirmations may fill the quota. These commands tolerate other seats advancing the room version but require the exact active round ID. Receipts remain idempotent. Confirmed cards cannot be removed, and the engine applies one exact discard when the quota is complete. Agents can submit an exact `act/discard` command; it must include previously locked cards.

At expiry, the server preserves locked cards and the latest draft, fills only the shortfall from the largest remaining resource stack (ties by resource ID), and resolves every pending seat. Staging is committed with the room and outbox and survives restart. Public `discarded` counts are emitted only once a seat resolves; resource types are not included in that event. An automatically rolled seven also opens the shared window.

Public observations now include exact `bank` resource counts, alongside the
derived `bank_available` flags. No raw opponent resource hand is added. Inventory
conservation can nevertheless reveal combined opponent holdings and resource types
returned to the bank between snapshots, including after a resolved discard. The
discard event itself still carries only a count; private staged drafts and locked
choices are unchanged. This public-bank rule applies equally to seated players,
ordinary spectators, and research spectators.

## Private theft receipts and lobby discovery

The reducer adds `theft_details { player, victim, resource }` alongside the public `rule/stole` count. The reducer derives this receipt from the committed before/after hands. Live, history, and archive projections disclose it only to the thief and victim; all other seated players and standard spectators receive the public event only. Explicit research spectators receive the receipt, as described below. The theft regression test exercises real commands, every viewer, and replay.

`CreateGame.player_name` optionally names the seated host without changing their JWT subject. Omission uses the JWT display name. The host may leave seats open: authenticated visitors read the public unstarted lobby, then explicitly `join` as a player or spectator. A host calls `roll_for_order` only when all seats are occupied, then `start`. Started games require membership before reads; a new spectator can join using the current version from a conflict response. Game IDs are identifiers, never credentials.

`GameSummary.connected_players` counts currently connected seated members, excluding spectators. Clients can distinguish active tables from saved games awaiting reconnection. It reveals no private game state.

## Research spectator tables and safe setup corrections

Create an agent-only research game with `research: true` and `host_as_player: false`.
Each AI takes its own seat using `join_agent { name }` and its own JWT. Regular
player `join` is rejected for these tables; `join { name, spectator: true }`
registers a viewer. Normal games default to `research: false`, and this flag
cannot be enabled after creation. Historical games are never classified by names.

A research spectator receives `GameView.research_hands`, one `{ seat, hand }` per
player, through snapshots on the normal WebSocket, REST reads, and replay views.
The hand contains held resources, development cards and newly drawn development
cards. Research spectators also receive theft receipts and directed messages.
Seated AI identities continue to receive their own hand and filtered events only;
`research_hands` is absent for every seated identity and for standard spectators.
No projection exposes the seed, RNG, future development deck, or private filters.
`GameSummary.research` identifies these rooms in discovery lists. The native arena
and research Python harness opt in explicitly. A game ID does not provide
authentication or connect a client to a different server automatically.

During `setup_road`, the active player may send
`act { action: { type: "relocate_settlement", vertex } }`. This moves only the
settlement just placed, until its connecting road is confirmed. Second-round
starting resources are returned and recalculated from the new location atomically;
the deadline does not reset. `setup_settlement_moved` describes the correction.

## Atomic trade replacement

An improved offer to the exact same recipient set
replaces an open offer from the same sender when it gives at least as much of
every resource, asks at most as much of every resource, and strictly improves one
quantity. The server cancels dominated offers before applying the open-offer cap.

If a dominated offer was already accepted in the current turn, a later replacement
is rejected with `trade_taken` and the accepting seat. Concurrent acceptance and
replacement therefore have one committed winner. After any resource-changing
transition, unfunded open offers are retracted in the same state/outbox commit.
The new public close reasons are `replaced` and `resources_changed`. Pruning checks
the offer sender's resources; it does not disclose recipient affordability.

## Trade clocks and consideration

`offer_trade` accepts
optional `expires_in_ms` (1,000–300,000 ms, default 30,000). Every new offer carries
`created_at_ms` and `expires_at_ms`; the deadline is capped by the current turn.
Server ticks expire offers independently of turn timeout, with the same atomic
state/outbox commit as other transitions. `trade_closed` reports `expired`, and
acceptance at or after that instant fails. Browser-local checkpoints preserve the
remaining lifetime while a table is closed.

Recipients send `think_trade { offer_id }` to add themselves to the offer's
`thinking` list and emit `trade_thinking`. This is an explicit response, independent
of their hand. An explicit `reject_trade` clears consideration, records the pass,
and closes the offer once all recipients have declined. Private automatic filters
do not appear as explicit responses.

Counter-offers use `offer_trade` with reversed, edited bundles, addressed to the
original sender. Off-turn players can offer only to the active player. Publication
validates the sender's offered cards but never tests a recipient's resources or
automatically passes on their behalf because of affordability. An unsuccessful
acceptance cannot publish a rejection or change the offer. The UI keeps unaffordable
incoming offers visible for consideration, passes, and counters; only acceptance
uses the viewing player's own hand. AI runners include their configured waiting
period in the offer lifetime.

## Completed browser game exports

`BrowserRoom.completed_history(subject)` returns the full durable record sequence
from creation through the first victory event. It requires the seated local host
and a finished game; agent seats and unfinished games cannot use this export.
Later presence changes are excluded so reopening a result preserves its identity.
The envelope includes the game ID, winner, schema version and
`source: "local-unverified"`. Records retain their inputs, public events, engine
version, sequence and state hash for replay and future research.

## Identity name privacy checks

`tests/name_privacy.rs` verifies legacy redaction, current-name history projection,
stale revision rejection, departed members, archival replay and restart.
`tests/managed_auth.rs` also checks that request bodies cannot override signed
names and that an ordinary game read applies a new name revision. These changes
remain in the server identity layer; game rules and public event DTOs are unchanged.


## Disconnect recovery (base-v9)

`base-v9` introduces persisted disconnect clocks and recorded computer decisions.
Versions 1–8 retain their original presence transitions and hashes. New optional
fields are omitted when empty. `tests/disconnection.rs` covers concurrent sockets,
idle aggregation, reconnect and timeout races, host authorization, private projections,
Expert execution, and replay/restart. Engine `tests/retirement.rs` checks conservation,
occupancy, production, setup, turn order, discards and last-player victory.

The frozen Expert checkpoint is bundled in `crates/server/assets/expert.bin.gz`.
Its raw SHA-256 is `f8e61e66e0279b0fe67f5f4d6be0ad3ca0a86617e91ed24fe08a34a997b46f0c`.
The loader verifies the digest and never silently substitutes another evaluator.
Native hosted search uses depth 3, a 4-second limit and 1,200,000 transitions with
decision-local caching. Only a redacted `GameView` crosses into the blocking policy
worker; its selected command is committed with the original observed version.

## Built-in computer seats

The admission update accepts an optional `computers` list of `{ subject, name,
policy }`, with each subject also present in `players`. Only the host admission
grant can configure these seats before play. Existing human seats cannot be
converted through admission. Built-ins are ready and connected in the public
projection, need no JWT or socket, and never acquire disconnect clocks.

| Policy | Search depth | Time limit | Transition budget | Evaluator |
| --- | --- | --- | --- | --- |
| `easy` | 2 | 1 second | 400,000 | Hand heuristic |
| `medium` | 3 | 4 seconds | 1,200,000 | Hand heuristic |
| `hard` | 4 | 12 seconds | 4,000,000 | Hand heuristic |
| `expert` | 3 | 4 seconds | 1,200,000 | Frozen n-tuple tables |

Each decision runs on a blocking worker with
an ordinary redacted seat view, decision-local caching, and version-fenced commit
through `ComputerCommand`. Durable records contain the chosen command; replay
never reruns search. The bundled Expert model requires no external download.
`tests/computers.rs` covers all four policies, startup without connections,
admission fences, rejected external control, durable decisions, and restart.

Hosts may choose any of these strengths through `resolve_disconnect` after the
existing two-minute recovery window. The automatic outcome remains Expert after
30 more seconds. The former player's hand and command privileges remain revoked.

New games accept only the four built-in strengths above. Historical worker
records remain readable so existing games can finish.

## Replay and module maintenance

Replay semantics default to typed `ReplayVersion::BaseV9`. Versions before base-v9
are unsupported. Keep fixed JSONL fixtures under `crates/server/tests/fixtures`;
tests compare persisted hashes and events rather than building their expected data
with the reducer under test. Add future variants at the creation, default reduction, and replay dispatch points. Keep base-v9 transitions in `reducer/base_v9.rs` when changing the default.

Search configuration, leaf selection, reporting, opening candidates, bargaining,
and offline ranking have separate modules. Within search, candidate generation,
action ordering, own-turn planning, and opponent scenarios are separate. Actor
request dispatch and durable commit recovery are separate from its queue loop.
Tests belong under the owning crate's `tests/`; internal tests use an explicit
`#[path]` module when private helpers need coverage.
