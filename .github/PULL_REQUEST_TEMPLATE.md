<!-- Ordinary changes: target dev. Start a feature/<slug>, fix/<slug>, improvement/<slug>, chore/<slug>, or similarly named branch from dev; use the same branch name across coordinated repositories. Git branch names do not start with a slash.
For dev -> main releases, use .github/PULL_REQUEST_TEMPLATE/release.md instead.
Replace guidance with actual results. Supply full PR URLs and a brief reason, or explicitly write None. Update links in both directions; do not leave placeholders. -->

## Change

<!-- Explain the authoritative Rust engine, protocol, persistence, client, or native/WASM behavior change. Describe compatibility for existing clients and saved games. -->

## Coordination

Upstream PRs must merge first; downstream PRs depend on this change.

- Upstream: None <!-- Required first: replace None with actual PR URLs when applicable. Normally None; identify any actual protocol or infrastructure prerequisite. -->
- Downstream: None <!-- Depends on this change: replace None with actual PR URLs when applicable. service for hosted fleet/container rollout; web for WASM; design/generated clients and research for protocol changes; arena only when deliberately refreshing vendored assets. -->
- Merge/deploy order: <!-- State required order and compatibility between intermediate revisions, or Independent. Shared-stack dispatches can deploy multiple repositories. -->

## Validation

<!-- Report actual commands/results and any checks not run with the reason. Suggested repository checks: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --all-features --locked`, and client checks in docs/development.md. Include regression/protocol generation checks for changed contracts. -->

- Expected dev target: No standalone public site deployment. Hosted rollout uses service at https://dev.settle.rs; browser WASM rollout uses web at https://dev.settlers.dev.
- Verified result: <!-- Record Rust/client results, protocol compatibility, source/container/WASM revisions, and actual consumer deployment runs. Check native and browser consumers when the reducer or shared DTOs change. Give successful run/preview links and outcomes, or Pending; configured URLs are not proof of deployment. -->
