import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const javascript = (source) => ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
}).outputText;
const importSource = (path) => import(`data:text/javascript;base64,${Buffer.from(javascript(read(path))).toString("base64")}`);
const { prepareTunForStart } = await importSource("src/tun-preflight.ts");
const { friendlyError, mountConnectionFeedback } = await importSource("src/connection-feedback.ts");
const { canStartRuntime } = await importSource("src/runtime-start.ts");
const main = read("src/main.ts");
const switchStart = main.indexOf("async function switchNetworkMode(");
const preflightStart = main.indexOf("async function ensureTunHelperReady(");
const nextStart = main.indexOf("async function switchRoutingMode(");
assert.ok(switchStart >= 0 && preflightStart > switchStart && nextStart > preflightStart);
const switchSource = main.slice(switchStart, preflightStart);
const preflightSource = main.slice(preflightStart, nextStart);
const noop = () => {};
const helper = (state = "ready", overrides = {}) => ({
  supported: true, state, message: `fixture ${state}`, protocolVersion: 1,
  runtimeRunning: false, runtimePid: null, runtimeVersion: null, lastError: null,
  ...overrides,
});
const failure = (detail) => ({ code: "CORE_ERROR", userMessage: {
  title: "合成启动失败", description: "合成操作未完成。", action: "diagnostics", details: detail,
} });
const runtime = (phase) => ({ phase, message: `fixture ${phase}`, lastError: null });
const deferred = () => {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
};
const tick = () => new Promise(resolve => setImmediate(resolve));

function feedbackView(initial) {
  const elements = new Map();
  const root = {
    classList: { toggle: noop }, dataset: {},
    querySelector(selector) {
      if (!elements.has(selector)) elements.set(selector, { textContent: "", addEventListener: noop });
      return elements.get(selector);
    },
  };
  const previousWindow = globalThis.window;
  // The actual component runs, but its cosmetic elapsed timer never schedules real work.
  globalThis.window = { setInterval: () => 0, clearInterval: noop };
  let component;
  try { component = mountConnectionFeedback(root, async () => {}); }
  finally {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  }
  if (initial) component.accept(initial);
  return { component, text: field => root.querySelector(`#connection-feedback-${field}`).textContent };
}

function preflightFixture(platform = "macos", options = {}) {
  const calls = [], observed = [];
  const context = {
    platform,
    status: async () => {
      calls.push("status");
      if (options.statusError) throw options.statusError;
      return options.status ?? helper();
    },
    install: async () => {
      calls.push("install");
      if (options.installError) throw options.installError;
      return options.installed ?? helper();
    },
    openApproval: async () => {
      calls.push("approval");
      if (options.approvalError) throw options.approvalError;
      return null;
    },
    prepare: async () => {
      calls.push("prepare");
      if (options.prepareWait) await options.prepareWait;
      if (options.prepareError) throw options.prepareError;
      return Object.hasOwn(options, "payload") ? options.payload : null;
    },
    // Repair is deliberately not a preflight operation, even if the caller has one.
    repair: async () => { calls.push("repair"); throw new Error("unexpected repair"); },
    observed: value => observed.push(value),
  };
  return { calls, observed, context, run: () => prepareTunForStart(context) };
}

