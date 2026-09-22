---
name: settlers-logo
description: Update the shared Settlers logo, header branding, favicons, and app icons across settle.rs and related apps. Use also when adding the logo to a new project or repository so its consumer inventory stays current.
---

# Update the Settlers logo

The canonical production artwork is `design/assets/board-icon.svg`. The approved
visual reference is `design/assets/logo-concept.png`. Preserve the seven regular
hexagons, terrain palette, ivory settlement, and dark green rounded square unless
the user requests a redesign. The SVG uses a pointy-top radius-80 hex grid,
radius-76 visible tiles, and a uniform 4-unit ivory stroke. Change geometry using
regular-hex coordinates, not independently adjusted vertices.

Paths below are relative to the sibling workspace. In a standalone checkout,
locate or obtain the relevant sibling repositories before exporting their copies.
Use `settlers-change` for repository checks and delivery; this skill does not
authorize production deployment.

## Source and consumer inventory

| Repository | Files and usage |
| --- | --- |
| `design` | `assets/board-icon.svg` is the editable source. `assets/logo-concept.png` records the approved concept, not a runtime asset. `scripts/export-logo.mjs` owns the output inventory. Generated `src/lib/logo-data.ts` embeds the SVG for `src/components/Logo.tsx`, exported from `src/index.ts`. |
| `design` | `src/components/primitives.tsx` uses `Logo` in `Brand`, covering hosted lobby/account/TV screens, browser-local games, phone controls, and agent research headers. `src/components/research/ResearchDocument.tsx` uses it in the notebook's `ResearchShell`. `src/components/admin/AdminShell.tsx` and `AdminAccess.tsx` use it for desktop/mobile navigation and sign-in. `src/components/Logo.stories.tsx` previews the mark and both wordmarks. |
| `design` | `public/brand/icon.svg`, `favicon.ico`, and `apple-icon.png` are the Storybook icons, linked by `.storybook/manager-head.html`. |
| `lobby` | `app/icon.svg`, `app/favicon.ico`, `app/apple-icon.png`; PWA copies `public/images/icon-192.png` and `icon-512.png`, referenced by `app/manifest.ts`. Serves `settle.rs`. Headers consume `design/Brand`. |
| `web` | `app/icon.svg`, `app/favicon.ico`, `app/apple-icon.png`. Serves `settlers.dev`; headers and embedded research consume `design/Brand`. |
| `controller` | `app/icon.svg`, `app/favicon.ico`, `app/apple-icon.png`. Serves `play.settle.rs`; headers consume `design/Brand`. |
| `admin` | `app/icon.svg`, `app/favicon.ico`, `app/apple-icon.png`. Serves `admin.settle.rs`; visible marks come from the design admin components. |
| `competitor` | `app/icon.svg`, `app/favicon.ico`, `app/apple-icon.png`, plus `public/images/logo.svg` used by `app/layout.tsx`. Independent research app; copied assets keep it free of sibling runtime imports. |
| `service`, `server`, `research`, `arena`, `scripts` | No independent app-logo files today. Service deploys lobby/controller; research appears through design/web/lobby. `design/src/emails.tsx` currently has text branding only. Game resource hexagons and replay `BoardIcon` symbols are gameplay UI, not logos. |

## Update and verify

Edit the canonical SVG and regenerate, rather than hand-editing copies. From
`design`, with its locked dependencies and Playwright Chromium installed:

```sh
npm run logo:export
npm run logo:check
```

The exporter needs no running app. It writes the five apps' SVGs, 16/32/48-pixel
ICO frames, opaque 180-pixel Apple icons, lobby 192/512-pixel PNGs, Storybook
assets, competitor header asset, and the embedded design component data. `--check`
reports missing or stale outputs without writing and exits nonzero on drift.
For a partial workspace use `--repo` repeatedly, e.g.
`npm run logo:export -- --repo design --repo lobby`, and report which consumers
still need updating. The default requires all six registered checkouts.

Inspect the exported 512-pixel image, browser-tab sizes, both wordmarks, admin,
and phone layouts. Check equal hex sides, spacing, padding, legibility, and the
browser's actual icon links. Run design's browser-backed tests and the affected
app checks. `scripts/export-web-artwork.mjs` only regenerates the web social
preview; it no longer owns logo exports. Social-preview screenshots containing
branding should be refreshed when their visible content changes.

## Keep this skill current

Whenever the logo is introduced into another repository, project, component,
email, manifest, social card, or asset path, update this inventory in the same
change. Add generated destinations to `design/scripts/export-logo.mjs` (including
its repository allowlist where needed), or document the consumer's actual update
mechanism if it cannot use the exporter. Audit tracked files for `Logo`, `Brand`,
`board-icon`, `logo-data`, `icon.svg`, `favicon`, `apple-icon`, and `icon-` so a
new copy is not missed. Remove retired consumers from both places.

The versioned source of this skill is
`scripts/agent-harness/skills/settlers-logo/SKILL.md`, not its installed `.agents`
copies. After changing it or the harness discovery links, run from the workspace:

```sh
python3 scripts/sync-agent-harness.py --write
python3 scripts/sync-agent-harness.py --check
```

Include the regenerated repository skill and instruction files with the source
change. If a new repository joins the workspace, register it in
`scripts/sync-agent-harness.py` and the product map so it receives this skill too.
