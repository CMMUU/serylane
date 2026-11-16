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
const { networkToggleIntent, canStartRuntime } = await import(moduleUrl(read("runtime-start.ts")));

function toolbarFixture({ submit, settle } = {}) {
  const writes = [], notices = [], refreshes = [];
  const noop = () => {};
  const e = {
    store: { settings: { networkMode: "tun" }, activeProfile: {}, runtime: { phase: "stopped" } },
    proxyControlSnapshot: null, proxyStopIssue: "", runtimeMutationRevision: 0,
    settingsSaving:false, manualModeSwitching:false, domainRouteChanging:false, networkModeSwitching:false, runtimeActionInFlight:false, runtimeActivity:null,
    proxyControlsPending: () => Boolean(e.proxyControlSnapshot?.busy),
    networkToggleIntent, currentProxyPresentation: () => e.proxyControlSnapshot?.network ?? { systemProxyActive:false, tunActive:false },
    acceptProxyControl: snapshot => { e.proxyControlSnapshot = snapshot; },
    renderHeader:noop,renderOverview:noop,renderAppearance:noop,themeController:{snapshot:{}},
    connectionFeedback:{ clearIssue:noop, showError: v => notices.push(v) }, friendlyError: error => ({ title:error.message,action:"refresh" }),
    errorMessage:error=>error.message, navigate:noop,
    api:{
      settings:async()=>e.store.settings,
      proxyControlAction: async (request) => { writes.push({ ...request }); return submit ? await submit(request) : null; },
    },
    refreshRuntimeOnly: async (forced) => { assert.equal(forced,true);assert.equal(e.runtimeActionInFlight,true);refreshes.push("core");if(settle)await settle; },
    localRouting:{invalidate:noop,refresh:async()=>refreshes.push("route")},
    refreshSessionResume:async()=>{},refreshBase:async()=>{},refreshConnectionFeedback:async()=>{},
  };
  vm.createContext(e);
  vm.runInContext(ts.transpileModule(main.slice(main.indexOf("async function runProxyControl("),main.indexOf("async function ensureTunHelperReady(")),{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText,e);
  return {e,writes,notices,refreshes};
}

test("main Start submits saved-mode intent; explicit TUN is never substituted", async () => {
  assert.match(main, /#global-start.*addEventListener\("click", \(\) => void startRuntime\("previous"\)\)/);
  assert.match(main, /id="global-start"[^>]*aria-label="按上次方案启动"/);
  const f=toolbarFixture();
  await f.e.startRuntime("previous");
  await f.e.startRuntime("tun");
  assert.deepEqual(f.writes,[{action:"start",mode:undefined},{action:"start",mode:"tun"}]);
  assert.equal(f.e.store.settings.networkMode,"tun");
});

test("toolbar and overview send one absolute, mode-specific intent", async () => {
  assert.deepEqual(networkToggleIntent("system_proxy",false),{action:"switch",mode:"system_proxy"});
  assert.deepEqual(networkToggleIntent("tun",true),{action:"stop",mode:"tun"});
  const f=toolbarFixture();
  f.e.proxyControlSnapshot={network:{systemProxyActive:true}};
  f.e.toggleGlobalNetworkMode("system_proxy");
  await new Promise(resolve=>setImmediate(resolve));
  await f.e.switchNetworkMode("tun");
  await f.e.stopRuntime();
  await f.e.runProxyControl({action:"refresh"});
  assert.deepEqual(f.writes,[{action:"stop",mode:"system_proxy"},{action:"switch",mode:"tun"},{action:"stop"},{action:"refresh"}]);
});

test("the UI holds its lock through IPC and readback without replay or a second core command", async () => {
  let ready, settled;
  const result=new Promise(resolve=>{ready=resolve;});
  const settle=new Promise(resolve=>{settled=resolve;});
  const f=toolbarFixture({submit:()=>result,settle});
  const first=f.e.startRuntime("previous");
  await f.e.stopRuntime();
  assert.equal(f.writes.length,1);
  ready(null);
  await new Promise(resolve=>setImmediate(resolve));
  await f.e.switchNetworkMode("tun");
  assert.equal(f.writes.length,1);
  settled(); await first;
  assert.equal(f.e.runtimeActionInFlight,false);
  assert.deepEqual(f.refreshes,["core","route"]);
});

test("a macOS panel write disables the other surface, and TUN failures are not replayed", async () => {
  const f=toolbarFixture({submit:()=>{throw Error("TUN 预检未通过");}});
  f.e.proxyControlSnapshot={busy:"switching"};
  await f.e.stopRuntime(); assert.equal(f.writes.length,0);
  f.e.proxyControlSnapshot=null;
  await f.e.switchNetworkMode("tun");
  assert.deepEqual(f.writes,[{action:"switch",mode:"tun"}]);
  assert.equal(f.notices[0].userMessage.title,"TUN 预检未通过");
  assert.equal(f.e.runtimeActionInFlight,false);
});

test("a transitional or live runtime has no standalone Start affordance", () => {
  for (const phase of ["running","starting","validating","stopping","recovering"]) assert.equal(canStartRuntime({phase}),false);
  for (const phase of ["stopped","crashed","uninitialized"]) assert.equal(canStartRuntime({phase}),true);
});

test("a successful IPC with an unverified readback is displayed as pending, not forced active", async () => {
  const observed={serial:2,busy:null,network:{state:"checking",label:"系统代理待确认",systemProxyActive:false}};
  const f=toolbarFixture({submit:async()=>observed});
  await f.e.startRuntime("system_proxy");
  assert.equal(f.e.proxyControlSnapshot.network.state,"checking");
  assert.equal(f.e.proxyControlSnapshot.network.systemProxyActive,false);
  assert.equal(f.writes.length,1);
});

test("refreshBase rejects a delayed pre-start snapshot after a network mutation", async () => {
  let resolveSettings;
  const settingsRead = new Promise(resolve => { resolveSettings = resolve; });
  const staleRuntime = { phase: "stopped" };
  const noop = () => {};
  const environment = {
    themeController: { mutationRevision: 0, sync: () => true },
    runtimeMutationRevision: 0, baseReadSequence: 0, domainRouteChanging: false, runtimeActionInFlight: false, networkModeSwitching: false, settingsSaving: false, manualModeSwitching: false,
    store: { settings: { networkMode: "manual" }, runtime: staleRuntime },
    action: async (_label, operation) => operation(),
    api: new Proxy({ settings: () => settingsRead, runtime: async () => staleRuntime }, { get: (target, key) => target[key] ?? (async () => null) }),
    invalidateNodeDetailsContext: noop, localRouting: { refresh: async () => {} }, renderTunHelper: noop, renderHeader: noop, renderOverview: noop, renderProfiles: noop, renderSubscriptions: noop, renderSettings: noop,
    renderOpenAiPolicy: noop, renderGlobalTraffic: noop, scheduleAutomaticUpdateCheck: noop,
  };
  const source = main.slice(main.indexOf("async function performBaseRead()"), main.indexOf("function renderHeader()"));
  vm.createContext(environment);
  vm.runInContext(ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText, environment);
  const pending = environment.performBaseRead();
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
  const duringWrite = environment.performBaseRead();
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
    proxyControlSnapshot: null, staleNetworkProjectionSerial: 0, runtimeMutationRevision: 0, runtimeReadSequence: 0, proxyReadSequence: 0, overviewNodeDetails: {}, renderProxies: noop,
    domainRouteChanging: false, runtimeActionInFlight: false, networkModeSwitching: false, settingsSaving: false, manualModeSwitching: false,
    store: { runtime: { phase: "stopped" }, systemProxy: { active: false } },
    api: {
      runtime: () => { reads++; activeRequest = requests.shift(); return activeRequest.promise; },
      systemProxy: async () => ({ active: activeRequest.proxyActive }),
      tunHelperStatus: async () => ({ state: "ready" }),
    },
    invalidateNodeDetailsContext: noop, localRouting: { refresh: async () => {} }, renderTunHelper: noop, renderHeader: noop, renderOverview: noop, renderSubscriptions: noop, refreshProxyControls: noop, refreshConnectionFeedback: async () => {},
  };
  const source = main.slice(main.indexOf("async function performRuntimeRead("), main.indexOf("async function startRuntime("));
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
  const beforeStart = f.environment.performRuntimeRead();
  f.environment.runtimeActionInFlight = true;
  f.environment.runtimeMutationRevision++;
  await f.environment.performRuntimeRead();
  assert.equal(f.reads(), 1, "busy timer does not dispatch a stale runtime request");
  const completeFinalRead = f.queue("running", true);
  const finalRead = f.environment.performRuntimeRead(true);
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
  const earlier = f.environment.performRuntimeRead(true);
  const completeFinalRead = f.queue("running", true);
  const final = f.environment.performRuntimeRead(true);
  completeFinalRead();
  await final;
  completeEarlierRead();
  await earlier;
  assert.equal(f.environment.store.runtime.phase, "running");
  const completeLateRead = f.queue("stopped", false);
  const late = f.environment.performRuntimeRead(true);
  f.environment.runtimeActionInFlight = false;
  f.environment.runtimeMutationRevision++;
  completeLateRead();
  await late;
  assert.equal(f.environment.store.runtime.phase, "running");
  assert.equal(f.environment.store.systemProxy.active, true);
});

test("eleven visible routes retain unique labels and icons while program route is parked", () => {
  assert.deepEqual(NAV_ITEMS.map(({ id }) => id), ["overview", "profiles", "subscriptions", "proxies", "manual", "programs", "routing", "rules", "connections", "logs", "diagnostics", "settings"]);
  assert.equal(new Set(NAV_ITEMS.map(({ id }) => id)).size, 12);
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

test("fresh core crash readback invalidates an older active shared projection", async () => {
  const f = runtimeRefreshFixture();
  f.environment.proxyControlSnapshot = { serial: 6, network: { active: true, systemProxyActive: true } };
  const complete = f.queue("crashed", false);
  const pending = f.environment.performRuntimeRead();
  complete();
  await pending;
  assert.equal(f.environment.staleNetworkProjectionSerial, 6);
  assert.equal(f.environment.store.runtime.phase, "crashed");
});
