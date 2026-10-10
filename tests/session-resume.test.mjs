import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

const read = name => readFileSync(new URL(`../${name}`, import.meta.url), "utf8");
const source = ts.transpileModule(read("src/session-resume.ts"), {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
}).outputText;
const { sessionResumeHelp, sessionResumePresentation, canStopSession, startupModeFromSettings, startupModeSettings, startupModeHelp, startupRegistrationPresentation } = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
const subscriptionSource = ts.transpileModule(read("src/subscription-cards.ts"), {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
}).outputText;
const { newestSubscriptionStatus, synchronizedProfiles } = await import(`data:text/javascript;base64,${Buffer.from(subscriptionSource).toString("base64")}`);

test("startup modes preserve legacy intent and unrelated settings", () => {
  const legacy = { launchAtLogin: false, silentStartup: true, restoreLastSession: false, networkMode: "tun", controllerPort: 9090 };
  assert.equal(startupModeFromSettings(legacy), "manual");
  assert.deepEqual(startupModeSettings(legacy, "manual"), legacy);
  assert.deepEqual(startupModeSettings(legacy, "background"), { ...legacy, launchAtLogin: true, silentStartup: true, restoreLastSession: true });
  const window = startupModeSettings(legacy, "window");
  assert.equal(window.silentStartup, false);
  assert.equal(startupModeFromSettings(window), "window");
  assert.equal(startupModeFromSettings({ ...legacy, launchAtLogin: true }), "custom");
  assert.deepEqual(startupModeSettings(legacy, "custom"), legacy);
  assert.equal(legacy.restoreLastSession, false, "reading/changing a draft does not mutate saved preferences");
  assert.throws(() => startupModeSettings(legacy, "invalid"), /启动模式无效/);
  assert.match(startupModeHelp(legacy, "background"), /托盘后台恢复/);
  assert.match(startupModeHelp(legacy, "background"), /上次已停止则保持停止/);
});

test("registration feedback distinguishes saved intent, OS denial and unknown state", () => {
  const status = { launchRequested: true, registered: true, systemAllows: false, desiredRunning: false, message: "系统已禁用。" };
  assert.equal(startupRegistrationPresentation(status).issue, true);
  assert.match(startupRegistrationPresentation(status).text, /已关闭，下次保持停止/);
  assert.equal(startupRegistrationPresentation({ ...status, systemAllows: true }).issue, false);
  assert.equal(startupRegistrationPresentation({ ...status, registered: false }).issue, true);
  assert.equal(startupRegistrationPresentation({ ...status, registered: null }).issue, true);
  assert.match(startupRegistrationPresentation(null).text, /尚未读取/);
});

test("waiting restoration remains cancellable even while the core is stopped", () => {
  assert.equal(sessionResumePresentation({ phase: "waiting_network", message: "等待网络" }).busy, true);
  assert.match(sessionResumePresentation({ phase: "waiting_network", message: "等待网络" }).title, /等待网络/);
  assert.match(read("src/main.ts"), /!canStopSession\(store.runtime\?\.phase, sessionResumeStatus, startupStatus\)/);
  assert.equal(canStopSession("stopped", { phase: "paused", message: "配置待处理" }, null), true);
  const startup = { launchRequested: false, registered: false, systemAllows: null, desiredRunning: true, message: "" };
  assert.equal(canStopSession("crashed", { phase: "idle", message: "" }, startup), true);
  assert.equal(canStopSession("stopped", { phase: "idle", message: "" }, { ...startup, desiredRunning: false }), false);
});

test("missing restore status never claims success or starts a core", () => {
  const view = sessionResumePresentation(null);
  assert.equal(view.visible, false);
  assert.equal(view.busy, false);
  assert.match(view.message, /尚未读取/);
});

test("pending and restoring show progress, not connection success", () => {
  for (const phase of ["pending", "restoring", "waiting_network"]) {
    const view = sessionResumePresentation({ phase, message: "正在检查本地核心。" });
    assert.equal(view.busy, true);
    assert.equal(view.visible, true);
    assert.doesNotMatch(view.title, /成功|已连接/);
  }
});

