# Product and repository map

Settlers combines browser-local Catan games and AI research at `settlers.dev` with hosted multiplayer at `settle.rs`. The Rust engine owns rules and outcomes. Shared React UI belongs to `design`; applications own routing, state, and transport. Follow each repository's more specific `AGENTS.md` and README.

| Repository | Owns / useful entry points | Dependencies and consumers |
| --- | --- | --- |
| `server` | Authoritative Rust reducer (`crates/engine`), wire DTOs (`crates/protocol`), durable server, search and policies. `docs/development.md`. | Supplies hosted `service`, `web` WASM, and `research`; generated protocol/client updates can affect `design` and apps. |
| `service` | Identity, accounts, karma, admission, fleet, archives and analytics (`src/`); shared deployment (`infra/`, `config/`, `scripts/release.ts`). | Exports contracts/helpers used by `design`, `web`, `lobby`, `admin`; deploys `lobby` and `controller` with `design`, `server`, and `research`. |
| `design` | Reusable screens, controls, board, email templates and Storybook (`src/components/`, `src/index.ts`, `src/admin.ts`, `src/research.ts`). | Consumes service helpers and generated server protocol; used by `web`, `lobby`, `controller`, `admin`, and service email rendering. No credentials or network orchestration here. |
| `web` | Browser-local games, worker transport, AI workers, research notebook, replay viewer (`app/`, `lib/`, `scripts/`). | Builds from `design`, `service`, `server` and `research`; deploys its own SST stack. Does not import `lobby` application code. |
| `lobby` | Hosted multiplayer routes, account state, lobbies and TV play (`app/`, `lib/`). | Uses `design` and `service`; stages research content. Its deploy workflow dispatches `service`, which includes server and controller. Does not import `web` application code. |
| `controller` | Phone controls for private TV games, authenticated API proxy and reconnect handling (`app/`, `lib/`). | Uses `design` and hosted service contracts; deployed inside the service stack. |
| `admin` | Operations dashboard routing/state for users, servers and analytics (`app/`, `lib/`, `infra/`). | Uses `design` and `service`; deploys its own SST stack. |
| `research` | Experiment harness, policies, filed studies, evidence and MDX notebook (`harness/`, `policies/`, `approaches/`, `log/`). | Uses server protocol and design publishing components; consumed by `web` and hosted builds. Read its research skills before experiments. |
| `arena` | Self-contained TypeScript tournament CLI, provider adapters, provenance and replay artifacts (`src/`, `vendor/`). | Bundles Rust/WASM and policy assets; no sibling runtime imports. Coordinate protocol/replay changes with `server`/`web` when relevant. |
| `competitor` | Competitor catalogue and independent research viewer (`data/`, `app/`). | No current runtime dependencies on other Settlers repositories. |
| `scripts` | CI deployment setup, IAM/OIDC tooling, runbooks, and this agent harness. | Supports every repository; setup tools default to preview, while `--apply` changes external configuration. |

Some older instructions mention `../model`; no `model` repository is currently present. Confirm ownership against the actual tree before adding a new shared package. `archive/` and `reference/` are not deployment repositories.

## Deployment ownership

| Workflow owner | Sources checked out from the selected branch | Dev endpoints | Production endpoints |
| --- | --- | --- | --- |
| `web/.github/workflows/deploy.yml` | web, design, service, server, research | https://dev.settlers.dev | https://settlers.dev |
| `service/.github/workflows/deploy.yml` | service, lobby, controller, design, server, research | https://dev.settle.rs ; https://play.dev.settle.rs | https://settle.rs ; https://play.settle.rs |
| `admin/.github/workflows/deploy.yml` | admin, design, service | https://admin.dev.settle.rs | https://admin.settle.rs |

`lobby` and `controller` dispatch the matching service workflow; a green dispatcher means queued, not deployed. Service also owns short links at `go.dev.settle.rs` / `go.settle.rs`. Stage selection is `dev` → `dev` and `main` → `production`, including all sibling checkout refs and environment-specific service URLs. These are configured targets; verify the current workflow result and live behavior before claiming a deployment succeeded.

`design`, `server`, and `research` have no independent hosted-site deployment. Their changes require a downstream owner build. A dependency-only merge does not automatically rebuild every consumer: dispatch each affected owner after its dependencies have merged. `arena`, `competitor`, and `scripts` have no configured public application deployment. Do not invent domains for them.

The dependency graph contains a cycle: design consumes service helpers while service deploys apps and email code that consume design. Treat “upstream” as the required change/contract for a particular PR, not an immutable repository ranking. Ship compatible additions before consumers, then remove old contracts in a later release. CI serializes the shared service stack but does not atomically merge repositories or hold deployments until all linked PRs merge.

## Validation entry points

Run from the owning repository; use its locked dependencies and documented prerequisites. Record actual results and distinguish skipped checks. Run the checks appropriate to the change; repository instructions may require the full suite.

| Repository | Checks |
| --- | --- |
| `web` | `npm run check:architecture`, `npm run typecheck`, `npm run lint`, `npm test`, `npm run test:e2e`, `npm run build`; infrastructure: `npm run typecheck:infra`. Build compiles server WASM and validates/stages research. |
| `lobby` | `npm run check:architecture`, `npm run typecheck`, `npm run lint`, `npm test`, `npm run test:e2e`, `npm run build`. |
| `controller` | `npm run typecheck`, `npm run lint`, `npm test`, `npm run build`, `npm run test:e2e`. |
| `admin` | `npm run check:architecture`, `npm run typecheck`, `npm run lint`, `npm test`, `npm run build`, `npm run test:e2e`; infrastructure: `npm run typecheck:infra`. |
| `design` | `npm run typecheck`, `npm run lint`, `npm test`, `npm run build-storybook`, plus affected consuming apps. |
| `service` | `npm run typecheck`, `npx tsc --noEmit -p tsconfig.infra.json`, `npm test`, `npm run test:integration` with DynamoDB Local per README. Its source CI owns these checks; deployment reuses source CI for every checked-out commit. |
| `server` | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --all-features --locked`; client/schema/integration/process checks from `docs/development.md` and `.github/workflows/ci.yml`. |
| `research` | `just check`; policy changes add a small protocol match; publishing uses `just notebook`, `just check`, `just site-check`. Live/billable experiments need their own authorized scope. |
| `arena` | `npm run typecheck`, `npm test`, `npm run verify`. No paid model calls needed for these checks. |
| `competitor` | `npm run validate`, `npm run lint`, `npm run build`. |
| `scripts` | `python3 -m unittest discover -s tests -v`; workspace harness: `python3 sync-agent-harness.py --check`. |

Local file dependencies require sibling directories with their original names. Install the service package before design and consuming apps. Use each README for toolchains, browser prerequisites, local services, and generated SST types. A green PR policy check validates delivery metadata, not application behavior.

Source repositories run their own CI on dev/main pushes and ordinary PRs. Deployment owners wait for successful source CI for their exact selected commits, pin those commits, then package and smoke-test the deployed applications. They do not repeat sibling test suites. Design's browser-backed `npm test` is intentionally agent-run: run it automatically when validating UI changes, with the affected consuming app browser checks. Do not omit it because hosted CI no longer runs it. CI still typechecks, lints, and builds Storybook. See `scripts/docs/ci-and-deployment.md` in the workspace for ownership and rollout details.