function mainFixture(options = {}) {
  const calls = [], issues = [], toasts = [], observations = [];
  let actualMode = options.actualMode ?? "system_proxy";
  let actualRuntime = runtime(options.actualPhase ?? "running");
  let saveCount = 0, startCount = 0;
  const preflight = preflightFixture(options.platform ?? "macos", options);
  const elements = new Map();
  const feedback = options.feedbackSnapshot ? feedbackView(options.feedbackSnapshot) : null;
  const env = {
    store: {
      appInfo: { targetOs: options.platform ?? "macos" },
      settings: { networkMode: options.cachedMode ?? actualMode },
      runtime: runtime(options.cachedPhase ?? actualRuntime.phase),
      activeProfile: options.hasProfile === false ? null : { profile: { id: "fixture" } },
      systemProxy: { active: actualMode === "system_proxy" && actualRuntime.phase === "running" },
    },
    networkModeSwitching: false, runtimeActionInFlight: false, settingsSaving: false, runtimeMutationRevision: 0,
    prepareTunForStart, canStartRuntime, friendlyError,
    errorMessage: error => error?.userMessage?.details ?? error?.message ?? String(error),
    $: selector => {
      if (!elements.has(selector)) elements.set(selector, { disabled: false });
      return elements.get(selector);
    },
    themeController: { snapshot: {} },
    renderAppearance: noop, renderHeader: noop, renderOverview: noop,
    renderTunHelper: () => observations.push(env.store.tunHelper),
    connectionFeedback: {
      clearIssue: () => feedback?.component.clearIssue(),
      showError: error => { issues.push(friendlyError(error)); feedback?.component.showError(error); },
    },
    toast: (message, tone) => toasts.push({ message, tone }),
    navigate: view => calls.push(`navigate:${view}`),
    api: {
      settings: async () => { calls.push("read-settings"); return { networkMode: actualMode }; },
      runtime: async () => { calls.push("read-runtime"); return { ...actualRuntime }; },
      tunHelperStatus: preflight.context.status,
      installTunHelper: preflight.context.install,
      repairTunHelper: preflight.context.repair,
      openTunHelperSettings: preflight.context.openApproval,
      prepareTun: preflight.context.prepare,
      stop: async () => {
        calls.push("stop");
        if (options.stopError) throw options.stopError;
        actualRuntime = runtime("stopped");
        return { ...actualRuntime };
      },
      setNetworkMode: async mode => {
        calls.push(`mode:${mode}`);
        saveCount++;
        if (options.saveErrors?.[saveCount - 1]) throw options.saveErrors[saveCount - 1];
        actualMode = mode;
        return { networkMode: mode };
      },
      startActive: async () => {
        calls.push(`start:${actualMode}`);
        startCount++;
        if (options.startWait) await options.startWait;
        const outcome = options.startResults?.[startCount - 1] ?? "running";
        if (typeof outcome !== "string") {
          actualRuntime = runtime(options.failedStartPhase ?? "crashed");
          throw outcome;
        }
        actualRuntime = runtime(outcome);
        return { ...actualRuntime };
      },
    },
    refreshRuntimeOnly: async force => {
      calls.push("refresh-runtime");
      assert.equal(force, true, "switch must request authoritative readback despite its own lock");
      assert.equal(env.networkModeSwitching, true, "mutation lock covers final runtime readback");
      if (options.refreshWait) await options.refreshWait;
      if (options.refreshError) throw options.refreshError;
      if (options.refreshedPhase) actualRuntime = runtime(options.refreshedPhase);
      env.store.runtime = { ...actualRuntime };
      env.store.systemProxy.active = actualMode === "system_proxy" && actualRuntime.phase === "running";
    },
    refreshConnectionFeedback: async () => {
      calls.push("refresh-feedback");
      assert.equal(env.networkModeSwitching, true, "mutation lock covers final feedback readback");
      if (options.feedbackWait) await options.feedbackWait;
      if (feedback) feedback.component.accept(options.feedbackSnapshot);
    },
    refreshBase: async () => { calls.push("refresh-base"); },
  };
  vm.createContext(env);
  vm.runInContext(javascript(`${switchSource}\n${preflightSource}`), env);
  return {
    env, calls, issues, toasts, observations, preflight, feedback,
    mutations: () => calls.filter(call => /^(stop|mode:|start:)/.test(call)),
    observed: () => ({ mode: actualMode, phase: actualRuntime.phase }),
    prepare: () => env.ensureTunHelperReady(),
    switch: (mode = "tun") => env.switchNetworkMode(mode),
  };
}

test("Tauri Rust unit result contract is represented by JSON null in the browser fixture", () => {
  // serde serializes Rust () as JSON null; Promise<void> does not change this wire value.
  assert.match(read("src-tauri/src/lib.rs"), /async fn prepare_tun_active_profile\([^)]*\)\s*->\s*Result<\(\), AppErrorDto>/);
  assert.match(read("src/api.ts"), /prepareTun:\s*\(\)\s*=>\s*invoke<void>\("prepare_tun_active_profile"\)/);
  const fixture = read("tests/fixtures/theme-preview.ts");
  const start = fixture.indexOf('if (command === "prepare_tun_active_profile") {');
  const end = fixture.indexOf('if (command === "start_active_profile") {', start);
  assert.ok(start >= 0 && end > start);
  const context = { command: "prepare_tun_active_profile", fixtureRuntimeHelperReady: true };
  vm.createContext(context);
  const value = vm.runInContext(`(() => { ${javascript(fixture.slice(start, end))} })()`, context);
  assert.equal(value, JSON.parse("null"), "native IPC succeeds with null; bare return would hide the regression");
});

