# Validation record — 2026-10-01

The incremental open-source preparation passed `cargo fmt --all --check`, workspace
Clippy with all targets/features and warnings denied, and the full all-feature Rust
suite: **239 passed, 0 failed, 3 ignored**. The ignored targets are two DynamoDB
Local integrations and one benchmark. Docker did not respond to a bounded
availability check, so the container tests were not rerun. The local HTTP AWS SDK
fixture passed conditional index retries, conflicting-record rejection, strongly
consistent paginated reads, gzip import reads, and zero S3 calls for online pages.

TypeScript checking/tests, strict Python mypy/client tests, unchanged JSON Schema
and generated clients, Gitleaks working-tree scan, and cached Cargo advisory checks
passed. No live AWS deployment or price/latency benchmark was performed.

New regressions cover typed base-v9 replay and a permanent 29-record fixture;
corrupt snapshots and browser checkpoints; settings/seat/phase/occupancy indices;
resource, development-card, and piece inventories; legal play and retirement;
long history without whole-archive reads; gaps, file-key mismatches, read caps,
and cursor overflow. Valid base-v9 behavior retains the fixture's fixed final hash.

The full suite passed before the final module/import cleanup and explicit version
dispatch extraction. Replay, readiness, opening, discard, disconnection, theft,
trade, history, recovery, and AWS SDK regressions plus workspace Clippy passed
again afterward. The base-v9 fixture retained its exact final hash.

## Validation record — 2026-09-07

Executed locally on macOS arm64 with Rust 1.96.0, Node 24.19, and Python 3.14. The commands and fixtures are in [development.md](development.md); CI repeats them on Linux.

| Check | Result |
| --- | --- |
| Rust formatting and Clippy, all workspace targets/features, warnings denied | Passed |
| Rust integration tests | 67 passed: 39 engine, 26 server, 2 policy |
| DynamoDB Local | Actual AWS SDK transactional writes, stale-write rejection, reads, and pruning passed |
| Firehose/S3 SDK fixture | Partial Firehose batch failure/retry and gzip S3 slice reading passed against a local HTTP fixture |
| TypeScript | Strict type checking, prepared-request test, and real REST/browser-style WebSocket authentication passed |
| Python | Strict mypy, two client tests, live observer, and JSON Schema validation passed |
| Terraform | Formatting and provider-backed validation passed |
| Seeded simulation | All 100 four-player ten-point games finished; 27,043 actions |
| Independent policy processes | Four Rust processes finished a game in 68 turns; Python validated 355 frames and paginated 234 history batches |
| Abrupt server crash | Saved JWT, exact game view, history, and duplicate request receipt survived kill/restart |

The complete-game test also reconstructs a finished game from its private seed/input archive and verifies the entire room hash after replay. Failure fixtures cover archive outages, delayed visibility, lost commit acknowledgments, retrying presence after restart, and conflicting duplicate archive records. The simultaneous trade test verifies that the first acceptance commits and the second receives `trade_taken` with the winner.

The process test uses a temporary server/data directory and removes them afterward. The DynamoDB Local container used for this run was also removed. No AWS resources were deployed, and no live AWS Firehose-to-S3/Glue/Athena execution was performed. Terraform validation and SDK fixtures do not substitute for that deployment smoke test.

## Measurements

`Board` occupied 54 bytes and `State` 240 bytes. The final release benchmark reported 64.25 ns for its explicit-roll fixture, 1.48 ns for packed road reads, and 0.59 ns for byte-array road reads. These single-machine microbenchmarks ran under ordinary desktop load and are not stable performance guarantees. Packing trades accessor work for lower memory consumption; it does not make every access faster.

The 100-game simulation completed in 0.37 seconds, including policy decisions and observation construction, with no networking or persistence. [Per-seed outcomes](validation/simulation-100.jsonl) are retained. The seed index is encoded little-endian in the first four bytes of a zero-filled 32-byte seed. Seats alternate `fast_builder`, `eta_builder`, `fast_builder`, `eta_builder`. This is a completion/performance check, not evidence of optimal strategy or a statistically controlled policy comparison.
