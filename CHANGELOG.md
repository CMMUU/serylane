# Changelog

## 0.7.13 - 2026-09-18

- Classify local proxy, DNS, TLS/certificate, connection, HTTP/account and streaming failures without storing request URLs, credentials, bodies or model output. Expose the latest historical diagnosis and bounded, rate-limited application-log events.
- Probe ChatGPT and OpenAI API independently; distinguish expected reachability, unverified HTTP, failures and unavailable/stale checks. Select for ChatGPT by default, or the explicitly enabled API route target; never treat 401 as a verified model stream.
- Coalesce transport-failure hints into bounded rechecks. Preserve manual choices, multiplier budgets and healthy connections; hold an in-budget current exit during shared failures without penalizing every node. Never replay model requests.
- Preserve the v0.7.12 application-binding changes; fix the relocated Windows test manifest and a parallel macOS test port race without removing release gates. Synchronize website help and six-platform downloads through the existing release pipeline.

## 0.7.12 - 2026-09-17

- Bind Windows MSIX/Store applications by package family and application ID; resolve the current registered executable before launching, preserving proxy mode, arguments and directory policy.
- Add installed-app selection, conservative legacy migration with backup, identity-aware background-process checks, bounded native queries and actionable installation states.
- Add a light/dark Star History chart to the repository README.
- Gate Windows releases with a disposable registered-package v1-to-v2 launch/environment fixture; preserve existing six-target signing, website and mirror verification.

## 0.7.11

- 系统代理启用与后台网络检查分离，非 TUN 模式热切换保留运行核心。
- 真实阶段提示、故障分类、恢复结果与技术详情；严格 API 就绪截止时间。
- 后台检查取消和配置归属保护，保持既有更新及数据兼容性。

## 0.7.10 - 2026-09-15

- Default missing legacy silent-startup preferences to tray-only login, while preserving explicit window-mode choices and manual opening. Test legacy settings, duplicate autostart and explicit single-instance reopening on disposable Windows runners.
- Add independent manual selection for ordinary proxy groups and OpenAI failover, with persistent manual intent and explicit restore-auto. Show actual selected node details on Overview without treating failed/stale reads as a current connection.
- Add per-profile traffic multipliers, opt-in value-based OpenAI ranking, an optional multiplier cap and explicit unknown-cost fallback. Preserve healthy in-budget nodes, prioritize manual choice, warn before unknown/over-budget manual selection and skip large bandwidth downloads in value mode.
- Preserve current signing/upgrade identities, default-off Codex routing, all six download architectures and existing macOS native tray rendering. Physical login/reboot and live model-stream reliability remain separate acceptance checks.
- Allow bounded Gitee uploads enough time for verified large packages on slow cross-region routes; retain low-speed detection, uncertain-write inspection and full checksum/signature requirements.

## 0.7.9 - 2026-09-11

- Render macOS two-row traffic rates as native attributed text with 9.5pt medium tabular digits, smaller arrows/units, fixed columns and a stable status-item width.
- Keep the application S artwork as a separate template icon; preserve existing menus, clicks, system appearance and the bitmap fallback paths.
- Center both rows using the real status-button geometry, clear native text and fixed width when traffic display is disabled, and keep explicit binary rate units in tooltips.
- Handle rounded unit boundaries and very large rates without truncation; cover native 1x/2x rendering, light/dark/selected appearance, shared branding and disable/re-enable regressions.
- Synchronize application notes, website documentation, GitHub and Gitee through the existing verified release pipeline.
- Make Windows installer readiness polling tolerate the journal's atomic replacement gap without relaxing startup, window, proxy or timeout assertions.

## 0.7.8 - 2026-09-11

