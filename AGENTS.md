<!-- BEGIN SETTLERS HARNESS -->
## Settlers delivery contract

This is the `server` repository in the Settlers workspace. Read [.agents/rules/product.md](.agents/rules/product.md) for its ownership, dependency and deployment map, and [.agents/rules/delivery.md](.agents/rules/delivery.md) before making or shipping changes.

- **Current phase: stabilization.** Direct, non-force pushes to `dev` are allowed until the user declares the test version stable; dev deployments still run through CI/CD. Do not require a PR or enable protection that blocks this authorized phase.
- For PR-based work, implement on `feature/<slug>`, `fix/<slug>`, `improvement/<slug>`, `chore/<slug>`, or another documented type, based on `origin/dev`. Share the exact branch name across coordinated repositories. No leading `/`.
- Ordinary PRs target `dev`; release PRs are `dev` → `main`. Use this repository's `.github/PULL_REQUEST_TEMPLATE.md` or `.github/PULL_REQUEST_TEMPLATE/release.md`, with explicit Upstream and Downstream PR links (or `None`).
- Use explicit push refspecs. Never push directly to `main` or force-push. After stability is declared, deliver changes to `dev` through topic PRs. Preserve unrelated edits and existing authorization boundaries.
- Dev builds use dependency `dev` branches; production uses `main`. Validate the actual deployment owner's run and endpoint, not merely a dispatch job. Merge compatible upstream changes before consumers.
- For implementation/PR work, read [.agents/skills/settlers-change/SKILL.md](.agents/skills/settlers-change/SKILL.md). For release work, read [.agents/skills/settlers-release/SKILL.md](.agents/skills/settlers-release/SKILL.md).

- For logo, favicon, or app-icon work, read [.agents/skills/settlers-logo/SKILL.md](.agents/skills/settlers-logo/SKILL.md). Register new logo consumers in that skill and its exporter.

These rules and skills are portable copies maintained in `settlerust/scripts` under `agent-harness/`. In the sibling workspace, regenerate with `python3 ../scripts/sync-agent-harness.py --write`; preserve the repository-specific instructions below.
<!-- END SETTLERS HARNESS -->

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