test("macOS and Windows accept fulfilled null and undefined without interpreting payload truthiness", async () => {
  for (const platform of ["macos", "windows"]) {
    for (const payload of [null, undefined]) {
      const f = preflightFixture(platform, { payload });
      assert.deepEqual(await f.run(), { kind: "ready" });
      assert.deepEqual(f.calls, ["status", "prepare"]);
    }
  }
});

test("status and prepare rejections retain their original error rather than becoming readiness", async () => {
  for (const name of ["statusError", "prepareError"]) {
    const error = failure(name);
    const f = preflightFixture("macos", { [name]: error });
    const result = await f.run();
    assert.equal(result.kind, "failed");
    assert.equal(result.error, error);
    assert.deepEqual(f.calls, name === "statusError" ? ["status"] : ["status", "prepare"]);
  }
});

test("macOS installs only a missing helper and observes the post-install status", async () => {
  const initial = helper("not_installed"), installed = helper();
  const f = preflightFixture("macos", { status: initial, installed });
  assert.equal((await f.run()).kind, "ready");
  assert.deepEqual(f.calls, ["status", "install", "prepare"]);
  assert.deepEqual(f.observed, [initial, installed]);
});

test("macOS approval opens settings without claiming readiness or starting preflight", async () => {
  for (const needsInstall of [false, true]) {
    const f = preflightFixture("macos", {
      status: helper(needsInstall ? "not_installed" : "requires_approval"),
      installed: helper("requires_approval"),
    });
    assert.equal((await f.run()).kind, "blocked");
    assert.deepEqual(f.calls, ["status", ...(needsInstall ? ["install"] : []), "approval"]);
  }
});

test("macOS install and authorization-setting failures remain explicit failures", async () => {
  for (const [state, name] of [["not_installed", "installError"], ["requires_approval", "approvalError"]]) {
    const error = failure(name);
    const f = preflightFixture("macos", { status: helper(state), [name]: error });
    assert.equal((await f.run()).error, error);
    assert.ok(!f.calls.includes("prepare"));
  }
});

test("macOS outdated, unreachable and checking states never automatically reinstall or repair", async () => {
  for (const state of ["outdated", "unreachable", "checking"]) {
    const f = preflightFixture("macos", { status: helper(state) });
    assert.equal((await f.run()).kind, "blocked");
    assert.deepEqual(f.calls, ["status"]);
  }
});

test("Windows state variants never call macOS installation, repair or approval APIs", async () => {
  for (const state of ["not_installed", "requires_approval", "outdated", "unreachable", "checking", "unsupported"]) {
    const f = preflightFixture("windows", { status: helper(state, { supported: state !== "unsupported" }) });
    assert.equal((await f.run()).kind, "blocked");
    assert.deepEqual(f.calls, ["status"]);
  }
});

test("Linux and unknown platforms stay blocked even when given a malformed ready capability", async () => {
  for (const platform of ["linux", "unknown"]) {
    for (const status of [helper("unsupported", { supported: false }), helper()]) {
      const f = preflightFixture(platform, { status });
      assert.equal((await f.run()).kind, "blocked");
      assert.deepEqual(f.calls, ["status"]);
    }
  }
  const unsupported = preflightFixture("macos", { status: helper("ready", { supported: false }) });
  assert.equal((await unsupported.run()).kind, "blocked");
  assert.deepEqual(unsupported.calls, ["status"]);
});

test("the real main preflight accepts null and makes no enabled claim until the switch starts", async () => {
  for (const platform of ["macos", "windows"]) {
    for (const payload of [null, undefined]) {
      const f = mainFixture({ platform, payload });
      assert.equal(await f.prepare(), true);
      assert.deepEqual(f.mutations(), []);
      assert.equal(f.toasts.length, 1);
      assert.equal(f.toasts[0].tone, "info");
      assert.match(f.toasts[0].message, /尚未切换网络/);
      assert.equal(f.issues.length, 0);
    }
  }
});

test("the real main preflight exposes platform-specific block reasons without Mac actions on Windows", async () => {
  for (const [platform, state, supported, action] of [
    ["macos", "requires_approval", true, "settings"],
    ["macos", "unreachable", true, "refresh"],
    ["macos", "checking", true, "refresh"],
    ["macos", "outdated", true, "settings"],
    ["windows", "requires_approval", true, "settings"],
    ["windows", "unreachable", true, "refresh"],
    ["linux", "unsupported", false, "settings"],
  ]) {
    const f = mainFixture({ platform, status: helper(state, { supported, lastError: "fixture diagnostic" }) });
    assert.equal(await f.prepare(), false);
    assert.deepEqual(f.mutations(), []);
    assert.equal(f.issues.at(-1).action, action);
    assert.equal(f.issues.at(-1).details, "fixture diagnostic");
    assert.match(f.issues.at(-1).description, /未切换网络模式/);
    if (platform !== "macos") assert.deepEqual(f.preflight.calls, ["status"]);
  }
});

