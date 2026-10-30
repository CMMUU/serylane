# Serylane publishing and synchronization

## Required for every subsequent release

- Future Serylane implementation/update requests use this complete publication flow by default; explicit analysis-only, local-only or no-release requests are exceptions.

- Treat implementation, publication and synchronization as one delivery: tests → reviewed GitHub main → verified multi-platform release → verified download-center archive → official website copy/docs/deployment → public end-to-end verification.
- Product downloads and application updates use only the download center, per the 2026-10-30 user decision. No GitHub/Gitee fallback, source picker or alternate download instructions. On unavailable or invalid center data, fail with retry. Preserve immutable historical repositories, tags and signed assets.
- Never report a release complete while download-center availability/integrity or website deployment/verification is pending or failed. Do not lower SHA-256/signature checks or rewrite an existing release tag. Legacy Gitee mirror maintenance is separate and is not a product download channel or prerequisite for deploying the official site.
- Verify release/ref identity, signed update manifests, center installer availability and the website's six current-version download routes. Record explicit OS login/reboot acceptance separately.
- macOS static and dynamic tray icons must share the application S artwork; test both normal-font and fallback renderers. Template coloring may adapt to the menu bar, but a separate M/letter glyph is not a brand source.


- Every app/repository/release synchronization includes the official website: review public copy and docs for affected features, run website tests, deploy trusted `main`, then verify the live build and latest-download routes. CI deployment does not automatically rewrite feature descriptions.
- Keep application, repository, Releases display titles, download labels and website branding consistent as Serylane. Never rewrite historical tags, signed binaries, updater identities or real old asset filenames merely to change display branding. Use asset `label` for historical display names.
- macOS downloads must distinguish Intel 芯片 (x64) from Apple 芯片（M 系列）(ARM64). Preserve six platform/architecture routes and never substitute another architecture or an old version when the current package cannot be verified.
- Website deployment runs independently from legacy mirror synchronization. A missing deployment secret or failed live verification means delivery is incomplete; report it explicitly.
- Production: `cd website && pnpm run deploy` uses version upload/deploy only against `serylane-website` on the existing `serylane.cmmuu.com` binding. Do not run `wrangler deploy`/`triggers deploy` to update this site. Do not change DNS or other sites. Commit and push reviewed source first. Keep the explicit `run`: `pnpm deploy` is a different built-in workspace command.
- Never claim an unreleased source version is the latest downloadable app. Latest installer links resolve verified public release metadata at click time; source deployment is separate from publishing application bundles.
- Preserve domain routing off by default and separate saving from enabling. Respect actual backend capability gates: program proxy, enhanced stability and new Codex attachment are currently disabled. Website testing does not authorize starting, stopping, installing or changing the user's running app or proxy.
- Update README, current business contract, official site and project-docs against actual code. Run `node scripts/product-docs.mjs`; refresh generated facts with `--write` after reviewing changes. Keep historical release evidence explicitly historical. TUN plans are not implementation or real-network acceptance.
- Credentials belong only in the authorized credential store/GitHub Actions secrets. Never put tokens, subscription URLs or user config in source, public artifacts, logs or chat.