test("paused restoration remains visible with its actual safety reason", () => {
  const message = "其他代理正在使用系统设置，请手动处理。";
  assert.deepEqual(sessionResumePresentation({ phase: "paused", message }), {
    busy: false, visible: true, title: "上次运行状态暂未恢复", message,
  });
});

test("idle and restored avoid a permanent global success banner", () => {
  for (const phase of ["idle", "restored"]) {
    assert.equal(sessionResumePresentation({ phase, message: "状态已读取。" }).visible, false);
  }
});

test("restore help distinguishes login launch, manual opening and opt out", () => {
  assert.match(sessionResumeHelp(false, true), /手动打开应用才会恢复/);
  assert.match(sessionResumeHelp(true, true), /登录后自动打开应用并恢复/);
  assert.match(sessionResumeHelp(true, true), /上次已停止则保持停止/);
  assert.match(sessionResumeHelp(true, true), /不会自动接入 Codex 或启动其他程序/);
  assert.doesNotMatch(sessionResumeHelp(false, false), /登录后自动/);
  assert.match(sessionResumeHelp(false, false), /不会自动启动代理核心/);
  assert.match(sessionResumeHelp(true, true, true), /托盘后台恢复/);
  assert.doesNotMatch(sessionResumeHelp(true, true, true), /自动打开应用/);
});

