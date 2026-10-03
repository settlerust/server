# Durability and replay

## Atomic checkpoints

Every successful input is a checkpoint. A room stores its current rules state, settings, identities, private filters, offers, deadline, rate-limit counters, and recent idempotency receipts. Its version equals the latest committed input sequence.

The local adapter uses SQLite WAL with `synchronous=FULL`. In one `IMMEDIATE` transaction it checks the prior version, updates current state, and inserts the pending record. DynamoDB uses `TransactWriteItems` for the conditional `STATE` replacement and the unique `EVENT#000…sequence` outbox item. The table is keyed by `(game_id, kind)`.

The server publishes neither a success nor new state until commit succeeds. After ambiguous failure it reads state back and compares version plus state hash. A confirmed matching commit returns success; a conflicting durable version replaces stale actor state and returns a conflict. Client retries use the original UUID and expected version.

## Bounded outbox and S3 history

The transactional outbox holds at most 256 pending records per game. A full outbox
returns `backpressure` until archive delivery succeeds. DynamoDB stores the current
`STATE`, temporary `EVENT#…` outbox, and ownership manifests. Permanent replay
history lives in S3; the archive adapter never reads or writes DynamoDB.

AWS history uses gzip JSONL batches under
`history/v1/game_id=UUID/batch-<20-digit first sequence>.jsonl.gz`. Each key covers
100 fixed sequence positions: 1–100, 101–200, and so on. A partial batch is written
immediately, so slow games and final records do not wait for a full batch. Updating
it merges identical records and rejects conflicting duplicates. `If-None-Match`
creates a new object; `If-Match` fences updates against the ETag read before the
merge. Concurrent workers reload and merge on a conditional conflict, with a
bounded retry count. A batch containing all 100 positions needs no further writes.

After delivery, the worker reads only the pending sequence window, compares whole
records, and prunes the confirmed prefix. A failed write, lost acknowledgment,
read-back failure, or exhausted conflict retry leaves the outbox available.
Restart or reassignment restores the latest snapshot and resumes this delivery;
reconstructing old history is unnecessary for recovery or game migration.

REST history reads the outbox first, then a window of at most 100 archive positions.
Local mode opens exact sequence filenames. AWS fetches at most two S3 batches for a
public page; internal 256-position verification fetches at most four. Object sizes,
decoded sizes, and record counts are bounded. Missing positions stop the public
page and set `archive_pending`; readers never skip gaps. Snapshot versions bound
the response, and redacted empty batches still advance its cursor. Authentication
and participant-specific projection apply before records reach a client.

`SETTLERS_FIREHOSE_STREAM` optionally enables the existing raw analytics delivery
under `events/game_id=UUID/`. Configured analytics sends precede S3 history writes;
a failed send leaves the outbox available for retry. Firehose acceptance does not
confirm an S3 history write. A crash may resend analytics events, whose consumers
must deduplicate `(game_id, sequence)`. Raw analytics JSON includes `input_json`
and `public_json` columns for Athena; history batches store canonical replay
records. Both copies contain private data.

Batch writes occur once per affected sequence range in each delivery, rather than
once per record. Partially filled ranges incur additional PUTs as new records
arrive. The versioned bucket expires superseded `history/v1/` object versions after
one day; it retains current history and leaves raw analytics objects unaffected.
S3 lifecycle expiration is asynchronous. Online paths never list S3 objects or read
a whole game's archive. `Archive::read` is reserved for explicit offline export.

Existing base-v9 Firehose exports need a one-time batch import before their archived
portion is available through online history. Configure AWS credentials and
`SETTLERS_ARCHIVE_BUCKET`, then run this once per selected game:

```sh
cargo run -p server --features aws --bin archive-import -- GAME_UUID
```

The command reads the selected export, verifies every replay hash, and writes
history batches without resending Firehose events. Retries are safe. Startup and
request handlers never perform this import. Runtime IAM needs `s3:GetObject` and
`s3:PutObject` on `history/v1/*`; deploy this additive permission before running the
new adapter. Existing DynamoDB `ARCHIVE#…` entries are no longer used and are not
automatically deleted. After verifying their S3 replacement, their explicit
cleanup can reclaim storage without changing `STATE`, outbox, or ownership items.

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

At startup the server validates current snapshots, resumes archive delivery, marks old sockets disconnected, and resumes persisted deadlines. A saved JWT and signing key preserve the same seat. Lost public broadcasts are recovered by REST pagination plus a fresh private snapshot. There is no need to replay all S3 history merely to restart the server. Archive outages do not block loading durable snapshots. If a full outbox or a failed write prevents committing disconnected presence, the last committed presence remains visible until actor retries succeed; no uncommitted presence is published. One game's archive failure does not prevent delivery for other games.

Full reconstruction from the initial seed and recorded inputs is a separate verification/research path:

```sh
cat data/archive/game_id=GAME_UUID/*.json | cargo run -q -p server --bin replay
```

For S3, download the permitted game's gzip objects, decompress their lines, and feed the same CLI. The CLI sorts, deduplicates, enforces contiguous sequence numbers, applies creation/commands/presence/timeouts, and verifies every resulting hash. Incompatible schema/engine versions fail explicitly. `ReplayVersion::CURRENT` defaults to `BaseV9`, serialized as `base-v9`. Earlier and
unknown versions are rejected during decoding. Pre-v9 pending outboxes are not
migrated automatically; deploy against base-v9 data. Participant wire protocol remains
version 1. For a future semantic change, add a typed variant and its explicit reducer
dispatch; capture a new permanent fixture before changing the default. The base-v9
fixture contains fixed inputs, public events, and per-transition hashes and is never
regenerated by its test.

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

`input_json` includes seed and private participant actions. Give research archive/Glue/Athena access only to research roles. Browser/player JWTs grant no AWS access. The server runtime IAM policy permits current-state/outbox operations, optional Firehose sends, and history reads/writes; it does not grant Athena execution or S3 deletion.


## Snapshot validation

`State::validate` checks settings, phase and seat indices, packed occupancy, piece
counts and limits, resource conservation (19 per type), and the 25-card development
inventory across deck, held, new, and used cards. Retired pieces remain on the board;
retirement returns resources and moves held development cards to the used inventory.
`Room::validate` adds unique identities and seats, matching room/engine settings, and
opening/offer indices. Validation runs when storage or browser checkpoints are
restored, when an ambiguous write reloads a snapshot, and after every replay step.
Rule transitions remain pure and do not scan the whole state on each live command.