- Replace the three startup toggles with explicit startup modes while preserving existing preferences and the last successful running/stopped intent.
- Read back login registration separately from OS disable flags; preserve disabled Windows registrations and restore exact values if settings persistence fails.
- Keep transient startup-network failures waiting with capped backoff, and allow Stop to cancel pending/paused restoration or clear a remembered running intent.
- Derive both macOS rate-tray renderers from the same S artwork as the application instead of drawing a separate M glyph.
- Retry only idempotent attachment reads after integrity failures, retain strict hash validation and the manifest barrier, and offer a trusted macOS runner for stalled Gitee upload routes.
- Make GitHub release, Gitee code/releases, official website deployment and public verification mandatory parts of every release.

## 0.7.7 - 2026-09-10

- Unify Serylane product, executable, installer, repository and download names while preserving existing data, signing keys, upgrade identity and byte-identical legacy updater aliases.
- Add optional quiet login startup, preserve the last saved network mode and running intent, and make the main Start button respect that mode. Keep manual launch visible and require permission/conflict checks for restoration.
- Add compact application logs with filtering, duplicate aggregation, a 256 KiB / 2,000-entry cap and configurable retention of 1–90 days (default three days).
- Classify startup connectivity failures, retry transient checks within fixed bounds, preserve explicit subscription health-check settings and retain matching-node stability evidence across policy updates.
- Hide application scrollbars and add continuous smooth wheel scrolling with touchpad, keyboard and reduced-motion compatibility.
- Resolve official website downloads from the latest verified stable release at click time, distinguish Intel and Apple-silicon macOS packages, and deploy/verify the website after repository and release synchronization.
- Gate publication on six-platform tests, signed updater validation and disposable Windows installer/startup checks. Automated tests do not establish uninterrupted model streams, physical reboot acceptance or real-world TUN compatibility.

## 0.7.6 - 2026-09-08

- Make Subscriptions the single entry point for adding subscription URLs. Overview empty states navigate there; Profiles retains local YAML import, configuration management and version rollback.
- Keep the add form compact, with clear primary fields and collapsible advanced options. Make “use after adding” explicit and leave it unchecked by default when an active profile already exists.
- Focus the saved subscription card after successful addition unless the user has navigated or moved focus, and retain the form draft when addition fails. Report optional OpenAI failover generation and observation-write errors separately so they do not misrepresent a saved subscription as a failed import.
- Reuse exact duplicate subscription URLs without fetching or refreshing them; explicitly selecting a duplicate uses its saved revision. Recheck duplicates inside the configuration transaction.
- Preserve the Serylane display brand and S icon, RouteDeck installer/update identities, existing signing key and domestic-first updates with GitHub fallback. Opening the add form does not start the core, change system proxy/TUN or attach Codex.
- Treat saved/selected configuration state, real connectivity, published installers and completed mirror verification as separate outcomes; no uninterrupted-connectivity guarantee is made.

## 0.7.5 - 2026-09-08

- Make the global Start action start the Mihomo core with system proxy enabled, instead of inheriting a previous core-only mode.
- Restore the previous network preference after a failed start without restarting another session. Make failed port validation and process creation retryable, retain concurrent-start guards, and verify observed runtime/proxy state before showing success.
- Prevent settings writes and stale asynchronous status reads from racing the start action.
- Keep the explicit TUN action on the TUN path, with its existing permission and preflight requirements. Program launches and independent local-router operations still do not automatically change system proxy or TUN settings; they are not Mihomo core-start buttons. The local router remains off by default, with separate save, enable and Codex attachment actions.
- Preserve the Serylane display brand and S icon, RouteDeck installer/update identities, existing data, signing key and release history. The default changes when the user invokes global Start; no background mode migration is added.
- Keep real network, TUN and long-session acceptance separate from automated tests; no uninterrupted-connectivity guarantee is made.

## 0.7.4 - 2026-09-08