test("settings save, polling and event paths are wired to the same persisted option", () => {
  const main = read("src/main.ts");
  assert.match(main, /startupModeSettings\(store\.settings, startupModeDraft/);
  assert.match(main, /startupModeDraft = null/);
  assert.match(main, /startupModeDraft \?\? savedStartupMode/);
  assert.match(read("src/settings-view.ts"), /id="settings-startup-mode"/);
  assert.match(read("src/api.ts"), /invoke<StartupStatus>\("get_startup_status"\)/);
  assert.match(main, /listen<SessionResumeStatus>\("session-resume-status"/);
  assert.match(main, /revision !== sessionResumeRevision/);
  assert.match(read("src/api.ts"), /invoke<SessionResumeStatus>\("get_session_resume_status"\)/);
  assert.match(read("src/settings-view.ts"), /id="session-resume-status" role="status" aria-live="polite"/);
});

test("late initial status reads cannot overwrite a newer completed-resume refresh", async () => {
  let finishFirst;
  const first = new Promise(resolve => { finishFirst = resolve; });
  let calls = 0;
  const noop = () => {};
  const oldQuota = { checkedAt: "2026-09-20T01:00:00Z", usage: { downloadBytes: 10 } };
  const newQuota = { checkedAt: "2026-09-20T01:05:00Z", usage: { downloadBytes: 20 } };
  const subscription = status => ({ profile: { id: "quota-fixture" }, status });
  const context = {
    baseReadSequence: 0, runtimeMutationRevision: 0, proxyReadSequence: 0, overviewNodeDetails: {}, refreshProxies: async () => {}, openAiCosts: { refresh: async () => {} },
    runtimeActionInFlight: false, networkModeSwitching: false, settingsSaving: false, manualModeSwitching: false,
    sessionResumeReadBusy: true,
    themeController: { mutationRevision: 0, sync: () => true },
    store: { subscriptions: [] }, action: async (_message, run) => run(), newestSubscriptionStatus, synchronizedProfiles,
    api: new Proxy({
      settings: () => ++calls === 1 ? first : Promise.resolve({ networkMode: "tun" }),
      subscriptions: async () => [subscription(oldQuota)],
    }, {
      get: (target, key) => target[key] ?? (async () => null),
    }),
    renderHeader: noop, renderOverview: noop, renderProfiles: noop, renderSubscriptions: noop,
    renderSettings: noop, renderManualMode: noop, renderOpenAiPolicy: noop, renderGlobalTraffic: noop, scheduleAutomaticUpdateCheck: noop,
  };
  const main = read("src/main.ts");
  const code = main.slice(main.indexOf("async function refreshBase()"), main.indexOf("function renderHeader()"));
  vm.createContext(context);
  vm.runInContext(ts.transpileModule(code, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText, context);
  const old = context.refreshBase();
  // Simulate a background quota event while the older full-page read is pending.
  context.store.subscriptions = [subscription(newQuota)];
  assert.equal(await context.refreshBase(), true);
  assert.equal(context.store.subscriptions[0].status, newQuota);
  finishFirst({ networkMode: "manual" });
  assert.equal(await old, false);
  assert.equal(context.store.settings.networkMode, "tun");
  assert.equal(context.store.subscriptions[0].status, newQuota);
});

test("native login repair exposes OS approval separately and never rewrites draft preference", () => {
  const status = { state: "needs_repair", launchRequested: true, registered: true, systemAllows: true, repairAvailable: true, desiredRunning: false, message: "旧登录项待迁移。" };
  assert.equal(startupRegistrationPresentation(status).issue, true);
  assert.match(startupRegistrationPresentation(status).text, /需要修复/);
  assert.match(startupRegistrationPresentation({ ...status, state: "requires_approval", systemAllows: false }).text, /待系统允许/);
  const markup = read("src/settings-view.ts");
  assert.match(markup, /id="settings-startup-repair"/);
  assert.match(markup, /id="settings-startup-open"/);
  assert.match(read("src/api.ts"), /invoke<StartupStatus>\("repair_startup_registration"\)/);
  const main = read("src/main.ts");
  const repair = main.slice(main.indexOf('$("#settings-startup-repair")!.addEventListener'), main.indexOf('document.querySelectorAll<HTMLInputElement>', main.indexOf('$("#settings-startup-repair")!.addEventListener')));
  assert.match(repair, /startupModeDraft !== null/);
  assert.match(repair, /startupRepairBusy = true/);
  assert.doesNotMatch(repair, /saveSettings|setNetworkMode|startActive|api\.stop/);
});

test("native login bundle uses a short-lived exact-app launcher and retains autostart semantics", () => {
  const config = JSON.parse(read("src-tauri/tauri.conf.json"));
  assert.equal(config.bundle.macOS.files["Library/LaunchAgents/com.cmmuu.mihomodesktop.login.plist"], "helper/com.cmmuu.mihomodesktop.login.plist");
  const plist = read("src-tauri/helper/com.cmmuu.mihomodesktop.login.plist");
  assert.match(plist, /Contents\/MacOS\/serylane-login-helper/);
  assert.doesNotMatch(plist, /<key>KeepAlive<\/key>/);
  const launcher = read("src-tauri/src/bin/serylane-login-helper.rs");
  assert.match(launcher, /\.arg\(bundle\)/);
  assert.match(launcher, /\.arg\("--autostart"\)/);
  assert.doesNotMatch(launcher, /\.arg\("-b"\)/);
});

test("all native binaries have explicit Cargo paths to avoid Tauri 2.11.4 discovery deduplication", () => {
  const manifest = read("src-tauri/Cargo.toml");
  for (const [name, path] of [["serylane", "src/main.rs"], ["mihomo-tun-helper", "src/bin/mihomo-tun-helper.rs"], ["serylane-login-helper", "src/bin/serylane-login-helper.rs"]]) {
    const blocks = manifest.split("[[bin]]").slice(1);
    assert.ok(blocks.some(block => block.includes(`name = "${name}"`) && block.includes(`path = "${path}"`)), `${name} must retain an explicit source path`);
  }
  const verifier = read("scripts/verify-macos-layout.sh");
  assert.match(verifier, /serylane mihomo-tun-helper serylane-login-helper mihomo/);
  assert.match(verifier, /BundleProgram Contents\/MacOS\/mihomo-tun-helper/);
  assert.match(verifier, /codesign --verify --deep --strict/);
});
