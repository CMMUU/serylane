import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(join(root, path), "utf8").replaceAll("\r\n", "\n");
const json = (path) => JSON.parse(read(path));
const displayName = "Serylane";
// Display/binary names change; data/helper IDs and signed compatibility aliases do not.
const installerName = "Serylane";
const slug = "serylane";
const libraryName = "serylane_lib";
// These identifiers are compatibility contracts, not user-facing branding.
const identifier = "com.cmmuu.mihomodesktop";
const helperName = "mihomo-tun-helper";
const ruleMetadataPrefix = "# mihomo-codex-rule:";
const pkg = json("package.json");
const lock = json("package-lock.json");
const config = json("src-tauri/tauri.conf.json");
const cargo = read("src-tauri/Cargo.toml");
const protocol = read("src-tauri/src/tun_service/protocol.rs");

assert.equal(pkg.name, slug);
assert.equal(lock.name, slug);
assert.equal(lock.packages[""].name, slug);
assert.equal(lock.version, pkg.version);
assert.equal(lock.packages[""].version, pkg.version);
assert.equal(config.productName, installerName);
assert.equal(config.mainBinaryName, "serylane");
assert.equal(config.bundle.windows.wix.upgradeCode, "3dc8b957-21f1-51b9-ab4c-df67ce49b53d");
assert.deepEqual(config.bundle.windows.wix.fragmentPaths, ["installer/login-cleanup.wxs"]);
assert.deepEqual(config.bundle.windows.wix.componentGroupRefs, ["SerylaneLoginCleanup"]);
const msiCleanup = read("src-tauri/installer/login-cleanup.wxs");
assert.match(msiCleanup, /ExeCommand="--installer-remove-login"/);
assert.match(msiCleanup, /Impersonate="yes"/);
assert.match(msiCleanup, /Before="RemoveFiles"/);
assert.match(msiCleanup, /Installed AND REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE/);
assert.equal(config.bundle.windows.nsis.template, "installer/installer.nsi");
assert.equal(config.version, pkg.version);
assert.equal(config.identifier, identifier);
assert.equal(config.app.windows.find((window) => window.label === "main").title, displayName);
assert.ok(cargo.includes(`[package]\nname = "${slug}"\nversion = "${pkg.version}"`));
assert.ok(cargo.includes(`default-run = "${slug}"`));
assert.ok(cargo.includes(`version = "${pkg.version}"`));
assert.ok(cargo.includes(`name = "${libraryName}"`));
assert.ok(read("src-tauri/Cargo.lock").includes(`name = "${slug}"\nversion = "${pkg.version}"`));
assert.ok(read("src-tauri/src/main.rs").includes(`${libraryName}::run()`));
assert.ok(read(`src-tauri/src/bin/${helperName}.rs`).includes(`${libraryName}::tun_service::daemon::run()`));
assert.ok(protocol.includes('APP_BINARY_NAME: &str = "serylane"'));
assert.ok(protocol.includes(`LABEL: &str = "${identifier}.tun-helper"`));
assert.ok(protocol.includes(`HELPER_BINARY_NAME: &str = "${helperName}"`));
assert.ok(protocol.includes(`PLIST_NAME: &CStr = c"${identifier}.tun-helper.plist"`));
assert.ok(read("index.html").includes(`<title>${displayName}</title>`));
assert.ok(read("src/main.ts").includes(`<strong>${displayName}</strong>`));
assert.doesNotMatch(read("src/main.ts"), /RouteDeck/, "All user-facing dialogs must use Serylane");
assert.ok(read("src-tauri/src/lib.rs").includes(`tooltip("${displayName}")`));
assert.ok(read("src-tauri/src/lib.rs").includes(`product_name: "${displayName}"`));
assert.ok(read("src-tauri/src/traffic_monitor.rs").includes(`Some("${displayName}")`));
assert.ok(read("tests/fixtures/theme-preview.html").includes(`<title>${displayName} ·`));
assert.ok(read("tests/fixtures/theme-preview.ts").includes(`productName: "${displayName}"`));
const introduction = read("README.md");
assert.match(introduction, /^# Serylane /);
assert.doesNotMatch(introduction, /formerly\s+RouteDeck|(?:原|曾用名)[^。\n]*RouteDeck/i,
  "The project introduction uses Serylane without former-brand marketing copy");
assert.doesNotMatch(cargo.match(/^description\s*=.*$/m)?.[0] ?? "", /RouteDeck/i,
  "Package description uses only the current display brand");

const plist = read(`src-tauri/helper/${identifier}.tun-helper.plist`);
assert.ok(plist.includes(`<string>${identifier}</string>`));
assert.ok(plist.includes(`<string>${identifier}.tun-helper</string>`));
assert.ok(plist.includes(`<string>Contents/MacOS/${helperName}</string>`));
assert.equal(
  config.bundle.macOS.files[`Library/LaunchDaemons/${identifier}.tun-helper.plist`],
  `helper/${identifier}.tun-helper.plist`,
);
assert.ok(read("src-tauri/src/storage.rs").includes(".app_data_dir()"));
assert.ok(read("src-tauri/src/user_rules.rs").includes(`METADATA_PREFIX: &str = "${ruleMetadataPrefix}"`));
assert.ok(read("src/rule-manager.ts").includes(ruleMetadataPrefix));
assert.ok(read("tests/fixtures/theme-preview.ts").includes(ruleMetadataPrefix));

// Existing Codex leases, old updater clients and package managers must survive a
// display-only rename. Do not broaden these exact identities to new-name aliases.
assert.ok(read("src-tauri/src/local_routing/codex.rs").includes(`const PROVIDER: &str = "routedeck"`));
assert.ok(read("src-tauri/src/local_routing/codex.rs").includes('"codex-lease.json"'));
assert.ok(read("src-tauri/src/local_routing/codex.rs").includes(`provider["name"] = value("${displayName} 本地路由")`));
const tray = read("src-tauri/src/traffic_monitor.rs");
const trayLayout = read("src-tauri/src/traffic_monitor/macos_layout.rs");
assert.ok(trayLayout.includes('tauri::include_image!("icons/128x128.png")'));
assert.doesNotMatch(tray, /draw_m_mark|const M_X|const M_Y/);
assert.ok(tray.includes("macos_layout::brand_icon()"), "Native tray uses the compact shared S mark");
assert.equal((tray.match(/macos_layout::bitmap_canvas\(\)/g) ?? []).length, 2,
  "Both macOS bitmap paths must share the compact S mark and fixed layout");
assert.ok(read("src-tauri/tests/macos_tray_native.rs").includes("macos_layout::brand_icon()"),
  "Native geometry tests must render the production icon, not a square stand-in");
assert.deepEqual(readFileSync(join(root, "src-tauri/icons/128x128.png")),
  readFileSync(join(root, "assets/brand/app-icon-128.png")), "Sidebar and tray must use identical app artwork");
const updater = read("src-tauri/src/app_update.rs");
for (const endpoint of [
  "https://github.com/CMMUU/serylane/releases/latest/download/latest-serylane.json",
  "https://github.com/CMMUU/serylane/releases/latest/download/latest.json",
  "https://gitee.com/api/v5/repos/cmmuu/serylane/releases/latest",
  "https://github.com/CMMUU/routedeck/releases",
  "https://gitee.com/cmmuu/serylane/releases",
]) assert.ok(updater.includes(`"${endpoint}"`), `Retain updater endpoint: ${endpoint}`);
assert.ok(updater.includes('format!("RouteDeck_{version}_{suffix}")'));
assert.match(updater, /if url\.as_str\(\) != expected\s*&& url\.as_str\(\) != legacy_channel\s*&& !legacy_github\s*&& !legacy_gitee/);
assert.ok(read(".github/workflows/sync-gitee.yml").includes("--repo serylane"));
assert.ok(read("scripts/sync_gitee.py").includes('SOURCE_REPOS = {"serylane": "serylane"}'));
assert.ok(updater.includes('format!("{base}/download/{tag}/latest-serylane-gitee.json")'));
assert.ok(read("scripts/updater_release.py").includes('("latest-serylane.json", "https://github.com/CMMUU/serylane")'));
assert.ok(read("scripts/updater_release.py").includes('("latest-serylane-gitee.json", "https://gitee.com/cmmuu/serylane")'));
assert.ok(updater.includes('"https://github.com/CMMUU/serylane/releases"'));
for (const path of [".github/workflows/release.yml", ".github/workflows/sync-gitee.yml"]) {
  assert.ok(read(path).includes("github.repository == 'CMMUU/serylane'"));
  assert.ok(!read(path).includes("github.repository == 'CMMUU/routedeck'"));
}
assert.ok(read("scripts/updater_release.py").includes('prefix = f"RouteDeck_{version}"'));
assert.ok(read("scripts/publish_github_release.py").includes('prefix = f"{brand}_{version}"'));
assert.equal(createHash("sha256").update(config.plugins.updater.pubkey).digest("hex"),
  "de516897f5cc1e06aab7fa1e822ce6f6db8ae95547c15b2b216d9c0391c52792", "Retain the existing signed-update verification key");

console.log(`${displayName} ${pkg.version}: display/installer/binary names unified; legacy update/data identities retained`);
