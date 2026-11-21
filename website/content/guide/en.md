# Serylane user guide

Install Serylane, bring your own subscription or local YAML, choose a proxy mode, route AI traffic by domain, keep the app updated, and troubleshoot what you see.

## Download and install
<!-- anchor: install -->

Get the current verified release from [the website](https://serylane.cmmuu.com/#download) or the [Download Center](https://downloads.cmmuu.com/projects/serylane). The client does not include nodes, subscriptions, or AI accounts.

- Windows: choose x64 for 64-bit Intel or AMD PCs, or ARM64 for Windows on Arm.
- macOS: choose Apple silicon for an M-series chip or Intel for an Intel Mac. Open the DMG and drag Serylane into Applications. macOS 13 or later is required.
- Linux: choose the AppImage for your architecture, make it executable, and run it.

Starting in v0.7.51, new users choose an interface language before entering Serylane. The Windows setup executable from the website asks during installation. On macOS, Linux, and Windows MSI installs, choose the language on first launch. The system match is preselected and Follow system is also available. Upgrades keep the saved language.

## Get started in five minutes
<!-- anchor: quick-start -->

1. In Subscriptions, choose Add Subscription, enter a name and a Mihomo or Clash Meta subscription URL, then validate and add it. For a local YAML, open it from Profiles, review the validation, then create and activate the profile.
2. Check that the intended subscription or profile is selected. Adding a new subscription only replaces an existing selection if you choose to activate it. Selecting a profile does not start the proxy.
3. Start with System Proxy in the top bar. Once it is running, open a familiar website and use Connections to check whether a request reached the core and matched the expected rule. Use Diagnostics if needed.

## Choose a proxy mode
<!-- anchor: proxy-modes -->

- **System Proxy** is for clients that read OS proxy settings. On Linux it uses GNOME gsettings; on other desktops, use the local port when supported by the client.
- **TUN** is for apps that ignore the system proxy. On macOS, use a complete signed and notarized installer, then approve the helper in System Settings → General → Login Items & Extensions if asked. On Windows, TUN is experimental and needs administrator privileges. It is not implemented on Linux.
- **Local port** is for apps that let you enter a proxy manually. Use `127.0.0.1` and Serylane’s mixed port with HTTP or SOCKS5. Do not use the controller port.

Helper readiness is not evidence of traffic takeover. Check the selected mode and an actual connection; stop the proxy when finished. Network mode controls how traffic enters the core, while Rule, Global, and Direct modes control what happens next.

## Manage subscriptions
<!-- anchor: subscriptions -->

Refresh an individual card or all subscriptions. Usage and profiles refresh separately; a failed refresh keeps the last usable configuration. Use More or right-click to edit a card, then refresh after changing the URL. For a failed import, make sure the link is a Mihomo or Clash Meta subscription, not an account page, and check the subscription download route and failed stage. A failed selected proxy route does not silently switch to direct.

## Choose nodes manually or use AI Proxy
<!-- anchor: nodes -->

These selection modes are mutually exclusive. Select a node from a subscription policy group or run a latency test. AI Proxy accepts 1–10 qualifying nodes; a single node has no backup. After choosing manually, click Apply Node and wait for the applied state. Choose Resume Automatic to restore automatic selection. A new node applies to future connections, not a model stream already interrupted.

## AI tools, domain routing, and Codex
<!-- anchor: ai-tools -->

Domain routing is off by default and only affects requests already entering the core in Rule mode. Choose tool presets or domains, select an exit, save the draft, then explicitly enable or apply it. `example.com` matches the root domain; `*.example.com` matches subdomains only. See [Domain routing](https://serylane.cmmuu.com/docs/routing).

**Optional: use the system proxy in Codex.** On macOS or Windows, a supported Codex CLI can be started with `codex --enable respect_system_proxy`. This applies to that invocation of the CLI, reads existing OS proxy settings, and does not turn on PAC. Existing PAC rules can select a route, but do not provide failover in this setting or make the same route more reliable. See [OpenAI’s documentation](https://learn.chatgpt.com/docs/enterprise/managed-configuration#use-the-system-proxy-for-native-client-requests) and verify with an actual completed request.

## Check for and install updates
<!-- anchor: updates -->

In Settings → Software Update, check the version and the actual source. The updater tries GitHub direct, the project’s GitHub accelerator, GitHub via the running Serylane proxy, and the Download Center. Installation requires verification of the same version and architecture and the package signature. If the old updater still fails, quit the app and replace it with the matching verified full installer; keep your configuration. See [Download verification](https://serylane.cmmuu.com/docs/verification).

## Troubleshooting
<!-- anchor: troubleshooting -->

- An app is not proxied: check the top-bar mode and Connections. A running local port alone does not take over other apps’ traffic.
- macOS TUN is awaiting approval or asks for re-association: check the Login Items & Extensions permission. If the app explicitly requests re-association, stop the proxy first, then use the helper repair action and wait for readiness.
- Port in use when leaving TUN: check the port and failure stage in technical details. Allow recently stopped connections time to clear. A restored mode with the proxy stopped still requires a start.
- An AI endpoint returns HTTP 401: the endpoint is reachable, but sign-in and a real model request remain unverified.

When reporting a problem, include the version, OS and architecture, proxy mode, time and sanitized error. Do not include subscription URLs, tokens, or keys.
