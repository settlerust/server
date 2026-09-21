<!-- RELEASE ONLY: head dev, base main. Keep the shipping summary short; do not copy a feature checklist or commit log.
Replace every field with actual PR/run URLs and results, or None/Pending as appropriate. Link coordinated releases in both directions; list required upstream releases before merging. -->

## Shipping

<!-- Summarize the game rules, protocol, persistence, or runtime changes shipping in 1–3 sentences. -->

- Upstream: None <!-- Required release PRs first: replace None with actual PR URLs when applicable. Link actual prerequisites or write None. -->
- Downstream: None <!-- Dependent release PRs: replace None with actual PR URLs when applicable. Link service fleet, web WASM, generated-client/design, and research release PRs that require this server revision. Mark compatibility-sensitive rollout order explicitly. -->
- Dev validation: <!-- Link the successful owning deployment/check run, actual dev URL when applicable, tested source/dependency SHAs, and a brief result. Pending is not verified. -->
- Production rollout: server/main supplies service's native image and web's WASM build. Link required service/web deployments; merging server alone does not update either live runtime.
<!-- Add one short compatibility/rollout note only when needed. Preserve the tested dependency combination or revalidate it if dependency branches moved. -->
