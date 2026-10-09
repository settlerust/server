# Branches, PRs and CI/CD

## Current phase: stabilization

The user is still building a stable test version. Direct, non-force pushes to `dev`
are allowed during this phase and trigger the same development CI/CD as merged PRs.
Do not require a topic PR or configure a remote rule that blocks these pushes.
PR templates and policy checks are available now for work that uses PRs.

This exception ends when the user explicitly declares the test version stable or
asks to enforce the PR process. Then update this versioned harness and synchronize
its copies: future changes use typed branches and PRs targeting `dev`. Do not infer
stability from a single passing build or a date. Production always uses authorized
`dev` → `main` release PRs; direct `main` pushes and force pushes remain prohibited.

For an authorized stabilization delivery, fetch and reconcile `origin/dev`, commit
the requested changes, and push with `git push origin HEAD:refs/heads/dev`. Ensure
`origin/dev` is an ancestor of the pushed commit; resolve divergence without force.
Keep local `dev` synchronized, preserve unrelated work, and publish required
dependencies before applications. Watch the actual deployment owner, not just a
dispatch job. The PR workflow below applies when PRs are used and becomes the
required workflow after stability is declared.

## Work branches and pushing

Use `feature/<lowercase-dashed-name>` for new features, `fix/<name>` for bugs, `improvement/<name>` for general improvements, and `chore`, `docs`, `refactor`, `test`, `perf`, `build`, or `ci` for their usual scopes. There is no leading slash. Reuse the exact branch name across repositories in one coordinated change.

Inspect status, remotes, and worktrees first. Fetch `origin` and branch from `origin/dev`. Never move, discard, stash, or commit unrelated edits merely to get a clean tree. If the current work belongs to another task, use an isolated worktree; coordinated builds need a sibling workspace containing the relevant repositories at their expected names. Worktrees from the same repository cannot check out the same branch twice, so reuse its owning worktree where needed. Research worktrees follow that repository's local placement rules.

Stage explicit paths/hunks and inspect the staged diff before committing. Push with an explicit refspec from the owning repository:

```sh
branch=$(git branch --show-current)
git check-ref-format --branch "$branch"
# Confirm this is the intended typed topic branch, never dev/main.
git push --set-upstream origin "HEAD:refs/heads/$branch"
```

Older clones may have `remote.origin.push=refs/heads/dev:refs/heads/dev`, which overrides the current branch on a bare push. Inspect `git config --get-all remote.origin.push`; remove that exact obsolete value and use `push.default=simple`. Explicit refspecs avoid relying on local defaults. Existing hooks may only protect `main`; direct `dev` pushes are allowed during stabilization or under a later explicit user exception. Never force-push either shared branch.

## Change PRs

All ordinary PRs target `dev`; use the owning repository's `.github/PULL_REQUEST_TEMPLATE.md`. Describe the concrete behavior, relevant checks and dev validation target. Do not claim a URL has new code until the owner deployment succeeds.

Use visible `Upstream:` and `Downstream:` fields. Upstream means this PR requires that change first; downstream means those PRs consume this change. Give actual full PR URLs and a short reason/required order, or `None` when there is no dependency. A merely related PR is not a blocking upstream dependency. Label optional related work separately. For a coordinated release, include counterpart release PRs as well when they exist.

Create required upstream PRs first, then downstream PRs and backfill the reverse links. Keep dependent PRs draft until their prerequisites are merged and validation is meaningful. Source CI tries the same-named topic branch in each dependency repository, then falls back to `dev` when that branch is absent. It pins and records the selected commits. Ordinary PR checks default to `dev`, even when the repository default branch is `main`; shared `main` source checks use `main`. Authentication, API, and missing fallback errors must fail rather than silently choose another revision. Coordinated PR checks do not waive merge order: deployments still consume only `dev` or `main`, never topic branches.