- Adopt Serylane as the desktop display brand, including the window, tray, interface and safety prompts. Keep the Chinese functional subtitle and the RouteDeck name in explicit installer-compatibility explanations.
- Use the new transparent blue S mark for desktop and sidebar icons, generated from the approved image with the official Tauri icon CLI; retain the previous source artwork for provenance.
- Retain the RouteDeck installer/product identity, routedeck executable and package names, application identifier, data directories, helper identity, Codex provider/lease format, updater endpoints, artifact filenames and existing update verification key.
- Separate display branding from compatibility contracts in automated checks. Do not migrate active application, proxy or Codex configuration as part of the rename.
- Introduce the Serylane website address and preserve references to former names. Publication, website availability and real-network acceptance remain separate checks; no uninterrupted-connectivity guarantee is made.

## 0.7.3 - 2026-09-07

- Apply the approved frosted-glass design across the desktop shell, readable subscription cards, dark themes and narrow layouts, with keyboard and reduced-transparency fallbacks.
- Display provider-reported upload/download, allowance, expiry and sample time. Preserve cached usage on missing headers or refresh failure, including HTTP 304 and unchanged configuration bodies without new revisions or runtime reloads.
- Keep configuration nodes distinct from provider counts, selected profiles distinct from connectivity, and missing quota information distinct from zero or unlimited service.
- Add an opt-in local model route with separate save/start/Codex attachment, HTTP/SSE compatibility, WebSocket passthrough, backup restoration and active-request/conflict protection.
- Evaluate OpenAI stability using recent failures and attributable model-stream evidence, with current-node retention, cooldown and recovery hysteresis. Switching affects new connections and does not resume broken streams or replay model requests.
- Gate subscription and routing regressions in six-platform CI and signed releases; retain domestic-first updates and GitHub fallback for identical verified packages.

## 0.7.2 - 2026-09-05

- Apply the approved Apple-inspired desktop design with a unified sidebar, settings groups, iconography, rounded interaction states and capsule scrollbars across light, dark, purple and compact layouts.
- Prefer Gitee for the newest available stable build in automatic update mode, retaining GitHub as fallback only for byte-identical signed packages. A lagging or unavailable mirror does not hide a newer GitHub release.
- Explain domestic-first updates in Settings while preserving explicitly selected single-source preferences, independent preference saving and installation confirmation.
- Add navigation, theme/accessibility, preference wiring and update-source ordering regression tests to local checks and both CI workflows.

## 0.7.1 - 2026-09-05

- Supersede the unpublished 0.7.0 build without rewriting its mirrored tag; include all dual-channel updater, compatibility, program-management and UI changes below.
- Fix the macOS/Linux updater restart return path so strict cross-platform Clippy checks pass.
- Require formatting and Clippy checks in the six-platform signed release workflow before publication.
- Use HTTP/1.1 for Gitee Git transport, retry only transient read operations and verify every destination ref when a push response is uncertain; never force refs or blindly retry writes.

## 0.7.0 - Unreleased

- Replace the release-page-only update action with signed in-app downloads, progress, cancellation, SHA-256 verification and explicit install/restart confirmation.
- Add independently selectable GitHub/Gitee channels and an automatic mode that chooses the newest available stable release and only fails over between byte-identical signed packages.
- Check updates after startup and every six hours while enabled; keep automatic downloading opt-in and never automatically stop the proxy to install.
- Keep Windows NSIS/MSI installer formats, and provide signed macOS archives and Linux AppImage updates. Existing non-updater versions need a one-time manual upgrade.
- Gate all six application version fields and release tags, verify updater signatures before publication, generate channel-specific manifests and synchronize Gitee manifests only after their packages.
- Include the previously unreleased 0.6.0/0.6.1 subscription, system-proxy compatibility, hidden helper windows, program management and UI fixes below.

## 0.6.1 - Unreleased