test("failed or pending preflight leaves the existing proxy untouched, including repeated clicks", async () => {
  const gate = deferred();
  const f = mainFixture({ prepareWait: gate.promise, prepareError: failure("prepare failed") });
  const pending = f.switch();
  await tick();
  assert.equal(f.env.networkModeSwitching, true);
  assert.deepEqual(f.preflight.calls, ["status", "prepare"]);
  assert.deepEqual(f.mutations(), []);
  assert.deepEqual(f.observed(), { mode: "system_proxy", phase: "running" });
  await f.switch();
  assert.deepEqual(f.preflight.calls, ["status", "prepare"]);
  gate.resolve();
  await pending;
  assert.deepEqual(f.mutations(), []);
  assert.match(f.issues.at(-1).title, /预检尚未通过/);
  assert.match(f.issues.at(-1).description, /尚未切换网络模式/);
  assert.equal(f.env.networkModeSwitching, false);
});

test("blocked Linux or Windows permissions never stop a running original session", async () => {
  for (const [platform, status] of [
    ["linux", helper("unsupported", { supported: false })],
    ["windows", helper("requires_approval")],
    ["windows", helper("unreachable")],
  ]) {
    const f = mainFixture({ platform, status });
    await f.switch();
    assert.deepEqual(f.mutations(), []);
    assert.deepEqual(f.observed(), { mode: "system_proxy", phase: "running" });
    assert.equal(f.env.networkModeSwitching, false);
  }
});

test("successful native null preflight precedes stop, mode save and one accepted TUN start", async () => {
  const f = mainFixture();
  await f.switch();
  assert.deepEqual(f.preflight.calls, ["status", "prepare"]);
  assert.deepEqual(f.mutations(), ["stop", "mode:tun", "start:tun"]);
  assert.deepEqual(f.observed(), { mode: "tun", phase: "running" });
  assert.equal(f.toasts.filter(item => item.tone === "success").length, 1);
  assert.equal(f.env.networkModeSwitching, false);
});

test("mode save failure after stopping restores the original running proxy", async () => {
  const f = mainFixture({ saveErrors: [failure("mode save failed")] });
  await f.switch();
  assert.deepEqual(f.mutations(), ["stop", "mode:tun", "mode:system_proxy", "start:system_proxy"]);
  assert.deepEqual(f.observed(), { mode: "system_proxy", phase: "running" });
  assert.match(f.issues.at(-1).description, /已恢复之前的网络模式与运行状态/);
  assert.equal(f.issues.at(-1).details, "mode save failed");
  assert.equal(f.toasts.filter(item => item.tone === "success").length, 0);
});

test("failed TUN start reports successful rollback together with the original diagnostic", async () => {
  const f = mainFixture({ startResults: [failure("driver initialization failed"), "running"] });
  await f.switch();
  assert.deepEqual(f.mutations(), ["stop", "mode:tun", "start:tun", "mode:system_proxy", "start:system_proxy"]);
  assert.deepEqual(f.observed(), { mode: "system_proxy", phase: "running" });
  assert.match(f.issues.at(-1).description, /已恢复/);
  assert.equal(f.issues.at(-1).details, "driver initialization failed");
});

test("a crashed start result never claims TUN success and restores the previous session", async () => {
  const f = mainFixture({ startResults: ["crashed", "running"] });
  await f.switch();
  assert.deepEqual(f.observed(), { mode: "system_proxy", phase: "running" });
  assert.match(f.issues.at(-1).description, /已恢复/);
  assert.equal(f.toasts.filter(item => item.tone === "success").length, 0);
});

