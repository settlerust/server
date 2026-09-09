# Durability and replay

## Atomic checkpoints

Every successful input is a checkpoint. A room stores its current rules state, settings, identities, private filters, offers, deadline, rate-limit counters, and recent idempotency receipts. Its version equals the latest committed input sequence.

The local adapter uses SQLite WAL with `synchronous=FULL`. In one `IMMEDIATE` transaction it checks the prior version, updates current state, and inserts the pending record. DynamoDB uses `TransactWriteItems` for the conditional `STATE` replacement and the unique `EVENT#000…sequence` outbox item. The table is keyed by `(game_id, kind)`.

The server publishes neither a success nor new state until commit succeeds. After ambiguous failure it reads state back and compares version plus state hash. A confirmed matching commit returns success; a conflicting durable version replaces stale actor state and returns a conflict. Client retries use the original UUID and expected version.

## Moving archive horizon

DynamoDB stores current state and a **temporary bounded outbox**, never an unbounded completed history. Local mode has the same distinction. At most 256 pending records per game are allowed; a full horizon returns `backpressure` until archive delivery catches up.

The archive worker:

1. Sends pending records to Firehose (local mode: atomic JSON files with file/directory fsync).
2. Tracks accepted sends to avoid resending them on every poll in the same process.
3. Reads back S3 slices and verifies identical complete records.
4. Deletes only the contiguous confirmed outbox prefix.

A Firehose acceptance is not proof of S3 visibility. The outbox remains until read-back confirmation. Partial batch failures retry safely; a crash after delivery can resend records. Readers deduplicate by `(game_id, sequence)` and reject conflicting records. A server restart forgets the in-memory delivery cache and safely retries pending records. Accepted records still missing from S3 are re-sent after five minutes, allowing recovery even if downstream delivery exhausted its own retries.

S3 objects are gzip newline-delimited JSON under `events/game_id=UUID/`. Every line has canonical typed `input` and `public` fields plus `input_json` and `public_json` string columns for convenient Athena JSON functions. These duplicated query columns are only added at the archive boundary. Records include `schema`, `engine_version`, sequence, timestamp, and a SHA-256 hash of the resulting complete room.

Pending records bridge Firehose buffering delay in REST history. Reading the outbox before S3 avoids a prune race. Public history uses the caller projection and never exposes the private `input` field. Batches with no visible events still advance pagination.

## Shared-process ownership

With `SETTLERS_SERVER_ID`, DynamoDB `STATE` items contain an `owner_id` string.
An ownership manifest item has `game_id = OWNER#<server UUID>` and
`kind = <game UUID>`. The orchestrator must atomically update the route, state
owner, and manifest when assigning a game. A new assignment may create an owner-only
`STATE` placeholder; the host's idempotent create request initializes its data.
The process queries its own manifest rather than scanning all saved games.

Every state/outbox transaction also checks `owner_id`, so withdrawing ownership
fences subsequent commits even if an old actor still has queued work. The process
checks its manifest every 15 seconds and unloads released actors. On-demand creation
restores existing owned state for the original host, including when a saved game
is assigned to a process that is already serving other games. A resident-game cap
rejects excess creation with backpressure. Ownership does not replace the
orchestrator's requirement to stop the old process before restarting the same
server UUID.

Optional Fargate telemetry uses `game_id = SERVER#<server UUID>`, `kind = METRICS`,
and a JSON `data` attribute. Reports include Unix-second `timestamp`, normalized
`cpuPercent`, working-set `memoryPercent`, `loadedGames`, `commands`,
`commandP95Ms` (a bounded histogram upper estimate), and `tickLagMs`. CPU is
normalized to `SETTLERS_TASK_VCPU`; memory to `SETTLERS_TASK_MEMORY_MIB`.
The telemetry worker needs DynamoDB `PutItem` on this table and reads only
its own ECS metadata endpoint. It does not make placement decisions.

## Crash recovery

At startup the server loads current snapshots, resumes archive delivery, marks old sockets disconnected, and resumes persisted deadlines. A saved JWT and signing key preserve the same seat. Lost public broadcasts are recovered by REST pagination plus a fresh private snapshot. There is no need to replay all S3 history merely to restart the server. Archive outages do not block loading durable snapshots. If a full outbox or a failed write prevents committing disconnected presence, the last committed presence remains visible until actor retries succeed; no uncommitted presence is published. One game's archive failure does not prevent delivery for other games.

Full reconstruction from the initial seed and recorded inputs is a separate verification/research path:

```sh
cat data/archive/game_id=GAME_UUID/*.json | cargo run -q -p server --bin replay
```

For S3, download the permitted game's gzip objects, decompress their lines, and feed the same CLI. The CLI sorts, deduplicates, enforces contiguous sequence numbers, applies creation/commands/presence/timeouts, and verifies every resulting hash. Incompatible schema/engine versions fail explicitly. Preserve versioned replay code when changing rule behavior or deterministic randomness.

## Athena

Terraform configures a Glue external JSON table and Athena workgroup. Partition projection uses an injected `game_id`, so queries must constrain it explicitly. No crawler or `MSCK REPAIR` is needed.

```sql
SELECT DISTINCT sequence, timestamp_ms,
  json_extract_scalar(input_json, '$.request.command.action.type') AS action,
  state_hash
FROM events
WHERE game_id = 'GAME_UUID'
ORDER BY sequence;
```

```sql
WITH deduplicated AS (
  SELECT DISTINCT sequence, public_json
  FROM events WHERE game_id = 'GAME_UUID'
)
SELECT sequence, json_extract_scalar(event, '$.event.type') AS rule_event,
       json_extract_scalar(event, '$.event.player') AS player
FROM deduplicated
CROSS JOIN UNNEST(CAST(json_parse(public_json) AS ARRAY(JSON))) AS t(event)
ORDER BY sequence;
```

`input_json` includes seed and private participant actions. Give research archive/Glue/Athena access only to research roles. Browser/player JWTs grant no AWS access. The server runtime IAM policy permits current-state/outbox operations, Firehose sends, and archive reads; it does not grant Athena execution or S3 deletion.

Trade responses use archive transition version `base-v6`. Explicit passes persist
in each offer, and the last rejection closes it in the same transaction. Replay
continues to use the historical rejection behavior for `base-v1` through `base-v5`;
the empty response list is omitted from serialization to preserve historical hashes.

Archive transition version `base-v7` locks lobby readiness once the opening order
is decided. Earlier records retain their original readiness and disconnect
behavior during replay. No wire schema changes are required.