- Write one Windows system-proxy endpoint for HTTP and HTTPS CONNECT compatibility, while recognizing older protocol-map leases and preserving safe restoration when another proxy client takes ownership.
- Add a read-only Settings proxy-format check using the HTTP library's actual parser; it does not claim to verify a target application's effective route or WebSocket stream.
- Bypass environment proxies for the local Mihomo controller and avoid automatic OpenAI policy regeneration when a subscription refresh returned unchanged content.
- Add a Windows Program Proxy page with revision-checked add/edit/delete persistence and opt-in launch. Environment mode configures only the new process; Chromium/Electron mode also supplies explicit proxy arguments and disables QUIC. Neither mode forcibly intercepts arbitrary programs or modifies global settings.
- Block launches when the core is stopped, the local port is unavailable, the executable is missing or an existing instance is detected. Never kill an existing application; deleting a list entry does not uninstall it or close its process.
- Keep console subprocesses hidden, pass arguments literally without a command shell, and retain drafts after errors or concurrent edits. Actual application proxy support and authenticated long-session stability require manual acceptance.

## 0.6.0 - Unreleased

- Hide the Windows console when launching the isolated Mihomo process for OpenAI failover generation and node benchmarking.
- Retry subscription HTTP 403 responses with Mihomo-compatible request headers and bounded user-agent fallbacks, while keeping one 30-second request/body deadline and redacting token-bearing URLs from transport errors.
- Skip duplicate subscription revisions and live reloads when a successful refresh returns unchanged source, including serialized re-checks after concurrent refreshes.
- Add a Settings update panel with an optional quiet startup check, an explicit manual check, and a validated official stable-release link; it never downloads, installs, restarts, or changes proxy state in the background.
- Unify sidebar navigation states and rounded scrollbars across themes, remove the accidental rectangular navigation backdrop, and keep focus/hover styling inside clipped rounded bounds.
- Rename the application, window and tray branding to RouteDeck, and the repository, npm/Cargo package and main executable to `routedeck`.
- Update the helper's main-application lookup and the branding, release and repository-sync checks to the new names.
- Retain `com.cmmuu.mihomodesktop`, the existing application-data directory, the `mihomo-tun-helper` binary and service identity, and the `# mihomo-codex-rule:` metadata format for compatibility.
- Preserve historical release artifacts, compliance records and Figma references under their original names. No proxy, TUN or subscription behavior change is intended by this rename.
- Track the rename as a new source version; cross-platform installer migration and live network acceptance are not implied by automated checks or a successful build.

## 0.5.0 - 2026-09-04

- Add experimental Windows 10/11 TUN support through an elevated application session; the entire app runs as administrator without installing a persistent Windows service. Real TUN routing and recovery acceptance remain pending.
- Show platform-specific TUN permission guidance, including how to exit the tray app and restart with administrator privileges on Windows.
- Attach Windows core, native-validator and version-probe processes to a private Job Object at creation; stopping the core, exiting the app or terminating the app closes the protected process tree.
- Require at least one Google/Cloudflare connectivity target to pass proxy and TUN checks; retain OpenAI-specific failures as visible warnings without blocking otherwise usable connectivity.
- Track Windows system-proxy transitions with original, before and applied snapshots, including recovery from interrupted apply/restore operations and compatibility with legacy snapshots.
- Check proxy ownership before restoration so a later switch to Clash or another client is preserved; retain proxy bypass entries, temporarily clear PAC during takeover, and restore the previous PAC when still owned.
- Add coverage for Windows permission reporting, process ownership, proxy recovery and nonblocking connectivity warnings; distinguish automated checks from actual installation and network takeover acceptance.

## 0.4.0 - 2026-09-03

- Replace the desktop and sidebar icons with the original “双路汇聚” routing monogram; preserve the application, data-directory and helper identities.
- Add visual user-rule creation, editing, deletion, enable/disable, ordering and notes, alongside an advanced text editor and the live effective-rule view.
- Support rule-only text/YAML import and a copyable export panel that preserves application-specific rule metadata.
- Persist global user rules independently of subscriptions, prepend enabled rules ahead of managed AI/subscription rules, and retain the latest 20 previous rule snapshots.
- Require structural, target and native Mihomo validation before applying a draft; warn about routing modes and reject stale revisions without losing drafts.
- Apply rule and profile changes through a shared configuration transaction guard, with hot reload and restoration of the previous configuration on apply/persistence failure.
- Rebuild profile activation, refresh and rollback from their original source plus current user rules; keep isolated OpenAI benchmarks independent of user-rule targets.
- Bound native-validator execution time and diagnostic output, and add frontend, Rust, isolated-browser and real macOS save/rollback coverage.