test("rollback save and rollback start failures retain both failure details without claiming recovery", async () => {
  for (const options of [
    { startResults: [failure("TUN rejected")], saveErrors: [null, failure("rollback save failed")] },
    { startResults: [failure("TUN rejected"), failure("rollback start failed")] },
    { startResults: [failure("TUN rejected"), "crashed"] },
  ]) {
    const f = mainFixture(options);
    await f.switch();
    const issue = f.issues.at(-1);
    assert.match(issue.description, /恢复原模式时遇到问题/);
    assert.doesNotMatch(issue.description, /已恢复之前/);
    assert.match(issue.details, /TUN rejected/);
    assert.match(issue.details, /rollback|原模式尚未恢复运行/);
    assert.equal(f.toasts.filter(item => item.tone === "success").length, 0);
    assert.equal(f.env.networkModeSwitching, false);
  }
});

test("rollback does not stop or overwrite a still-running or transitional core", async () => {
  for (const failedStartPhase of ["running", "starting", "stopping", "recovering", "validating"]) {
    const f = mainFixture({ failedStartPhase, startResults: [failure("start acknowledgement lost")] });
    await f.switch();
    assert.deepEqual(f.mutations(), ["stop", "mode:tun", "start:tun"]);
    assert.match(f.issues.at(-1).details, /已保留现场/);
    assert.equal(f.observed().phase, failedStartPhase);
  }
});

test("an uncertain stop failure does not launch another core or claim the old session was restored", async () => {
  const f = mainFixture({ stopError: failure("stop confirmation missing") });
  await f.switch();
  assert.deepEqual(f.mutations(), ["stop"]);
  assert.deepEqual(f.observed(), { mode: "system_proxy", phase: "running" });
  assert.doesNotMatch(f.issues.at(-1).description, /已恢复/);
  assert.equal(f.issues.at(-1).details, "stop confirmation missing");
});

test("a previously stopped proxy only restores its setting after startup failure, never starts it", async () => {
  const f = mainFixture({ actualMode: "manual", actualPhase: "stopped", startResults: [failure("TUN rejected")] });
  await f.switch();
  assert.deepEqual(f.mutations(), ["mode:tun", "start:tun", "mode:manual"]);
  assert.match(f.issues.at(-1).description, /代理保持停止/);
  assert.notEqual(f.observed().phase, "running");
});

test("fresh settings, not the stale toolbar mode, determine the restored original session", async () => {
  const f = mainFixture({ cachedMode: "manual", actualMode: "system_proxy", cachedPhase: "stopped", startResults: [failure("TUN rejected"), "running"] });
  await f.switch();
  assert.deepEqual(f.mutations(), ["stop", "mode:tun", "start:tun", "mode:system_proxy", "start:system_proxy"]);
  assert.equal(f.observed().mode, "system_proxy");
});

test("a fresh transitional state blocks switching before preflight or any runtime mutation", async () => {
  for (const actualPhase of ["starting", "validating", "stopping", "recovering"]) {
    const f = mainFixture({ actualPhase, cachedPhase: "running" });
    await f.switch();
    assert.deepEqual(f.mutations(), []);
    assert.deepEqual(f.preflight.calls, []);
    assert.equal(f.issues.length, 1);
    assert.equal(f.env.networkModeSwitching, false);
  }
});

test("non-TUN mode switches preserve the core rather than introducing a stop/start regression", async () => {
  const f = mainFixture({ actualMode: "manual" });
  await f.switch("system_proxy");
  assert.deepEqual(f.mutations(), ["mode:system_proxy"]);
  assert.deepEqual(f.preflight.calls, []);
  assert.deepEqual(f.observed(), { mode: "system_proxy", phase: "running" });
});

test("repeat clicks remain blocked through startup, final runtime readback and final feedback readback", async () => {
  const start = deferred(), refresh = deferred(), feedback = deferred();
  const f = mainFixture({ startWait: start.promise, refreshWait: refresh.promise, feedbackWait: feedback.promise });
  const pending = f.switch();
  await tick();
  assert.equal(f.env.networkModeSwitching, true);
  await f.switch("manual");
  start.resolve();
  await tick();
  assert.ok(f.calls.includes("refresh-runtime"));
  await f.switch("manual");
  refresh.resolve();
  await tick();
  assert.ok(f.calls.includes("refresh-feedback"));
  await f.switch("manual");
  assert.deepEqual(f.mutations(), ["stop", "mode:tun", "start:tun"]);
  feedback.resolve();
  await pending;
  assert.equal(f.env.networkModeSwitching, false);
  assert.deepEqual(f.observed(), { mode: "tun", phase: "running" });
});

test("readback errors release the mutation lock without retrying or stopping the accepted core", async () => {
  const f = mainFixture({ refreshError: failure("readback failed") });
  await f.switch();
  assert.deepEqual(f.mutations(), ["stop", "mode:tun", "start:tun"]);
  assert.equal(f.issues.at(-1).details, "readback failed");
  assert.equal(f.env.networkModeSwitching, false);
});

