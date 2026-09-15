import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

const read = (name) => readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
const moduleUrl = (source) => `data:text/javascript;base64,${Buffer.from(ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
}).outputText).toString("base64")}`;
const uiUrl = moduleUrl(read("ui.ts"));
const { NAV_ITEMS, navigationMarkup, preferenceSwitch } = await import(uiUrl);
const settingsUrl = moduleUrl(read("settings-view.ts")
  .replace('"./ui"', JSON.stringify(uiUrl))
  .replace('"./theme"', JSON.stringify(moduleUrl(read("theme.ts")))));
const { preferencesMarkup } = await import(settingsUrl);
const css = read("desktop-theme.css");
const main = read("main.ts");
const { startRuntimeInMode, canStartRuntime } = await import(moduleUrl(read("runtime-start.ts")));

function runtimeFixture(mode = "manual", options = {}) {
  const calls = [];
  const runtime = (phase) => ({ phase, message: `synthetic ${phase}`, lastError: null });
  const state = { settings: { networkMode: mode }, runtime: runtime("stopped"), systemProxyActive: false, hasProfile: true, busy: false };
  let persistedMode = options.actualMode ?? mode;
  const context = {
    state: () => state,
    setBusy: (busy) => { state.busy = busy; },
    setSettings: (settings) => { state.settings = settings; },
    setRuntime: (value) => { state.runtime = value; },
    readRuntime: async () => { calls.push("read"); return runtime(options.actualPhase ?? "stopped"); },
    readSettings: async () => ({ networkMode: persistedMode }),
    setNetworkMode: async (nextMode) => {
      calls.push(`mode:${nextMode}`);
      if (options.failSave || (options.failRollback && nextMode === mode)) throw new Error("save failed");
      persistedMode = nextMode;
      return { networkMode: nextMode };
    },
    startActive: async () => {
      calls.push(`start:${state.settings.networkMode}`);
      if (options.waitForStart) await options.waitForStart;
      if (options.failStart) throw new Error("start failed");
      state.systemProxyActive = state.settings.networkMode === "system_proxy" && options.proxyActive !== false;
      return runtime(options.resultPhase ?? "running");
    },
    ensureTunReady: async () => { calls.push("tun-ready"); return options.tunReady ?? true; },
    refresh: async () => {
      calls.push("refresh");
      assert.equal(state.busy, true, "lock remains held while status is refreshed");
      if (options.waitForRefresh) await options.waitForRefresh;
      if (options.failRefresh) throw new Error("refresh failed");
      if (options.refreshedPhase) state.runtime = runtime(options.refreshedPhase);
      if (options.refreshedMode) state.settings = { networkMode: options.refreshedMode };
    },
  };
  return { calls, state, start: (intent = "system_proxy") => startRuntimeInMode(intent, context) };
}

