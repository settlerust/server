---
name: settlers-change
description: Implement and deliver changes across Settlers repositories using shared product ownership, typed branches, checks, and linked pull requests targeting dev. Use for feature, fix, improvement, and maintenance work in this workspace.
---

# Deliver a Settlers change

Read the target repository's `AGENTS.md`, [product map](../../rules/product.md), and [delivery rules](../../rules/delivery.md). Paths here work in either the workspace or a standalone repository's `.agents/skills` directory.

Identify the owning repositories and any contract changes before editing. UI work normally starts in design; authoritative rules/protocol work starts in server; hosted identity and fleet work starts in service. Distinguish build dependencies from runtime prerequisites and note cycles involving design/service.

Check the current delivery phase first. During stabilization, authorized changes may be committed and pushed directly to `dev`; fetch/reconcile it and use the explicit dev refspec in the delivery rules. Do not require a PR until the user declares stability. Production remains a release PR.

For PR-based work, inspect existing edits and create a typed branch from fetched `origin/dev`, using the same name in all participating repositories. Use isolated sibling worktrees if existing work makes that necessary. Do not silently include someone else's changes. Implement provider additions before consumers when automatic dev deployments could see an intermediate combination.

Validate each affected repository and the consuming app using its documented checks. Local coordinated tests can use sibling topic branches; record that state accurately and revalidate the published `dev` combination after prerequisite merges. For docs/harness changes, validate links, generated copies, policy and installer behavior instead of running unrelated gameplay or paid experiments.

For UI changes, automatically run design's browser-backed `npm test` and the affected application's browser checks before delivery. These checks remain the agent's responsibility even though shared UI browser tests are excluded from hosted CI/CD. Source CI owns each repository's other suites; deployments reuse successful checks for exact source commits rather than rerunning them.

Review/stage the requested changes, commit, and push the explicit refspec for the selected phase when delivery is in scope. A request to commit all changes includes the reviewed pending changes, not ignored secrets or generated build output. During stabilization, direct delivery to dev needs no change PR. When using a PR, create it from that repository, filling its own template:

```sh
gh pr create --base dev --head "$branch" --title 'improvement: describe the result' --body-file /tmp/settlers-change-pr.md
```

Write the body file with actual newlines and literal PR URLs. Match the title type and wording to the real change. Use `--draft` while required prerequisites or checks are pending. Create upstream PRs first; update both directions once downstream URLs exist. Never invent PR numbers or leave template instructions as the final description. Attach created PRs to the task when the client supports PR artifacts.

When authorized to merge/deploy dev, follow dependency order, wait for the owner workflow, and validate the live endpoint. If only a dependency changed, dispatch the affected owner workflow under existing deployment authorization. Finish with the actual PR links, checks/results, and any remaining merge or live-validation requirement. Do not claim local edits have been published.