test("final authoritative crash readback suppresses success without stopping or restarting another session", async () => {
  const f = mainFixture({ refreshedPhase: "crashed" });
  await f.switch();
  assert.equal(f.env.store.runtime.phase, "crashed");
  assert.deepEqual(f.mutations(), ["stop", "mode:tun", "start:tun"]);
  assert.equal(f.toasts.filter(item => item.tone === "success").length, 0, "a stale accepted start is not final enablement");
  assert.ok(f.issues.length > 0, "readback mismatch should explain that the runtime was not confirmed");
});

test("an old failed backend feedback snapshot cannot erase the current TUN preflight error", async () => {
  const f = mainFixture({
    actualPhase: "crashed",
    prepareError: failure("current preflight failure"),
    feedbackSnapshot: {
      revision: 8, operation: 3, phase: "failed", mode: "system_proxy", health: "unchecked",
      retrying: false, elapsedMs: 0, checks: [],
      issue: { title: "上次启动错误", description: "旧失败说明", details: "old error", action: "retry" },
    },
  });
  await f.switch();
  assert.deepEqual(f.mutations(), []);
  assert.equal(f.feedback.text("title"), "TUN 预检尚未通过");
  assert.match(f.feedback.text("description"), /尚未切换网络模式/);
  assert.match(f.feedback.text("detail-text"), /current preflight failure/);
});

function settingsFixture(options = {}) {
  const calls = [], issues = [], handlers = new Map(), elements = new Map();
  let persisted = {
    networkMode: options.previousMode ?? "system_proxy", mixedPort: 7890, controllerPort: 9090,
    diagnosticsRetentionDays: 3, appLogRetentionDays: 3, theme: "system",
  };
  const values = {
    "#settings-mode": "tun", "#settings-mixed-port": "7890", "#settings-controller-port": "9090",
    "#settings-retention": "3", "#settings-app-log-retention": "3", ...options.values,
  };
  const env = {
    store: { settings: { ...persisted }, runtime: runtime(options.phase ?? "stopped") },
    settingsSaving: false, runtimeActionInFlight: false, networkModeSwitching: false,
    runtimeMutationRevision: 0, sessionResumeRevision: 0, startupStatus: null, startupModeDraft: null,
    themeController: { snapshot: { saving: false, preference: "system" }, refresh: noop },
    $: selector => {
      if (!elements.has(selector)) elements.set(selector, {
        value: values[selector], checked: false, disabled: false,
        addEventListener: (event, callback) => handlers.set(`${selector}:${event}`, callback),
      });
      return elements.get(selector);
    },
    startupModeFromSettings: () => "manual",
    startupModeSettings: settings => settings,
    ensureTunHelperReady: async () => { calls.push("preflight"); return options.preflightReady !== false; },
    action: async (_label, operation) => {
      try { return await operation(); }
      catch (error) { issues.push(error); return null; }
    },
    api: {
      // This mutator exposes an accidental partial-mode write rather than silently mocking it away.
      setNetworkMode: async mode => { calls.push(`mode:${mode}`); persisted.networkMode = mode; return { ...persisted }; },
      updateSettings: async settings => {
        calls.push("update-settings");
        assert.equal(env.settingsSaving, true);
        if (settings.mixedPort === settings.controllerPort) throw failure("ports conflict");
        if (env.store.runtime.phase === "running" && settings.networkMode !== persisted.networkMode) throw failure("stop before changing network settings");
        if (options.updateError) throw options.updateError;
        persisted = { ...settings };
        return { ...persisted };
      },
      stop: async () => { calls.push("stop"); },
      startActive: async () => { calls.push("start"); },
    },
    renderSettings: noop, renderOverview: noop, renderGlobalTraffic: noop, renderTunHelper: noop,
    refreshSessionResume: async () => {},
  };
  const start = main.indexOf('$("#settings-form")!.addEventListener("submit"');
  const end = main.indexOf("\nwindow.setInterval(", start);
  assert.ok(start >= 0 && end > start);
  vm.createContext(env);
  vm.runInContext(javascript(main.slice(start, end)), env);
  return {
    env, calls, issues,
    persisted: () => persisted,
    submit: () => handlers.get("#settings-form:submit")({ preventDefault: noop }),
  };
}