The `Pull request policy` workflow (`policy` job) checks branch/base conventions and explicit dependency fields. It does not fetch linked PRs or verify merge status, contracts, endpoint health, or an atomic release. GitHub must require the check in repository branch protection/rulesets to block merges; tracked files alone cannot configure those settings. Report missing protection rather than claiming it is enabled.

Merged topic branches must be deleted so remaining coordinated PRs fall back to the merged dependency on `dev`. Enable GitHub automatic head-branch deletion and retain `dev` and `main` with a deletion-only ruleset (`scripts/setup-github-branches.py`). This rule must not require PRs or block normal stabilization pushes. A dependency merge or branch deletion does not automatically rerun consumers: rerun their source CI to validate the new resolved combination. Do not reuse a topic name while older coordinated work with that name is still open.

## Development and production

Pushing or merging into `dev` runs the appropriate development deploy. All source dependencies must be on `dev`; runtime URLs must point to the matching development environment. Merging a `dev` → `main` release PR deploys production using `main` everywhere. Direct `dev` pushes are allowed during stabilization; after stability use PRs for both shared branches. Use normal merge commits for release PRs to preserve the long-lived branch ancestry; if repository settings prevent this, resolve that setting/strategy before promotion rather than silently squash-releasing. Bring production-only merge ancestry back through GitHub's update mechanism or a typed synchronization PR targeting `dev` when needed, never by force-resetting branches.

Merge upstream prerequisites first and wait for the relevant checks/deployments before consumers. Because pushes can deploy intermediate states, split incompatible cross-repository changes into compatible provider additions, consumer migration, and later cleanup. If that cannot work, describe and obtain the necessary coordinated deployment hold before executing the release; no such hold is currently implemented by the workflows.

Release PR descriptions contain a short shipping summary, one brief dev-validation sentence, and Upstream/Downstream **release PR** URLs (or `None`). Use `.github/PULL_REQUEST_TEMPLATE/release.md` explicitly; GitHub does not choose it automatically for a `main` base. Avoid long release checklists or copying every commit title.

Use existing task authorization for pushes, PRs, merges and deployments. Preparing changes or a release PR alone does not authorize production promotion. Do not apply IAM/OIDC setup, disable checks, use local deployment to evade CI, or expand paid test scope as a side effect of ordinary development.

## Deployment evidence and dependency-only changes

Consult the product map for the owning workflow. When a dependency-only merge needs a consumer rebuild and deploying that stage is authorized:

```sh
# Development examples. Production uses --ref main after authorized promotion.
gh workflow run deploy.yml --repo settlerust/service --ref dev -f operation=deploy
gh workflow run deploy.yml --repo settlerust/web --ref dev
gh workflow run deploy.yml --repo settlerust/admin --ref dev
```

Dispatch only affected owners. Infrastructure preview for the shared stack is the same service command with `-f operation=diff`. Inspect replacements before applying them. Do not cancel an active SST apply or run a second local apply against its stage.

Publish and verify each repository's source CI workflow before enabling a downstream deployment gate that requires it. Deployment gates resolve current branch commits, reuse each commit's source CI, and pin those revisions for the build. Missing, failed, cancelled, or skipped source validation cannot be bypassed. Duplicate requests for the most recently deployed source combination skip the build and apply; use the deployment workflow's `force=true` input when runtime configuration or secrets changed without a source commit. Force rebuilds still require passing source CI. Shared UI browser tests remain automatic agent-run validation, not deployment steps.

Find the actual run with `gh run list`, inspect its branch/source revisions, and watch it with `gh run watch RUN_ID --repo settlerust/OWNER --exit-status`. The service queue resolves matching branches when it starts; retrying an old job can deploy newer commits. Record actual source SHAs and workflow run URLs, then test the relevant live flow on the intended endpoint. HTTPS 200 alone does not establish authenticated/gameplay correctness. A green lobby/controller dispatch is not service deployment evidence.

For regressions, deliver a fix/revert to `dev` (direct pushes are allowed during stabilization; use a typed PR afterward), validate it, and promote a coordinated `dev` → `main` release. Reverting application code does not undo data migrations or infrastructure replacements.