test("main Start requests the last saved scheme accessibly", async () => {
  assert.match(main, /#global-start.*addEventListener\("click", \(\) => void startRuntime\("previous"\)\)/);
  assert.match(main, /id="global-start"[^>]*aria-label="按上次方案启动"/);
  const f = runtimeFixture();
  assert.equal((await f.start("previous")).kind, "started");
  assert.deepEqual(f.calls, ["read", "start:manual", "refresh"]);
  assert.equal(f.state.settings.networkMode, "manual");
  assert.equal(f.state.busy, false);
});

test("an existing system-proxy preference starts directly without rewriting mode", async () => {
  const f = runtimeFixture("system_proxy");
  assert.equal((await f.start()).kind, "started");
  assert.deepEqual(f.calls, ["read", "start:system_proxy", "refresh"]);
});

test("main Start restores saved TUN with permission preflight, not system proxy", async () => {
  const f = runtimeFixture("tun");
  assert.equal((await f.start("previous")).kind, "started");
  assert.deepEqual(f.calls, ["read", "tun-ready", "start:tun", "refresh"]);
  const stale = runtimeFixture("system_proxy", { actualMode: "tun", tunReady: false });
  assert.equal((await stale.start("previous")).kind, "cancelled");
  assert.deepEqual(stale.calls, ["read", "tun-ready", "refresh"]);
});

test("startup uses the authoritative prior preference rather than a stale toolbar snapshot", async () => {
  const currentSystem = runtimeFixture("manual", { actualMode: "system_proxy" });
  assert.equal((await currentSystem.start()).kind, "started");
  assert.deepEqual(currentSystem.calls, ["read", "start:system_proxy", "refresh"]);
  const rollback = runtimeFixture("manual", { actualMode: "tun", failStart: true });
  assert.equal((await rollback.start()).restored, true);
  assert.equal(rollback.state.settings.networkMode, "tun");
});

test("explicit TUN still requires readiness and never turns into system proxy", async () => {
  for (const previousMode of ["manual", "tun"]) {
    const f = runtimeFixture(previousMode);
    assert.equal((await f.start("tun")).kind, "started");
    assert.deepEqual(f.calls, ["read", "tun-ready", ...(previousMode === "tun" ? [] : ["mode:tun"]), "start:tun", "refresh"]);
  }
  const denied = runtimeFixture("manual", { tunReady: false });
  assert.equal((await denied.start("tun")).kind, "cancelled");
  assert.deepEqual(denied.calls, ["read", "tun-ready", "refresh"]);
  // Same-mode TUN toggle forwards its explicit intent rather than the main button default.
  assert.match(main, /store\.settings\.networkMode === mode && !running\) \{\s*void startRuntime\(mode\)/);
});

test("a failed mode save never starts the runtime or claims rollback", async () => {
  const f = runtimeFixture("manual", { failSave: true });
  const result = await f.start();
  assert.equal(result.kind, "failed");
  assert.equal(result.restored, false);
  assert.deepEqual(f.calls, ["read", "mode:system_proxy", "refresh"]);
  assert.equal(f.state.settings.networkMode, "manual");
  assert.equal(f.state.busy, false);
});

test("failed start restores the old preference without restarting a previous session", async () => {
  for (const previousMode of ["manual", "tun"]) {
    const f = runtimeFixture(previousMode, { failStart: true });
    const result = await f.start();
    assert.equal(result.kind, "failed");
    assert.equal(result.restored, true);
    assert.equal(f.state.settings.networkMode, previousMode);
    assert.deepEqual(f.calls, ["read", "mode:system_proxy", "start:system_proxy", `mode:${previousMode}`, "refresh"]);
  }
});

test("rollback failure and non-running results are never reported as successful starts", async () => {
  const f = runtimeFixture("manual", { failStart: true, failRollback: true });
  const result = await f.start();
  assert.equal(result.kind, "failed");
  assert.equal(result.restored, false);
  assert.match(result.rollbackError.message, /save failed/);
  assert.equal(f.state.busy, false);
  assert.equal((await runtimeFixture("manual", { resultPhase: "crashed" }).start()).kind, "failed");
});

test("busy lock deduplicates real asynchronous preflight/start/refresh work", async () => {
  let releaseStart, releaseRefresh;
  const waitForStart = new Promise(resolve => { releaseStart = resolve; });
  const waitForRefresh = new Promise(resolve => { releaseRefresh = resolve; });
  const f = runtimeFixture("manual", { waitForStart, waitForRefresh });
  const first = f.start();
  assert.equal((await f.start()).kind, "skipped");
  await Promise.resolve();
  assert.equal((await f.start("tun")).kind, "skipped");
  releaseStart();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal((await f.start()).kind, "skipped");
  releaseRefresh();
  assert.equal((await first).kind, "started");
  assert.equal(f.calls.filter(call => call.startsWith("start:")).length, 1);
  assert.equal(f.state.busy, false);
});

test("missing profile, busy/transitional state and stale running sessions remain guarded", async () => {
  const missing = runtimeFixture(); missing.state.hasProfile = false;
  assert.equal((await missing.start()).kind, "needs-profile");
  assert.deepEqual(missing.calls, []);
  for (const phase of ["running", "starting", "validating", "stopping", "recovering"]) {
    assert.equal(canStartRuntime({ phase }), false);
    const f = runtimeFixture("manual", { actualPhase: phase });
    assert.equal((await f.start()).kind, "skipped");
    assert.deepEqual(f.calls, ["read", "refresh"]);
  }
  const busy = runtimeFixture(); busy.state.busy = true;
  assert.equal((await busy.start()).kind, "skipped");
  assert.deepEqual(busy.calls, []);
});

test("status refresh failure is exposed and releases the busy lock", async () => {
  const f = runtimeFixture("manual", { failRefresh: true });
  const result = await f.start();
  assert.match(result.refreshError.message, /refresh failed/);
  assert.equal(f.state.busy, false);
});

test("final refreshed runtime and actual system proxy state must agree before success", async () => {
  for (const options of [{ proxyActive: false }, { refreshedPhase: "crashed" }, { refreshedMode: "manual" }]) {
    const f = runtimeFixture("manual", options);
    const result = await f.start();
    assert.equal(result.kind, "failed");
    assert.match(result.error.message, /未确认/);
    assert.deepEqual(f.calls, ["read", "mode:system_proxy", "start:system_proxy", "refresh"]);
  }
  const tun = runtimeFixture("tun", { proxyActive: false });
  assert.equal((await tun.start("tun")).kind, "started", "TUN does not use system proxy or macOS-only helper runtime flags");
});

test("refreshBase rejects a delayed pre-start snapshot after a network mutation", async () => {
  let resolveSettings;
  const settingsRead = new Promise(resolve => { resolveSettings = resolve; });
  const staleRuntime = { phase: "stopped" };
  const noop = () => {};
  const environment = {
    themeController: { mutationRevision: 0, sync: () => true },
    runtimeMutationRevision: 0, baseReadSequence: 0, runtimeActionInFlight: false, networkModeSwitching: false, settingsSaving: false,
    store: { settings: { networkMode: "manual" }, runtime: staleRuntime },
    action: async (_label, operation) => operation(),
    api: new Proxy({ settings: () => settingsRead, runtime: async () => staleRuntime }, { get: (target, key) => target[key] ?? (async () => null) }),
    renderHeader: noop, renderOverview: noop, renderProfiles: noop, renderSubscriptions: noop, renderSettings: noop,
    renderOpenAiPolicy: noop, renderGlobalTraffic: noop, scheduleAutomaticUpdateCheck: noop,
  };
  const source = main.slice(main.indexOf("async function refreshBase()"), main.indexOf("function renderHeader()"));
  vm.createContext(environment);
  vm.runInContext(ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText, environment);
  const pending = environment.refreshBase();
  environment.runtimeMutationRevision++;
  environment.store.settings = { networkMode: "system_proxy" };
  environment.store.runtime = { phase: "running" };
  resolveSettings({ networkMode: "manual" });
  await pending;
  assert.equal(environment.store.settings.networkMode, "system_proxy");
  assert.equal(environment.store.runtime.phase, "running");
  // A read requested during the write is stale even if the write has finished
  // by the time the response arrives and no newer write has begun.
  environment.runtimeActionInFlight = true;
  const duringWrite = environment.refreshBase();
  environment.runtimeActionInFlight = false;
  await duringWrite;
  assert.equal(environment.store.settings.networkMode, "system_proxy");
  assert.equal(environment.store.runtime.phase, "running");
});

function runtimeRefreshFixture() {
  const requests = [];
  const noop = () => {};
  let activeRequest;
  let reads = 0;
  const environment = {
    runtimeMutationRevision: 0, runtimeReadSequence: 0,
    runtimeActionInFlight: false, networkModeSwitching: false, settingsSaving: false,
    store: { runtime: { phase: "stopped" }, systemProxy: { active: false } },
    api: {
      runtime: () => { reads++; activeRequest = requests.shift(); return activeRequest.promise; },
      systemProxy: async () => ({ active: activeRequest.proxyActive }),
      tunHelperStatus: async () => ({ state: "ready" }),
    },
    renderTunHelper: noop, renderHeader: noop, renderOverview: noop, renderSubscriptions: noop, refreshConnectionFeedback: async () => {},
  };
  const source = main.slice(main.indexOf("async function refreshRuntimeOnly("), main.indexOf("async function startRuntime("));
  vm.createContext(environment);
  vm.runInContext(ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText, environment);
  return {
    environment,
    reads: () => reads,
    queue(phase, proxyActive) {
      let resolve;
      const promise = new Promise(done => { resolve = () => done({ phase }); });
      requests.push({ promise, proxyActive });
      return resolve;
    },
  };
}

test("runtime refresh rejects pre-start late reads and skips timer reads during startup", async () => {
  const f = runtimeRefreshFixture();
  const completeOldRead = f.queue("stopped", false);
  const beforeStart = f.environment.refreshRuntimeOnly();
  f.environment.runtimeActionInFlight = true;
  f.environment.runtimeMutationRevision++;
  await f.environment.refreshRuntimeOnly();
  assert.equal(f.reads(), 1, "busy timer does not dispatch a stale runtime request");
  const completeFinalRead = f.queue("running", true);
  const finalRead = f.environment.refreshRuntimeOnly(true);
  completeFinalRead();
  await finalRead;
  f.environment.runtimeActionInFlight = false;
  f.environment.runtimeMutationRevision++;
  completeOldRead();
  await beforeStart;
  assert.equal(f.environment.store.runtime.phase, "running");
  assert.equal(f.environment.store.systemProxy.active, true);
});

test("latest runtime read wins and a late in-action read cannot cross busy release", async () => {
  const f = runtimeRefreshFixture();
  f.environment.runtimeActionInFlight = true;
  const completeEarlierRead = f.queue("crashed", false);
  const earlier = f.environment.refreshRuntimeOnly(true);
  const completeFinalRead = f.queue("running", true);
  const final = f.environment.refreshRuntimeOnly(true);
  completeFinalRead();
  await final;
  completeEarlierRead();
  await earlier;
  assert.equal(f.environment.store.runtime.phase, "running");
  const completeLateRead = f.queue("stopped", false);
  const late = f.environment.refreshRuntimeOnly(true);
  f.environment.runtimeActionInFlight = false;
  f.environment.runtimeMutationRevision++;
  completeLateRead();
  await late;
  assert.equal(f.environment.store.runtime.phase, "running");
  assert.equal(f.environment.store.systemProxy.active, true);
});

test("all eleven navigation items retain unique routes, labels and code-native icons", () => {
  assert.deepEqual(NAV_ITEMS.map(({ id }) => id), ["overview", "profiles", "subscriptions", "proxies", "programs", "routing", "rules", "connections", "logs", "diagnostics", "settings"]);
  assert.equal(new Set(NAV_ITEMS.map(({ id }) => id)).size, 11);
  assert.equal((navigationMarkup.match(/<svg /g) ?? []).length, 11);
  assert.equal((navigationMarkup.match(/aria-current="page"/g) ?? []).length, 1);
  assert.equal((navigationMarkup.match(/aria-hidden="true"/g) ?? []).length, 11);
});
test("switches retain a native checked input and keyboard-focusable control", () => {
  assert.match(preferenceSwitch("settings-launch"), /id="settings-launch" type="checkbox" role="switch"/);
  assert.match(css, /input:focus-visible \+ \.toggle-track/);
  assert.match(css, /prefers-reduced-motion/);
  assert.match(css, /forced-colors/);
});
test("preference groups preserve all backend binding IDs without duplication", () => {
  const ids = [...preferencesMarkup.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]);
  assert.equal(new Set(ids).size, ids.length);
  for (const id of ["settings-mode", "settings-mixed-port", "settings-controller-port", "settings-startup-mode", "settings-startup-check", "startup-registration-status", "settings-global-traffic", "settings-retention", "settings-form", "update-preferences-form", "app-update-current", "app-update-check", "app-update-save", "app-update-download", "app-update-cancel", "app-update-install", "app-update-message", "network-mode-help"]) assert.ok(ids.includes(id), id);
  assert.match(preferencesMarkup, /安装和重启会短暂中断代理连接/);
});
test("network modes remain native radio drafts, not immediate proxy mutations", () => {
  for (const mode of ["manual", "system_proxy", "tun"]) assert.match(preferencesMarkup, new RegExp(`name="settings-network-mode" value="${mode}"`));
  const handler = main.slice(main.indexOf('radio.addEventListener("change"'), main.indexOf('$("#settings-form")!.addEventListener("submit"'));
  assert.match(handler, /\.value = radio\.value/);
  assert.doesNotMatch(handler, /api\.|switchNetworkMode|startRuntime|stopRuntime/);
});
test("shared theme tokens own the appearance and nav has one accessible active state", () => {
  assert.match(css, /--bg: #e8edfa/);
  assert.match(css, /--primary-background: #2764eb/);
  assert.match(css, /backdrop-filter: blur\(var\(--glass-blur\)\)/);
  assert.match(css, /prefers-reduced-transparency/);
  assert.doesNotMatch(navigationMarkup, /nav-icon-(blue|purple|green|orange|yellow|red)/);
  assert.match(css, /\.sidebar \.nav-item\[aria-current="page"\]/);
  assert.doesNotMatch(read("styles.css"), /body:has\(#\w+-view/);
  assert.doesNotMatch(main, /<h1[^>]*>应用状态/);
});
test("idle update hides status details, never the check/preferences controls", () => {
  assert.ok(preferencesMarkup.indexOf('id="app-update-check"') < preferencesMarkup.indexOf('id="app-update-feedback"'));
  assert.ok(preferencesMarkup.indexOf('id="app-update-save"') < preferencesMarkup.indexOf('id="app-update-feedback"'));
  assert.match(main, /#app-update-feedback[\s\S]*?appUpdateStatus\.phase === "idle"/);
});