test("invalid complete settings never persist a TUN mode before the new ports are validated", async () => {
  const f = settingsFixture({ values: { "#settings-controller-port": "7890" } });
  await f.submit();
  assert.deepEqual(f.calls, ["preflight", "update-settings"]);
  assert.equal(f.persisted().networkMode, "system_proxy");
  assert.equal(f.env.store.settings.networkMode, "system_proxy");
  assert.equal(f.persisted().controllerPort, 9090);
  assert.equal(f.issues[0].userMessage.details, "ports conflict");
  assert.equal(f.env.settingsSaving, false);
});

test("valid settings use one atomic update and never start or stop a proxy implicitly", async () => {
  const f = settingsFixture();
  await f.submit();
  assert.deepEqual(f.calls, ["preflight", "update-settings"]);
  assert.equal(f.persisted().networkMode, "tun");
  assert.equal(f.env.store.settings.networkMode, "tun");
  assert.equal(f.env.store.runtime.phase, "stopped");
  assert.equal(f.issues.length, 0);
  assert.equal(f.env.settingsSaving, false);
});

test("settings preflight or atomic-save failures leave the existing mode unchanged", async () => {
  const blocked = settingsFixture({ preflightReady: false });
  await blocked.submit();
  assert.deepEqual(blocked.calls, ["preflight"]);
  assert.equal(blocked.persisted().networkMode, "system_proxy");
  assert.equal(blocked.env.settingsSaving, false);
  const failed = settingsFixture({ updateError: failure("save failed") });
  await failed.submit();
  assert.deepEqual(failed.calls, ["preflight", "update-settings"]);
  assert.equal(failed.persisted().networkMode, "system_proxy");
  assert.equal(failed.env.store.settings.networkMode, "system_proxy");
  assert.equal(failed.env.settingsSaving, false);
});

test("running settings changes remain rejected atomically instead of silently stopping the original session", async () => {
  const f = settingsFixture({ phase: "running" });
  await f.submit();
  assert.deepEqual(f.calls, ["preflight", "update-settings"]);
  assert.equal(f.persisted().networkMode, "system_proxy");
  assert.equal(f.env.store.runtime.phase, "running");
  assert.equal(f.issues[0].userMessage.details, "stop before changing network settings");
});

function helperManagementFixture(options = {}) {
  const calls = [], issues = [], handlers = new Map(), elements = new Map();
  const env = {
    store: {
      appInfo: { targetOs: options.platform ?? "macos" },
      tunHelper: helper(options.helperState ?? "unreachable"),
      runtime: runtime(options.phase ?? "stopped"),
    },
    networkModeSwitching: false, runtimeActionInFlight: false, settingsSaving: false,
    runtimeMutationRevision: 0, ...options.busy,
    canStartRuntime,
    $: selector => {
      if (!elements.has(selector)) elements.set(selector, {
        disabled: false, classList: { toggle: noop },
        addEventListener: (event, callback) => handlers.set(`${selector}:${event}`, callback),
      });
      return elements.get(selector);
    },
    renderHeader: noop, renderOverview: noop,
    refreshBase: async () => { calls.push("refresh"); },
    connectionFeedback: { showError: error => issues.push(error) },
    action: async (_message, operation) => {
      try { return await operation(); }
      catch (error) { issues.push(error); return null; }
    },
    confirmAction: async () => {
      calls.push("confirm");
      if (options.confirmWait) await options.confirmWait;
      return options.confirmed !== false;
    },
    api: Object.fromEntries(["installTunHelper", "repairTunHelper", "uninstallTunHelper", "openTunHelperSettings"].map(name => [name, async () => {
      calls.push(name);
      if (options.operationWait) await options.operationWait;
      if (options.operationError) throw options.operationError;
      return helper();
    }])),
  };
  env.api.tunHelperStatus = async () => {
    calls.push("status");
    assert.equal(env.runtimeActionInFlight, true, "helper operation lock remains held through status readback");
    if (options.statusWait) await options.statusWait;
    if (options.statusError) throw options.statusError;
    return helper();
  };
  const renderStart = main.indexOf("function tunHelperStateLabel(");
  const renderEnd = main.indexOf("const systemAppearance =", renderStart);
  const manageStart = main.indexOf("async function manageTunHelper(");
  const manageEnd = main.indexOf('$("#app-update-check")!', manageStart);
  assert.ok(renderStart >= 0 && renderEnd > renderStart && manageStart >= 0 && manageEnd > manageStart);
  vm.createContext(env);
  vm.runInContext(javascript(`${main.slice(renderStart, renderEnd)}\n${main.slice(manageStart, manageEnd)}`), env);
  return {
    env, calls, issues, elements,
    render: () => env.renderTunHelper(),
    click: name => handlers.get(`#tun-helper-${name}:click`)({ currentTarget: elements.get(`#tun-helper-${name}`) }),
  };
}