## 0.3.2 - 2026-09-03

- Add immediately saved appearance choices: neutral Light, neutral Dark, Deep Purple, and System (Light/Dark only).
- Repair the historical light/dark CSS overrides and synchronize the native application appearance with the saved preference.
- Add a theme-only settings command that preserves proxy ports, subscriptions, autostart preferences and controller credentials.
- Restore the previous appearance when persistence fails and prevent concurrent theme writes.
- Add theme state-machine tests, backend persistence tests, and an isolated browser preview fixture for visual regression checks without proxy takeover.
- Fix unused Unix-only permission parameters and command helpers in Windows strict Clippy checks without changing runtime behavior.

## 0.3.1 - 2026-09-03

Includes the previously unreleased changes from 2026-08-23 and the following rename:

- Rename the app, project, package, window, tray and main executable to `mihomo-codex`.
- Retain the original bundle identifier, application-data path and TUN helper service identity for upgrade compatibility.
- Update the helper peer-binary lookup to the renamed main executable.
- Add a branding and compatibility-identity regression check.
- Correct the bundled Mihomo v1.19.30 license from the historical MIT mislabel to the upstream GPL v3 text, pin its hash, and include the matching upstream source with the first GitHub release.

### Added

- A shared application-status App Shell header rendered above the page scroller on every route.
- Global System Proxy and TUN controls in the application-status header, synchronized with runtime and operating-system state.
- Per-subscription OpenAI failover generation and regeneration actions, with global single-job progress, cancellation and failure feedback.
- Standby-profile OpenAI failover generation that writes a validated new revision without activating the profile or reloading the running core.
- OS-global real-time upload and download monitoring independent of Mihomo, Manual, System Proxy or TUN state.
- Figma component `Global Traffic / Sidebar` with runtime status on the left and vertically stacked upload/download rates on the right.
- Figma component `Global Traffic / Menu Bar` and a compact monochrome macOS template status item with antialiased M, arrows and upload/download text.
- Semibold menu-bar rate values with light glyph emboldening for improved recognition at native status-item scale.
- `showGlobalTraffic` setting, enabled by default and persisted through settings schema v3.
- Cross-platform network counter sampling through `sysinfo 0.39.6`, with one-second normalized rates and Tauri event delivery.

### Changed

- Refresh, start and stop controls now live in the shared application-status header instead of being duplicated inside individual pages.
- Page scrolling is isolated to `page-scroll`; the sidebar and application-status controls remain visible while content scrolls.
- OpenAI failover generation reloads Mihomo only when its target subscription is still active at commit time; standby subscriptions preserve the active profile and runtime PID.
- Non-network settings, including global traffic monitoring, can be saved while Mihomo is running.
- Tray left-click continues to open the application home page while the icon is updated with live traffic data.
- Subscription deletion now uses an in-app confirmation dialog instead of the unreliable WebView-native `window.confirm` path.
- Successful deletion reports a top-right toast only after storage deletion succeeds; clicking delete on the active subscription now gives an explicit switch-first message instead of doing nothing.
- Regular views now use the native main-content scroller instead of nested subscription/profile list scrolling, preserving mouse-wheel and trackpad momentum.
- Each view restores its previous scroll position during navigation; confirmation and node-detail dialogs lock background scrolling.
- The subscription import card remains visible with a lightweight wide-screen sticky position, while compact and short windows fall back to natural document flow.

### Privacy

- Traffic monitoring reads aggregate interface byte counters only and excludes process, destination, domain and payload data.
- Loopback, TUN, VPN and common virtual interfaces are filtered to avoid duplicate accounting.

## 0.3.0 - 2026-08-19

### Added

