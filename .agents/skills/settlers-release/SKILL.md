---
name: settlers-release
description: Prepare or carry out coordinated Settlers dev-to-main release PRs, upstream/downstream ordering, dependency-only rebuilds, and CI deployment verification. Use for release planning, production promotion, or checking an authorized deployment.
---

# Release Settlers

Read [delivery rules](../../rules/delivery.md), the [deployment map](../../rules/product.md), and each affected repository's release scripts/workflows. `scripts/OPERATIONS.md` from the workspace root (or `../scripts/OPERATIONS.md` from a sibling repository) provides failure/recovery procedures. Do not infer deployment success from an old status report.

1. Fetch the affected repositories and compare `origin/main...origin/dev`. Inspect all changes that would ship, including previously merged work. Record dev deployment source SHAs and actual live-test results; a feature branch or local tree is not the release candidate.
2. Identify required upstream **release** PRs, downstream consumers, stage URLs, and merge order. Check that source branches and runtime endpoints stay in the same environment. `design` changes generally precede consuming apps, but inspect service/design contract cycles and stage compatible changes rather than assuming any global repository order.
3. Prepare `.github/PULL_REQUEST_TEMPLATE/release.md` with a short explanation of what ships, brief dev evidence, and real Upstream/Downstream release PR links or `None`. Create one PR per changed repository with `gh pr create --base main --head dev --title 'Release: concise result' --body-file /tmp/settlers-release-pr.md`. Never create a feature-to-main PR. Backfill reciprocal links; a consumer with no source diff needs a rebuild, not an empty release PR.
4. Under existing production-promotion authorization, merge required upstream release PRs first using normal merge commits, preserving dev/main ancestry. Wait for their applicable checks or deployments. Merge consumers only when the dependency versions they will actually fetch are compatible. If a deployment hold is necessary but unavailable, stop that promotion and explain the concrete blocker.
5. Watch each actual owner run and inspect its recorded revisions. Dispatch affected owners after dependency-only promotion. Confirm production endpoints and the changed flows; do not count lobby/controller dispatch jobs as completed deployments. Record run URLs, revisions, and any rollback/compatibility concern in the release PR or task report.

Release bodies stay short. Put detailed diagnosis and validation logs in linked runs or task notes. Honor the user's scope: preparing release PRs does not itself authorize merging to production, and authorized promotion does not need repeated permission prompts.
