# Serylane <img src="assets/brand/app-icon-128.png" width="48" height="48" align="left" hspace="12" alt="">

**Open-source Mihomo desktop proxy client for Windows, macOS, and Linux.**

[![Latest GitHub release](https://img.shields.io/github/v/release/CMMUU/serylane?label=release&logo=github&color=2563eb)](https://github.com/CMMUU/serylane/releases/latest)
[![GPL-3.0-only license](https://img.shields.io/badge/license-GPL--3.0--only-0f766e?logo=gnu)](LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/CMMUU/serylane?label=stars&color=b45309)](https://github.com/CMMUU/serylane)

[Website](https://serylane.cmmuu.com/) · [User guide](https://serylane.cmmuu.com/docs/guide/) · **[Download](https://downloads.cmmuu.com/projects/serylane)** · [Changelog](CHANGELOG.md)

**English**
· [简体中文](website/content/guide/zh-Hans.md)
· [繁體中文](website/content/guide/zh-Hant.md)
· [日本語](website/content/guide/ja.md)
· [한국어](website/content/guide/ko.md)
· [Deutsch](website/content/guide/de.md)
· [Français](website/content/guide/fr.md)
· [Español](website/content/guide/es.md)
· [Português (Brasil)](website/content/guide/pt-BR.md)
· [Русский](website/content/guide/ru.md)
· [العربية](website/content/guide/ar.md)

Serylane uses **Mihomo, Rust, and Tauri 2** to manage subscriptions, profiles, nodes, routing rules, and system proxy settings. Optional domain routing for AI tools runs inside Mihomo and is off by default. [TUN support](#tun-compatibility) varies by platform.

Serylane is an independent project. Bring your own proxy subscription or Mihomo configuration.

<a name="下载与更新"></a>

## Downloads and updates

- The [download center](https://downloads.cmmuu.com/projects/serylane) supplies the installers. Windows, macOS, and Linux each have x64 and ARM64 builds. On macOS, choose **Intel (x64)** or **Apple silicon (M series, ARM64)**.
- In-app updates check stable releases in this order: direct GitHub access, Serylane's GitHub accelerator, GitHub through the running Serylane proxy, then the download center. The result shows which source the app used.
- If automatic checks are enabled, the app checks in the background after startup. The lower-right notification or sidebar version indicator opens **Settings → Software update** at the download action. The website verifies the current release in the download center every time you select an installer.
- Downloads support cancellation and a limited number of retries. The app verifies the signature and SHA-256 before offering installation. Checking and downloading keep the proxy running. Confirming installation and restarting briefly interrupt connections. On Linux, in-app updates support AppImage.
- **If v0.7.44 gets stuck checking for updates**, quit the app and install the full v0.7.46 or newer package over the existing installation. [Get the installer](https://serylane.cmmuu.com/#download)
- **If in-app installation fails on v0.7.24–v0.7.27**, get the full installer for a published v0.7.28 or newer release. Quit the old app, replace it in the same location, and keep the configuration directory. This first upgrade needs the standalone installer because the installed updater runs the older code.

<a name="快速开始"></a>

## Quick start

1. Download the installer that matches your operating system and processor.
2. Under **Subscriptions → Add subscription**, enter a Clash Meta or Mihomo YAML subscription URL. Select the activation checkbox to use the subscription as soon as it is imported. Use **Profiles** to import a local YAML file.
3. Select a profile and click **Start** to use your saved network mode. To switch modes, select system proxy or TUN and follow your platform's permission prompts.
4. To route AI tool domains through a specific node, configure and enable routing separately. Test general connectivity, access to the target site, and sustained model streaming separately.

<a name="当前功能"></a>

## Features

- **Languages.** Choose an interface language during Windows setup or on the first launch on macOS, Linux, and Windows MSI. The system match is preselected. Settings lists all 11 languages by their native names and applies your choice immediately.
- **Subscriptions and profiles.** The subscription dialog keeps the current session's draft when you close it with Esc or click outside. Submitted imports continue in the background. Subscription cards show the active profile, plan usage, expiry, and configuration updates. Expand a card for usage samples and error details. Profiles support validation and rollback.
- **Node selection.** Choose manual selection or AI proxy mode. Manual selection keeps the subscription's policy groups and rules. AI proxy ranks candidates by connection performance, rate multiplier, and latency. It shows their ranking and the current outbound node. It keeps a healthy node and selects a ranked replacement for new connections after confirming a failure. Use the numbered subscription recommendations to choose a node manually. The control shows it as applied only after Mihomo confirms the pin. Changes affect new connections that match the group.
- **macOS menu bar.** Left- or right-click the icon-only or two-line traffic item to open the native AppKit menu. It follows the system appearance. On startup, the S icon appears when the status item is positioned, and the traffic view uses the final item size before it appears. The menu and main window share controls and status checks. Dismiss the menu by selecting a command, clicking outside, or pressing Esc. **Open main window** returns you to your previous page.
- **Node details.** The overview shows the selected policy group's actual outbound node, chain, protocol, and latest valid latency measurement. In rule mode, the outbound depends on the matching rule. Changing a node applies to future connections.
- **Domain routing.** Select the tool domains and outbound node, save the draft, and confirm activation separately. Serylane writes the rules to Mihomo and checks that they took effect. Routing operates at the domain level. New Codex integration is temporarily disabled.
- **Startup and restore.** Choose to launch manually, restore in the background at login, or restore with the window visible. Serylane restores the last profile and network mode and keeps the proxy stopped if it was previously stopped. The main Start button uses the saved mode.
- **Logs and interface.** Logs are kept for three days by default, with a configurable range of 1–90 days. Log content is capped at 256 KiB and 2,000 entries. The interface includes frosted-glass themes, hidden scrollbars, a system tray, and traffic monitoring. Long lists load in batches, and read results apply to the operation that requested them.
- **Step-by-step diagnostics.** Checks appear as they run, with the active step marked in progress. Completed results stay visible, and individual failures let later checks continue. When the core is stopped, diagnostics skip network checks and keep the existing proxy state.

<!-- product-contract:start -->
| Source fact | Current value |
| --- | --- |
| Source version | 0.7.51 (see Releases for published builds) |
| Website installers | Download center |
| In-app updates | Direct GitHub → GitHub accelerator → GitHub through the running Serylane proxy → Download center |
| Eligible AI nodes | 1–10. A single node has no fallback. |
| Per-app proxy | Temporarily disabled |
| Enhanced failover | Temporarily disabled |
| New Codex integration | Temporarily disabled |
<!-- product-contract:end -->

This table is generated from the source and checked by tests. The [current product contract](docs/当前实现与业务契约.md) documents implementation details, defaults, and limitations. Historical release notes describe their specific versions.

<a name="tun-平台边界"></a>

## TUN compatibility

| Platform | Support |
| --- | --- |
| Windows 10/11 | Experimental. Run the entire app as an administrator. Full verification of live routes, DNS, and recovery after abnormal exits is pending. |
| macOS | Uses `utun` and a privileged helper. Choose a full installer with Developer ID signing and Apple notarization. After an in-place upgrade, Serylane checks and restores the existing helper registration. If macOS requests background-item permission, approve it in System Settings and return to the app. The compatibility signing used by builds such as v0.7.44 lacks the Developer ID required for TUN. |
| Linux | TUN is unavailable. Use system proxy mode. |

[v0.7.45 installation, permissions, and verification](docs/发布说明-v0.7.45.md) · [Platform stabilization plan](docs/TUN三端稳定化方案-2026-10-30.md)

Upgrade recovery, traffic takeover, and route restoration after stopping were tested in an isolated Apple silicon VM running macOS 27.2.1. Intel installer checks and runtime testing are tracked separately.

<a name="开发与验证"></a>

## Development and checks

Install Node.js, npm, Rust, and the Tauri prerequisites for your platform. On Windows, install the appropriate MSVC and C++ Build Tools, Windows SDK, and WebView2.

```bash
npm ci
npm run core:prepare
npm run tauri dev
npm run check
npm run test:rust
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

`core:prepare` downloads the pinned Mihomo build for the current Rust target and verifies its SHA-256. Use `core:prepare:all` to prepare every target. Serylane runs Mihomo's native configuration check before starting the core.

<a name="文档与发行维护"></a>

## Documentation and release maintenance

- [Documentation index](docs/index.md) · [Current product contract](docs/当前实现与业务契约.md) · [Proxy quality verification](docs/代理质量验收.md) · [Changelog](CHANGELOG.md)
- [App icon](assets/brand/图标说明.md) · [Central project documentation](https://github.com/CMMUU/project-docs/tree/main/serylane)
- When features change, update the current documentation, generated source facts, and relevant tests. Run `node scripts/product-docs.mjs --write` to refresh the facts and `node scripts/product-docs.mjs` to verify them.
- Releases require builds for all six targets, native installation and upgrade checks, and signing verification. GitHub hosts the source and immutable release artifacts and provides one route for in-app updates. The website directs installer downloads to the download center. Preserve historical tags, signed assets, compatibility identifiers, and asset filenames.
- The website deploys from trusted `main` independently of the legacy mirror. After a release, verify the download center's version and file integrity and all six website download routes. Delivery is complete once the release is archived in the download center and the public checks pass.
- Verify updater signatures, macOS Developer ID signing, Apple notarization, and permission for background services separately. TUN requires a Developer ID identity even when the package carries a valid updater signature.

<a name="许可证"></a>

## License

The source is licensed under [GNU GPL v3 (GPL-3.0-only)](LICENSE). See [third-party notices](THIRD_PARTY_NOTICES.md) for dependencies. Each release includes a license inventory and SBOM generated from its lockfiles and archived with the signed artifacts. The `private: true` package setting prevents accidental npm publication and leaves GPL rights intact.

<a name="star-增长趋势"></a>

## Star history

<a href="https://www.star-history.com/?repos=CMMUU%2Fserylane&amp;type=date&amp;legend=top-left">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=CMMUU/serylane&amp;type=date&amp;theme=dark&amp;legend=top-left" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=CMMUU/serylane&amp;type=date&amp;legend=top-left" />
    <img alt="GitHub star history for Serylane" src="https://api.star-history.com/chart?repos=CMMUU/serylane&amp;type=date&amp;legend=top-left" width="800" />
  </picture>
</a>

Star History provides the chart. Cached data may lag behind GitHub.