- Figma-first subscription management screen with masked sources, validation state, node/revision summaries, refresh, activate, version and delete actions.
- Current-node details on the proxy page with recursive route chain, provider, masked endpoint, protocol capabilities, delay history and retest controls.
- Local-proxy safety check covering Google HTTP 204, Cloudflare HTTP 204 and the unauthenticated OpenAI Models HTTP 401 response.
- Tray left-click now opens and focuses the main window on the Overview page; the tray menu remains available from right-click.
- New Figma-designed deep-purple routing monogram app icon, exported to macOS ICNS, Windows ICO, Linux PNG and Windows Store logo sizes.
- Reproducible ad-hoc macOS bundle signing through Tauri `bundle.macOS.signingIdentity: "-"`.
- Figma-first proxy-page redesign with local variables, typography/effect styles and reusable Button, Nav Item, Metric Cell and Proxy Group Card components.
- Pixel-matched 1180×780 deep-purple glass implementation based on Figma node `8:2`, while preserving all original proxy-group controls.
- Managed `🤖 OpenAI 自动灾备` fallback policy with up to ten references to existing subscription nodes.
- Isolated temporary Mihomo benchmark runtime that never changes the active system proxy or TUN state.
- Two-stage OpenAI reachability, latency, jitter and bounded 2 MiB throughput evaluation.
- Three-way concurrent bandwidth checks through node-specific loopback listeners.
- Automatic generation after subscription import, automatic maintenance after subscription refresh and manual regeneration from the proxy page.
- Persistent benchmark report, progress, cancellation, health check, disable and revision rollback controls.
- High-priority OpenAI and ChatGPT domain rules based on the official OpenAI network list.
- Runtime config hot reload with rollback when applying a generated policy fails.

### Security

- System Proxy startup is transactional: preflight before OS mutation, read-back verification, post-apply validation and snapshot restoration on every failure path.
- macOS proxy capture and mutation target the default-route physical service and exclude virtual services with empty devices.
- Automatic proxy groups use HTTPS 204 health checks; subscription status pseudo-nodes are removed from effective proxies and group membership.
- Node-details serialization is whitelist-only and never exposes UUIDs, passwords, tokens, keys or unmasked server endpoints.
- Refreshing a standby subscription updates only that profile revision and no longer changes the globally active profile or runtime configuration.
- Benchmark listeners bind only to `127.0.0.1`; temporary files use private permissions and are removed when the job finishes.
- OpenAI reachability checks use the unauthenticated Models endpoint with expected HTTP 401 and never request or persist an OpenAI API key.
- Subscription metadata/status pseudo-nodes are excluded from benchmark candidates.

## 0.2.0 - 2026-08-19

### Added

- Deep-purple premium glass theme with layered translucency, 32–50 px backdrop blur and Gaussian light fields.
- One-click System Proxy and TUN switches with automatic stop/apply/restart orchestration.
- Per-profile Global, Rule and Direct routing controls on the home screen.
- Subscription import progress state, first-run GeoIP／GeoSite explanation, concurrency guard and URL-based deduplication.
- Reproducible Mihomo 1.19.30 sidecar preparation for six target triples.
- Pinned compressed-asset SHA-256 checks.
- Versioned profiles, immutable revisions, activation and rollback.
- Effective configuration generation with local security overrides.
- Runtime state, port preflight, bounded redacted logs and crash visibility.
- Mihomo API integration for proxies, rules, connections and delay tests.
- Manual, System Proxy and TUN modes.
- macOS, Windows and Linux system-proxy adapters.
- Full dashboard, profiles, proxies, rules, connections, logs, diagnostics and settings UI.
- System tray, single-instance behavior and launch-at-login integration.
- Native CI and release workflows for macOS, Windows and Linux.

### Security

- CSP and explicit Tauri capability configuration.
- Controller secret remains inside the Rust process and protected settings file.
- Remote config cannot override local controller exposure, ports, allow-lan or TUN policy.
- Logs redact URL queries, UUIDs and secret-like fields.
