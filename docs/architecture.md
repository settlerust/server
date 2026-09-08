# Architecture and representation

```text
crates/engine       pure rules, packed state, exact chance branches, redacted observations
crates/protocol     typed REST/WS commands, views, events; JSON Schema generator
crates/server       auth → per-game FIFO actor → pure room reducer → durable commit → broadcast
crates/policies     fast_builder/ and eta_builder/ with shared planning helpers
clients/            Rust, TypeScript, Python libraries
infra/              DynamoDB, Firehose, S3, Glue, Athena, runtime IAM policy
```

## Compact engine

`Board` is exactly **54 bytes**, with no pointers:

- 54 intersections × 4 bits = 27 bytes. `0` empty, `1..4` settlement owner+1, `5..8` city owner+5.
- 72 edges × 3 bits = 27 bytes. `0` empty, `1..4` owner+1.

Every bit location has an exhaustive packing round-trip test. Road values crossing a byte boundary are extracted using two bytes. The final edge is bounds-safe. This saves 72 bytes against separate one-byte occupancy arrays (126 bytes).

Hex layout uses 19 bytes: the high nibble is the dice total and the low nibble is resource+1 (zero for desert). Port types use 9 bytes. Resource holdings and development inventories use bounded byte arrays. `State` contains no `Vec`, `String`, map, or heap allocation and implements `Copy`. Topology is generated once and shared through `LazyLock`, including all adjacency and coordinate tables.

This is a compact fixed-width representation, not a proof of the global memory/speed optimum. Ignoring legal-position constraints, encoding 9 possible intersection values and 5 possible edge values requires about 339 bits (~43 bytes). Mixed-radix encodings could approach that bound but require division or larger update operations. The included benchmark compares packed edge reads against byte-array reads. Choose subsequent changes from solver workload measurements, particularly cache behavior, rather than isolated accessor speed.

`apply(&S, player, A) -> Result<S, RuleError>` copies state, validates, and computes a successor without allocating in the transition path. Rejection leaves `S` untouched. `apply_recorded` additionally collects public events for the server. `apply_with_chance` replaces one random outcome with an explicitly validated branch. `legal_actions`, observation construction, topology initialization, and timeout event collection do allocate; they are convenience/server APIs rather than claims about the core transition.

The random stream is ChaCha12 with a 256-bit seed and saved word cursor. Rejection sampling avoids modulo bias. Seeds/cursors stay in private authoritative storage. Probability branches are described in [research.md](research.md).

## Server boundaries

Each game has one bounded Tokio command queue and one owner actor. REST, WebSocket, timers, and presence updates enter this queue. Pure room reducers validate identity, turn/version, trade status, communication filters, and game rules. A successful action yields a new room and a versioned record. Persistence must succeed before the actor publishes state or acknowledges the request.

Multiple games can progress independently; blocking local SQLite/file operations run through `spawn_blocking`. The actor does no synchronous file I/O. A command's arrival order is the order it enters that game's queue after authentication and parsing. Broadcast consumers receive public records and independently redacted private snapshots.

The deployment model is **one authoritative server process per game set**. Remote policies may run on any node. DynamoDB conditional writes fence stale state changes, but this release does not provide multi-server game ownership leases or cross-server WebSocket fan-out. Horizontally partition game ownership before running multiple servers against the same table. The engine can run independently in many simulation processes without storage or network overhead.

## Security and observability boundaries

JWT `sub` owns membership; client-supplied seats cannot impersonate another player. Display names are not identity. Production providers map verified external identities to stable subjects before JWT issuance. The built-in guest provider is local-only.

`GameView` and `EventBatch` are the only participant projections. Full snapshots, random seeds, command replay inputs, private preferences, and hidden hands stay behind server storage access. Exact bank-stack counts are private; views disclose only whether each resource is available. Public production and trades reveal the normal public information; individual development draws and discard types are omitted from broadcasts. The public theft event includes only thief, victim, and count. A separate `theft_details` event reveals the resource only to those two seated identities; the same projection protects live frames and paginated history. Authoritative archives are research data, not a client download API.

Storage warnings use typed/loggable error context without tokens. Queue pressure, archive lag, and lost connections have explicit behaviors. The current history implementation reads a game's archive slices and merges pending outbox records for a consistent cursor; large research histories should use Athena or add a bounded archive index/cache rather than repeatedly scanning whole games through REST.

Explicit agent-only research rooms provide a separate authorized spectator
projection: `research_hands` exposes all current hands, and research spectator
events include directed chat and theft receipts. Seated identities keep their
normal private projection, including when the room is a research game. This
opt-in mode never exposes future randomness, future deck order or private filters.
