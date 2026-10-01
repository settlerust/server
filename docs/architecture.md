# Architecture and representation

```text
crates/engine       pure rules, packed state, exact chance branches, redacted observations
crates/protocol     typed REST/WS commands, views, events; JSON Schema generator
crates/server       auth → per-game FIFO actor → pure room reducer → durable commit → broadcast
crates/policies     example agents, shared planning helpers, remote runners
crates/expectimax   sampled hidden states, expectimax search, learned evaluation
crates/arena        offline tournaments and self-play training
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

The design uses pure transitions surrounded by stateful I/O adapters. `State`,
`Action`, and `Phase` describe the rule state machine; `apply` computes a successor.
The room reducer adds identities, trade offers, deadlines, and replay receipts,
with the clock passed in as an argument. Neither reducer performs network or
storage operations. Methods on the state structs provide queries and compact
representation helpers rather than an inheritance hierarchy.

`GameService` is a shared handle to the actor registry. Each actor owns a mutable
copy of the last durable room. `Store`, `Archive`, and `IdentityProvider` are
traits for replaceable I/O adapters; tests inject failures through those traits.
The policy/search layer has its own decision state and caches, and reads redacted
observations instead of authoritative rooms.

A command passes through these steps:

1. Transport parses the request and authenticates its identity and grant.
2. The game actor checks duplicate receipts and applies due deadlines.
3. The pure reducer validates the command and builds a room and replay record.
4. Storage conditionally commits the snapshot and outbox record in one transaction.
5. The actor adopts the confirmed state, broadcasts the record, and returns a receipt.
6. Each participant receives a separately projected view or event batch.

If storage reports an ambiguous failure, the actor loads the durable snapshot and
compares its version and hash before deciding whether the command committed.
Archive delivery is separate: an acknowledgment from the destination is not
enough to prune the outbox; read-back must match the entire replay record.

Each game has one bounded Tokio command queue and one owner actor. REST, WebSocket, timers, and presence updates enter this queue. Pure room reducers validate identity, turn/version, trade status, communication filters, and game rules. A successful action yields a new room and a versioned record. Persistence must succeed before the actor publishes state or acknowledges the request.

Multiple games can progress independently; blocking local SQLite/file operations run through `spawn_blocking`. The actor does no synchronous file I/O. A command's arrival order is the order it enters that game's queue after authentication and parsing. Broadcast consumers receive public records and independently redacted private snapshots.

The deployment model is **one authoritative server process per game set**. Remote policies may run on any node. In managed mode, an external orchestrator partitions games through DynamoDB ownership manifests; conditional writes fence both stale versions and withdrawn owners. Each process can host many independent game actors. The orchestrator controls allocation and replacement, and must never run two processes with the same owner UUID. The server does not provide ownership leases or cross-server WebSocket fan-out. See [the ownership contract](persistence.md#shared-process-ownership). The engine can run independently in many simulation processes without storage or network overhead.

## Security and observability boundaries

JWT `sub` owns membership; client-supplied seats cannot impersonate another player. Display names are not identity. Production providers map verified external identities to stable subjects before JWT issuance. The built-in guest provider is local-only.

`GameView` and `EventBatch` are the only participant projections. Full snapshots, random seeds, command replay inputs, private preferences, and hidden hands stay behind server storage access. Views include exact public bank-stack counts and availability flags. Conservation can therefore reveal combined opponent holdings and resources returned to the bank between snapshots. Public production and trades reveal the normal public information; individual development draws and discard types are omitted from broadcasts. The public theft event includes only thief, victim, and count. A separate `theft_details` event reveals the resource only to those two seated identities; the same projection protects live frames and paginated history. Authoritative archives are research data, not a client download API.

Storage warnings use typed/loggable error context without tokens. Queue pressure, archive lag, and lost connections have explicit behaviors. The current history implementation reads a game's archive slices and merges pending outbox records for a consistent cursor; large research histories should use Athena or add a bounded archive index/cache rather than repeatedly scanning whole games through REST.

Explicit agent-only research rooms provide a separate authorized spectator
projection: `research_hands` exposes all current hands, and research spectator
events include directed chat and theft receipts. Seated identities keep their
normal private projection, including when the room is a research game. This
opt-in mode never exposes future randomness, future deck order or private filters.
