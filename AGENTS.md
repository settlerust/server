# Rust engineering contract

Read `.agents/skills/rust-best-practices/SKILL.md` for Rust work. Its Apollo reference chapters are vendored with their MIT license. Project requirements below take precedence.

- Keep the allocation-free game state and pure transitions in `crates/engine`. Never introduce networking, wall clocks, storage, identity, or policy into that crate.
- Use focused modules and external test files under each crate's `tests/`. Group related assertions when they verify one invariant.
- Public DTOs and wire schemas live in `crates/protocol`; preserve player-specific redaction. Never send seeds, RNG state, future deck contents, or private preferences to participants. Seated agents and ordinary spectators never receive other hands. Explicit agent-only research games may expose all current hands to spectator identities through `research_hands`, never to a seated identity.
- Commit state and its outbox atomically before acknowledging or broadcasting. Preserve optimistic concurrency, idempotency, and archive retry behavior.
- Treat JSettlers as a reference for understanding rules and strategy ideas. Do not copy or translate its implementation.
- Add a regression test for rule, persistence, protocol, or concurrency fixes. Benchmark optimizations in release mode before describing them as faster.
- Required checks: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --all-features --locked`, and client checks described in `docs/development.md`.
- Keep Markdown documentation self-contained in this repository. Update protocol schema and generated client types when wire types change.