test("Helper mutation controls are disabled for every active mutation and runtime transition", () => {
  for (const busy of [{ runtimeActionInFlight: true }, { networkModeSwitching: true }, { settingsSaving: true }]) {
    const f = helperManagementFixture({ busy });
    f.render();
    for (const name of ["install", "repair", "uninstall"]) assert.equal(f.elements.get(`#tun-helper-${name}`).disabled, true, name);
  }
  for (const phase of ["running", "starting", "validating", "stopping", "recovering"]) {
    const f = helperManagementFixture({ phase });
    f.render();
    for (const name of ["install", "repair", "uninstall"]) assert.equal(f.elements.get(`#tun-helper-${name}`).disabled, true, `${name} during ${phase}`);
  }
});

test("Helper event handlers recheck mutation gates even if a stale enabled control is invoked", async () => {
  for (const options of [
    { busy: { runtimeActionInFlight: true } }, { busy: { networkModeSwitching: true } },
    { busy: { settingsSaving: true } }, { phase: "starting" }, { phase: "running" },
  ]) {
    const f = helperManagementFixture(options);
    for (const name of ["install", "repair", "uninstall"]) await f.click(name);
    assert.deepEqual(f.calls, []);
  }
});

test("Windows, Linux and unknown platforms never dispatch native macOS Helper actions", async () => {
  for (const platform of ["windows", "linux", "unknown"]) {
    const f = helperManagementFixture({ platform });
    for (const name of ["install", "repair", "uninstall", "open-settings"]) await f.click(name);
    assert.deepEqual(f.calls, []);
  }
});

test("Helper management is single-flight through the operation and authoritative status readback", async () => {
  const operation = deferred(), status = deferred();
  const f = helperManagementFixture({ operationWait: operation.promise, statusWait: status.promise });
  const pending = f.click("repair");
  await tick();
  assert.equal(f.env.runtimeActionInFlight, true);
  await f.click("repair");
  await f.click("install");
  await f.click("uninstall");
  assert.deepEqual(f.calls, ["repairTunHelper"]);
  operation.resolve();
  await tick();
  assert.deepEqual(f.calls, ["repairTunHelper", "status"]);
  await f.click("repair");
  assert.equal(f.env.runtimeActionInFlight, true);
  status.resolve();
  await pending;
  assert.deepEqual(f.calls, ["repairTunHelper", "status", "refresh"]);
  assert.equal(f.env.runtimeActionInFlight, false);
  assert.equal(f.env.store.tunHelper.state, "ready");
});

test("uninstall confirmation is not authority to mutate a session that started while the dialog was open", async () => {
  for (const mutation of [
    env => { env.runtimeActionInFlight = true; },
    env => { env.store.runtime = runtime("running"); },
    env => { env.settingsSaving = true; },
  ]) {
    const confirmation = deferred();
    const f = helperManagementFixture({ confirmWait: confirmation.promise });
    const pending = f.click("uninstall");
    await tick();
    assert.deepEqual(f.calls, ["confirm"]);
    mutation(f.env);
    confirmation.resolve();
    await pending;
    assert.deepEqual(f.calls, ["confirm"]);
  }
});

test("idle confirmed uninstall runs once and declining confirmation has no privileged side effects", async () => {
  const declined = helperManagementFixture({ confirmed: false });
  await declined.click("uninstall");
  assert.deepEqual(declined.calls, ["confirm"]);
  const accepted = helperManagementFixture();
  await accepted.click("uninstall");
  assert.deepEqual(accepted.calls, ["confirm", "uninstallTunHelper", "status", "refresh"]);
  assert.equal(accepted.env.runtimeActionInFlight, false);
});

test("Helper operation or readback failure releases the mutation lock without retrying registration", async () => {
  for (const option of ["operationError", "statusError"]) {
    const f = helperManagementFixture({ [option]: failure(`${option} fixture`) });
    await f.click("repair");
    assert.equal(f.calls.filter(call => call === "repairTunHelper").length, 1);
    assert.equal(f.issues[0].userMessage.details, `${option} fixture`);
    assert.equal(f.env.runtimeActionInFlight, false);
  }
});
