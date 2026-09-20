import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(new URL("../src/program-proxy.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText;
const { parseProgramArguments, suggestedProgramName, launchBlockReason, programRunStatus, programManagerMarkup, applicationIdentity, isMacApplication, allowsRelativeDirectory, requiresApplicationInspection } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);

test("arguments are one literal argument per line, not shell syntax", () => {
  assert.deepEqual(parseProgramArguments("--option\r\nC:\\folder with spaces\n\n& echo not-a-command"), ["--option", "C:\\folder with spaces", "& echo not-a-command"]);
  assert.deepEqual(parseProgramArguments("  kept spaces  \n\t"), ["  kept spaces  "]);
  assert.deepEqual(parseProgramArguments(""), []);
  assert.throws(() => parseProgramArguments("x\0y"));
  assert.throws(() => parseProgramArguments("x\ry"));
  assert.throws(() => parseProgramArguments("中".repeat(3000)));
  assert.throws(() => parseProgramArguments(Array(65).fill("x").join("\n")));
});

test("name suggestions remove platform application suffixes and retain Unicode", () => {
  assert.equal(suggestedProgramName("C:\\应用 文件夹\\开发工具.EXE"), "开发工具");
  assert.equal(suggestedProgramName("C:/apps/demo.exe"), "demo");
  assert.equal(suggestedProgramName("/Applications/开发工具.app"), "开发工具");
  assert.equal(suggestedProgramName("/usr/share/applications/editor.desktop"), "editor");
});

test("launch is unavailable for unsupported, missing, running or stopped cases", () => {
  const state = { supported: true, coreRunning: true };
  const program = { available: true, runningPid: null };
  assert.equal(launchBlockReason(program, state), null);
  assert.match(launchBlockReason(program, { ...state, supported: false }), /此平台/);
  assert.match(launchBlockReason({ ...program, available: false }, state), /找不到/);
  assert.match(launchBlockReason({ ...program, runningPid: 4567 }, state), /退出/);
  assert.match(launchBlockReason(program, { ...state, coreRunning: false }), /核心/);
});

test("program UI uses scoped API and explicit confirmation, not global proxy mutations", () => {
  assert.match(programManagerMarkup, /不强制接管已运行的程序/);
  assert.match(programManagerMarkup, /保存不会启动程序/);
  assert.match(source, /services\.confirm\(\{ title: `代理启动/);
  assert.match(source, /saveProxyProgram\(input, draftRevision!/);
  assert.match(source, /if \(path === null\) return/);
  assert.doesNotMatch(source, /setNetworkMode|startActive|updateSettings|\.kill\(|setx|localStorage/);
});

test("program page shares the single accessible navigation styling contract", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const css = readFileSync(new URL("../src/desktop-theme.css", import.meta.url), "utf8");
  const ui = readFileSync(new URL("../src/ui.ts", import.meta.url), "utf8");
  assert.match(main, /id="programs-view"/);
  assert.match(main, /if \(view === "programs"\) void programManager\.refresh\(\)/);
  assert.match(ui, /id: "programs", label: "程序代理"/);
  assert.match(css, /\.sidebar \.nav-item\[aria-current="page"\]/);
  assert.match(main, /button\.setAttribute\("aria-current", "page"\)/);
  assert.match(main, /button\.removeAttribute\("aria-current"\)/);
  assert.doesNotMatch(css, /body:has\(#programs-view/);
});

test("compatibility diagnostics never enable system routing for unrelated clients", () => {
  const cargo = readFileSync(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8");
  const lib = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
  const controller = readFileSync(new URL("../src-tauri/src/mihomo_api.rs", import.meta.url), "utf8");
  assert.doesNotMatch(cargo, /client-proxy-system/);
  assert.match(controller, /reqwest::Client::builder\(\)\s*\.no_proxy\(\)/);
  assert.match(lib, /if result\.updated && result\.profile\.openai_policy\.auto_maintain/);
});

test("bound application states explain installation failures rather than a generic missing file", () => {
  for (const availability of ["not_installed", "updating", "maintenance", "needs_relink", "read_error", "unsupported_launch"]) {
    const program = { available: false, runningPid: null, resolution: { availability, detail: `说明：${availability}` } };
    assert.equal(launchBlockReason(program, { supported: true, coreRunning: true }), `说明：${availability}`);
  }
});

test("installed-app binding and portable files keep independent directory and identity settings", () => {
  assert.match(programManagerMarkup, /从已安装应用选择/);
  assert.match(programManagerMarkup, /手动选择文件 \/ 应用/);
  assert.match(programManagerMarkup, /包内相对目录/);
  assert.match(source, /readOnly = binding !== null/);
  assert.match(source, /installedProxyApplications\(force\)/);
  assert.match(source, /workingDirectoryRelative:/);
  assert.match(source, /editProgram\(program\)/);
  assert.match(source, /program-path-details/);
});

test("README uses token-free theme-aware Star History without adding a chart scheduler", () => {
  const readme = readFileSync(new URL("../README.md", import.meta.url), "utf8");
  assert.ok(readme.indexOf("## Star 增长趋势") > readme.indexOf("## 快速开始"));
  assert.match(readme, /prefers-color-scheme: dark/);
  assert.match(readme, /https:\/\/api\.star-history\.com\/chart\?repos=CMMUU\/serylane/);
  assert.doesNotMatch(readme, /sealed_token|[?&]token=/);
});

const windowsBinding = { packageFamilyName: "Fixture.App_family", applicationId: "App" };
const macBinding = { kind: "macos", bundleId: "invalid.fixture.App", location: "/Applications/Fixture.app", requirement: null };
const linuxBinding = { kind: "linux", desktopId: "invalid.fixture.App.desktop", location: "/usr/share/applications/invalid.fixture.App.desktop" };

test("identity labels use platform semantics and preserve duplicate-install locations", () => {
  assert.deepEqual(applicationIdentity(windowsBinding), { source: "Windows MSIX / Store", identity: "Fixture.App_family!App", location: "" });
  assert.deepEqual(applicationIdentity(macBinding), { source: "macOS 应用", identity: "invalid.fixture.App", location: "/Applications/Fixture.app" });
  assert.deepEqual(applicationIdentity(linuxBinding), { source: "Linux 桌面应用", identity: "invalid.fixture.App.desktop", location: "/usr/share/applications/invalid.fixture.App.desktop" });
  assert.notDeepEqual(applicationIdentity(macBinding), applicationIdentity({ ...macBinding, location: "/Users/demo/Applications/Fixture.app" }));
});

test("manual application selection inspects only platform-native app containers", () => {
  assert.equal(requiresApplicationInspection("/Applications/开发工具.APP", "macos"), true);
  assert.equal(requiresApplicationInspection("/Applications/开发工具.app/", "macos"), true);
  assert.equal(requiresApplicationInspection("/usr/local/bin/editor", "macos"), false);
  assert.equal(requiresApplicationInspection("/usr/share/applications/editor.desktop", "linux"), true);
  assert.equal(requiresApplicationInspection("/usr/bin/editor", "linux"), false);
  assert.equal(requiresApplicationInspection("C:/apps/demo.exe", "windows"), false);
  assert.equal(requiresApplicationInspection("/Applications/App.app", "linux"), false);
  assert.match(source, /inspectProxyApplication\(path\)/);
});

test("Mac keeps native working directory while package-relative mode is Windows-only", () => {
  assert.equal(isMacApplication(macBinding), true);
  assert.equal(isMacApplication(linuxBinding), false);
  for (const binding of [macBinding, linuxBinding, null]) assert.equal(allowsRelativeDirectory(binding), false);
  assert.equal(allowsRelativeDirectory(windowsBinding), true);
  assert.match(source, /原工作目录设置已保留/);
  assert.match(source, /默认保留桌面入口的参数与工作目录/);
  assert.match(source, /program-directory-clear/);
  assert.match(source, /⌘Q/);
});

test("catalog refresh is independent of form operations and bounded with stale-result guards", () => {
  assert.match(source, /program-installed"\)\.addEventListener\("click", \(\) => showApplications\(false\)\)/);
  assert.match(source, /program-app-refresh"\)\.addEventListener\("click", \(\) => showApplications\(true\)\)/);
  assert.match(source, /generation !== catalogGeneration \|\| !pickerVisible\(\)/);
  assert.match(source, /setTimeout\(\(\) => \{ catalogTimer = null; void readApplications\(generation, false, deadline\); \}, 500\)/);
  assert.match(source, /Date\.now\(\) \+ 30000/);
  assert.match(source, /Math\.min\(15000, deadline - Date\.now\(\)\)/);
  assert.match(source, /if \(results\.innerHTML !== markup\)/);
  assert.match(source, /visibilityObserver\.disconnect\(\)/);
  assert.match(source, /window\.removeEventListener\("pagehide", pageHidden\)/);
  assert.doesNotMatch(source, /operation\(showApplications\)/);
});

test("non-ready identity stays launch-blocked even if availability flag is stale", () => {
  assert.equal(launchBlockReason({ available: true, runningPid: null, resolution: { availability: "unsupported_launch", detail: "特殊激活入口" } }, { supported: true, coreRunning: true }), "特殊激活入口");
  assert.match(source, /保存关联不会启动该入口/);
  assert.match(source, /app\.version \? ` · 当前版本/);
});

test("synthetic fixtures cover native identities, delayed catalogs and failure without native I/O", () => {
  const fixture = readFileSync(new URL("fixtures/theme-preview.ts", import.meta.url), "utf8");
  assert.match(fixture, /platform: previewPlatform/);
  assert.match(fixture, /kind: "macos"/);
  assert.match(fixture, /kind: "linux"/);
  assert.match(fixture, /fixtureCatalog\(args\.refresh === true\)/);
  assert.match(fixture, /catalogDelayMs/);
  assert.match(fixture, /catalogFail/);
  assert.match(fixture, /fixtureCatalogForces/);
  assert.match(fixture, /command === "inspect_proxy_application"/);
});

test("native launch timeout is pending confirmation, never PID zero or a completed launch", () => {
  const program = { available: true, runningPid: null, launchPending: true };
  const state = { supported: true, coreRunning: true };
  assert.equal(launchBlockReason(program, state), "启动结果待确认；请稍后刷新查看");
  assert.equal(programRunStatus(program), "启动结果待确认；请稍后刷新查看");
  assert.equal(programRunStatus({ ...program, runningPid: 0 }), "启动结果待确认；请稍后刷新查看");
  assert.doesNotMatch(programRunStatus({ ...program, launchPending: false, runningPid: 0 }), /PID 0/);
  const completed = { ...program, launchPending: false, runningPid: 4567 };
  assert.match(programRunStatus(completed), /PID 4567/);
  assert.match(launchBlockReason(completed, state), /退出/);
  assert.equal(launchBlockReason({ ...completed, runningPid: null }, state), null);
  // Pending blocks only launch: edit/delete/save remain governed by the existing busy state.
  assert.match(source, /data-program-action="edit" \$\{busy \? "disabled" : ""\}/);
  assert.match(source, /data-program-action="delete" \$\{busy \? "disabled" : ""\}/);
  assert.match(source, /state\.programs\.find\(\(entry\) => entry\.id === program\.id\)\?\.launchPending/);
});
