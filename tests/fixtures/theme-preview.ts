/**
 * Vite-only visual fixture. Uses the real frontend with Tauri's installed IPC
 * mocks, not a running desktop app. All profile/runtime data below is synthetic.
 * No call is forwarded to a native bridge, filesystem, core, or subscription.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import packageInfo from "../../package.json";
import type { InvokeArgs } from "@tauri-apps/api/core";
import type {
  ConnectionFeedback, AppSettings, AppUpdateStatus, UpdateSource, ProfileDetails, ProfileRecord,
  UserRule, UserRulesState, UserRulesValidation,
  InstalledApplication, AppAvailability, ProgramInput, ProgramState, RouteSettings, RouteSnapshot,
  SubscriptionMetadata, SubscriptionOverview, SubscriptionStatus, NetworkMode, OpenAiPolicyTask,
} from "../../src/types";
import type { ThemePreference } from "../../src/theme";
import type { ProxyMap, NodeMetadata } from "../../src/node-selection";
import type { CostSnapshot, CostInput } from "../../src/openai-costs";

const STORAGE_KEY = "routedeck:test-fixture:theme-preview:v1";
const RULES_STORAGE_KEY = "routedeck:test-fixture:user-rules:v1";
const previewQuery = new URLSearchParams(location.search);
const previewPlatform = ["windows", "macos", "linux"].includes(previewQuery.get("platform") ?? "") ? previewQuery.get("platform")! : "macos";
const previewWindows = previewPlatform === "windows";
const PROGRAMS_STORAGE_KEY = `routedeck:test-fixture:proxy-programs:v1:${previewPlatform}`;
// Screenshot-only layout; synthetic version labels and the fail-closed bridge remain.
if (previewQuery.get("presentation") === "1" || previewQuery.get("glass") === "1") {
  document.documentElement.dataset.fixturePresentation = "true";
}
document.documentElement.dataset.fixtureGlass = String(previewQuery.get("glass") === "1");
const THEMES: readonly ThemePreference[] = ["system", "light", "dark", "purple"];
type FixtureState = { theme: ThemePreference; systemDark: boolean };

function readFixtureState(): FixtureState {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
    if (value && THEMES.includes(value.theme) && typeof value.systemDark === "boolean") {
      return { theme: value.theme, systemDark: value.systemDark };
    }
  } catch {
    // A malformed fixture value never causes access to actual application data.
  }
  return { theme: "system", systemDark: false };
}

let persisted = readFixtureState();
let failNextThemeSave = false;
let themeSaveCount = 0;
let networkMutationCount = 0;
let blockedBrowserRequestCount = 0;
let runtimeErrorCount = 0;
let failNextRulesSave = false;
let rulesSaveCount = 0;
let rulesRollbackCount = 0;
let rulesApplyCount = 0;

let programs: ProgramState = { platform: previewPlatform, revision: 0, supported: true, proxyEndpoint: "http://127.0.0.1:17890", coreRunning: true, programs: [] };
try {
  const saved = JSON.parse(localStorage.getItem(PROGRAMS_STORAGE_KEY) ?? "null") as ProgramState | null;
  if (saved && Array.isArray(saved.programs)) programs = { ...programs, revision: saved.revision, programs: saved.programs.map((program) => ({ ...program, runningPid: null, launchPending: false })) };
} catch { /* Isolated fixture only; never read actual application data. */ }
let installedAppVersion = "1.0.0.0";
let installedAppStatus: AppAvailability = "ready";
function fixtureInstalledApplication(): InstalledApplication {
  const detail = installedAppStatus === "ready" ? "已关联应用，启动前重新定位当前安装版本。" : installedAppStatus === "updating" ? "应用正在更新，完成后请刷新重试。" : "应用信息待重新关联；原代理参数已保留。";
  if (previewPlatform === "macos") return {
    binding: { kind: "macos", bundleId: "invalid.fixture.Codex", location: "/Applications/Fixture Codex.app", requirement: 'identifier "invalid.fixture.Codex"' },
    name: "Codex（合成应用）", version: installedAppVersion, packageFullName: "invalid.fixture.Codex", packageRoot: "/Applications/Fixture Codex.app",
    executable: "/Applications/Fixture Codex.app/Contents/MacOS/Codex", availability: installedAppStatus, detail,
  };
  if (previewPlatform === "linux") return {
    binding: { kind: "linux", desktopId: "invalid.fixture.Codex.desktop", location: "/usr/share/applications/invalid.fixture.Codex.desktop" },
    name: "Codex（合成应用）", version: "", packageFullName: "invalid.fixture.Codex.desktop", packageRoot: "/usr/share/applications",
    executable: "/opt/fixture-codex/codex", availability: installedAppStatus, detail,
  };
  return { binding: { packageFamilyName: "Fixture.Codex_123456789abcd", applicationId: "App" }, name: "Codex（合成应用）", version: installedAppVersion,
    packageFullName: `Fixture.Codex_${installedAppVersion}_x64__123456789abcd`, packageRoot: `C:\\Program Files\\WindowsApps\\Fixture.Codex_${installedAppVersion}`,
    executable: `C:\\Program Files\\WindowsApps\\Fixture.Codex_${installedAppVersion}\\app\\Codex.exe`, availability: installedAppStatus,
    detail: installedAppStatus === "ready" ? "已关联应用，更新后自动定位当前安装版本。" : installedAppStatus === "updating" ? "Windows 正在更新此应用，完成后请刷新重试。" : "应用信息待重新关联；原代理参数已保留。" };
}
let catalogDelayMs = 0;
let catalogFail = false;
let catalogCheckedAt: number | null = null;
let catalogReadyAt: number | null = null;
let catalogCached: InstalledApplication[] = [];
let catalogReadCount = 0;
let catalogForceCount = 0;
let programPickerPath: string | null = null;
function fixtureApplications() {
  const ready = fixtureInstalledApplication();
  return [ready, { ...ready, name: "特殊激活入口（合成应用）", availability: "unsupported_launch" as const,
    detail: "此入口需要特殊激活，暂未适配代理启动。", version: "", binding: previewPlatform === "macos"
      ? { kind: "macos" as const, bundleId: "invalid.fixture.Hosted", location: "/Applications/Fixture Hosted.app", requirement: null }
      : previewPlatform === "linux" ? { kind: "linux" as const, desktopId: "invalid.fixture.Hosted.desktop", location: "/usr/share/applications/invalid.fixture.Hosted.desktop" }
      : { packageFamilyName: "Fixture.Hosted_123456789abcd", applicationId: "App" } }];
}
window.addEventListener("serylane-fixture-app-binding", (event) => {
  const detail = (event as CustomEvent<{ version?: string; status?: AppAvailability; catalogDelayMs?: number; catalogFail?: boolean; pickerPath?: string | null }>).detail;
  installedAppVersion = detail.version ?? installedAppVersion;
  installedAppStatus = detail.status ?? installedAppStatus;
  catalogDelayMs = detail.catalogDelayMs ?? catalogDelayMs;
  catalogFail = detail.catalogFail ?? catalogFail;
  if (detail.pickerPath !== undefined) programPickerPath = detail.pickerPath;
});
function fixtureCatalog(refresh: boolean) {
  catalogReadCount++;
  if (refresh) catalogForceCount++;
  document.documentElement.dataset.fixtureCatalogReads = String(catalogReadCount);
  document.documentElement.dataset.fixtureCatalogForces = String(catalogForceCount);
  if (catalogFail) throw new Error("合成应用目录读取失败");
  if (catalogReadyAt === null && (refresh || catalogCheckedAt === null)) catalogReadyAt = Date.now() + catalogDelayMs;
  if (catalogReadyAt !== null && Date.now() >= catalogReadyAt) {
    catalogCached = fixtureApplications(); catalogCheckedAt = Date.now(); catalogReadyAt = null;
  }
  return { applications: structuredClone(catalogCached), warnings: [], refreshing: catalogReadyAt !== null, checkedAt: catalogCheckedAt };
}
let programLaunches = 0;
let failProgramSave = false;
let cancelProgramPicker = false;
let updatePreferences = { updateSource: "auto" as UpdateSource, autoCheckUpdates: false, autoDownloadUpdates: false };
let fixtureUpdate: AppUpdateStatus = { phase: "idle", info: null, downloadedBytes: 0, totalBytes: 0, error: null };
let updateScenario = "available";
let cancelUpdateDownload = false;
let updateInstallCount = 0;
let appearanceSettings = { launchAtLogin: false, silentStartup: true, restoreLastSession: true, showGlobalTraffic: true, diagnosticsRetentionDays: 7, appLogRetentionDays: 3 };
let fixtureResumeStatus = { phase: "idle", message: "合成状态：上次核心已停止，保持停止。未读取真实应用数据。" };
window.addEventListener("routedeck-fixture-resume", event => {
  const detail = (event as CustomEvent).detail;
  if (["idle", "pending", "restoring", "waiting_network", "restored", "paused"].includes(detail.phase)) fixtureResumeStatus = { phase: detail.phase, message: String(detail.message) };
});
let settingsSaveCount = 0;
let fixtureStartupRegistered: boolean | null = false;
let fixtureStartupAllowed: boolean | null = null;
let fixtureDesiredRunning = false;
let fixtureSettingsFailSave = false;
function reportStartup() {
  document.documentElement.dataset.fixtureStartupState = JSON.stringify({
    ...appearanceSettings, registered: fixtureStartupRegistered, systemAllows: fixtureStartupAllowed, desiredRunning: fixtureDesiredRunning,
  });
}
window.addEventListener("routedeck-fixture-startup", event => {
  const detail = (event as CustomEvent).detail;
  for (const key of ["launchAtLogin", "silentStartup", "restoreLastSession"] as const) {
    if (typeof detail[key] === "boolean") appearanceSettings[key] = detail[key];
  }
  if (typeof detail.registered === "boolean" || detail.registered === null) fixtureStartupRegistered = detail.registered;
  if (typeof detail.systemAllows === "boolean" || detail.systemAllows === null) fixtureStartupAllowed = detail.systemAllows;
  if (typeof detail.desiredRunning === "boolean") fixtureDesiredRunning = detail.desiredRunning;
  if (typeof detail.failSave === "boolean") fixtureSettingsFailSave = detail.failSave;
  reportStartup();
});
reportStartup();
let applicationLogReads = 0;
let applicationLogEntries = [
  { timestamp: Date.now(), level: "warn", source: "网络预检", message: "演示事件：连接超时；未接管系统代理。此行不是实际网络故障。", count: 2 },
  { timestamp: Date.now() - 1000, level: "info", source: "状态恢复", message: "演示事件：上次已停止，保持停止。", count: 1 },
];

// Explicitly opt in to a wholly in-memory toolbar-start scenario. The default
// visual fixture still rejects core/network mutations; no IPC is forwarded.
let runtimeScenarioEnabled = false;
let fixtureRuntimeMode: NetworkMode = "manual";
let fixtureRuntimePhase = "running";
let fixtureSystemProxyActive = false;
let fixtureRuntimeFailSave = false, fixtureRuntimeFailStart = false, fixtureRuntimeFailRollback = false;
let fixtureRuntimeDelay = 120;
let fixtureRuntimeHelperReady = true;
let fixtureRuntimeHelperState: string | null = null;
let fixtureRuntimeProxyConfirmed = true, fixtureRuntimeCrashAfterStart = false;
const fixtureRuntimeCalls: { command: string; mode?: NetworkMode }[] = [];
function reportRuntimeScenario() {
  document.documentElement.dataset.fixtureRuntimeState = JSON.stringify({ enabled: runtimeScenarioEnabled, networkMode: fixtureRuntimeMode, phase: fixtureRuntimePhase, systemProxyActive: fixtureSystemProxyActive });
  document.documentElement.dataset.fixtureRuntimeCalls = JSON.stringify(fixtureRuntimeCalls);
}
function setRuntimeScenario(detail: Record<string, unknown>) {
  runtimeScenarioEnabled = true;
  if (typeof detail.scenario === "string") {
    fixtureRuntimeMode = detail.scenario === "tun" ? "tun" : detail.scenario === "system_proxy" ? "system_proxy" : "manual";
    fixtureRuntimePhase = detail.scenario === "running" ? "running" : "stopped";
    fixtureSystemProxyActive = false;
    fixtureRuntimeFailSave = detail.scenario === "save-failed";
    fixtureRuntimeFailStart = detail.scenario === "start-failed";
    fixtureRuntimeFailRollback = false;
    fixtureRuntimeHelperReady = true;
    fixtureRuntimeHelperState = null;
    fixtureRuntimeProxyConfirmed = detail.scenario !== "proxy-unconfirmed";
    fixtureRuntimeCrashAfterStart = detail.scenario === "crashed-after-start";
    fixtureRuntimeCalls.length = 0;
  }
  if (typeof detail.failSave === "boolean") fixtureRuntimeFailSave = detail.failSave;
  if (typeof detail.failStart === "boolean") fixtureRuntimeFailStart = detail.failStart;
  if (typeof detail.failRollback === "boolean") fixtureRuntimeFailRollback = detail.failRollback;
  if (typeof detail.helperReady === "boolean") fixtureRuntimeHelperReady = detail.helperReady;
  if (typeof detail.helperState === "string") fixtureRuntimeHelperState = detail.helperState;
  if (typeof detail.proxyActive === "boolean") fixtureRuntimeProxyConfirmed = detail.proxyActive;
  if (typeof detail.crashAfterStart === "boolean") fixtureRuntimeCrashAfterStart = detail.crashAfterStart;
  if (typeof detail.delayMs === "number" && Number.isInteger(detail.delayMs) && detail.delayMs >= 0 && detail.delayMs <= 3000) fixtureRuntimeDelay = detail.delayMs;
  reportRuntimeScenario();
}
window.addEventListener("routedeck-fixture-runtime", event => {
  const detail = (event as CustomEvent<Record<string, unknown>>).detail;
  if (!detail || typeof detail !== "object") return;
  setRuntimeScenario(detail);
  if (detail.refresh === true) document.querySelector<HTMLButtonElement>("#global-refresh")?.click();
});
if (previewQuery.has("runtimeScenario")) setRuntimeScenario({ scenario: previewQuery.get("runtimeScenario") });
reportRuntimeScenario();

// In-memory scenarios only: no persisted Codex config, native IPC or sockets.
// Dispatch `routedeck-fixture-routing` with { scenario, ...options } and refresh
// to exercise the real UI. Counters make cancelled confirmations observable.
function initialRoute(): RouteSnapshot {
  return {
    revision: 1, enabled: false, running: false,
    settings: { listenPort: 15731, mode: "compatible", upstream: "chatgpt", outboundProxy: "" },
    endpoint: "http://127.0.0.1:15731", requests: 0, active: 0, completed: 0, failed: 0,
    lastStatus: 0, lastError: null,
    codex: { configRevision: "fixture-config-1", attached: false, hasBackup: false, provider: "openai", endpoint: null, warning: null, backupPath: null },
    stability: { enabled: false, running: false, eligible: false, profileId: null, revisionId: null, current: null, lastSwitch: null,
      message: "隔离合成状态：尚无托管节点；没有执行网络或模型验证。", nodes: [] },
  };
}
let route = initialRoute();
let routeConfigRevision = 1;
let routeFailRead = false, routeFailSave = false, routeFailStart = false, routeRestoreConflict = false;
const routeCalls = { reads: 0, saves: 0, enables: 0, bindings: 0, stability: 0 };
function reportRoute() {
  document.documentElement.dataset.fixtureRoutingState = JSON.stringify(route);
  document.documentElement.dataset.fixtureRoutingCalls = JSON.stringify(routeCalls);
}
function routeError(code: "INVALID_INPUT" | "STATE_CONFLICT" | "IO_ERROR" | "RUNTIME_ERROR", message: string) {
  return { code, stage: "fixture_local_routing", message: `隔离合成状态：${message}`, retryable: code !== "INVALID_INPUT" };
}
function configRevision() { route.codex.configRevision = `fixture-config-${++routeConfigRevision}`; }
function attachFixtureRoute() {
  route.codex = { ...route.codex, attached: true, hasBackup: true, provider: "routedeck_local",
    endpoint: `${route.endpoint}/v1`, backupPath: "C:\\fixture-only\\codex-config.backup.toml",
    warning: "合成接入配置，仅用于交互预览；未读取或更改真实 Codex 配置，也未验证模型连接。" };
  configRevision();
}
function restoreFixtureRoute() {
  if (routeRestoreConflict) throw routeError("STATE_CONFLICT", "接入字段已被其他程序修改；保留备份并停止恢复，请核对原配置。");
  route.codex = { ...initialRoute().codex, configRevision: route.codex.configRevision };
  configRevision();
}
function routeScenario(detail: Record<string, unknown>) {
  if (typeof detail.scenario === "string") {
    route = initialRoute(); routeConfigRevision = 1;
    routeFailRead = routeFailSave = routeFailStart = routeRestoreConflict = false;
    if (["running", "attached", "active", "restore-conflict"].includes(detail.scenario)) {
      route.enabled = route.running = true;
    }
    if (["attached", "active", "restore-conflict"].includes(detail.scenario)) attachFixtureRoute();
    if (detail.scenario === "active") route.requests = route.active = 1;
    if (detail.scenario === "restore-conflict") {
      routeRestoreConflict = true; route.codex.attached = false;
      route.codex.warning = "隔离合成冲突：其他程序已修改接入字段；恢复时会拒绝覆盖。";
    }
    if (detail.scenario === "read-failed") routeFailRead = true;
    if (detail.scenario === "start-failed") routeFailStart = true;
    if (detail.scenario === "eligible") detail.eligible = true;
    if (detail.scenario === "dual-health") {
      const checkedAt = Math.floor(Date.now() / 1000);
      route.enabled = route.running = true;
      route.requests = 8; route.completed = 6; route.failed = 2; route.lastStatus = 502;
      route.lastDiagnostic = { timestamp: Date.now(), category: "tls", code: "upstream_tls_failed", message: "合成异常：TLS 握手中断；等待复查，未直接归因节点", target: "chatgpt", stage: "request", elapsedMs: 10020, httpStatus: null, attribution: "unconfirmed" };
      route.stability = { ...route.stability, enabled: true, running: true, eligible: true, profileId: "fixture-active", revisionId: "fixture-active-revision", current: "演示 · 日本 01", selectionTarget: "chatgpt", lastCheck: checkedAt, commonFailure: false, message: "合成状态：保持当前合预算出口；仅用于界面测试，没有真实探测。", nodes: [
        { name: "演示 · 日本 01", probeOk: true, successRate: 95, samples: 20, cooldownSeconds: 0, recoveryPasses: 3, modelCompleted: 0, modelInterrupted: 0, chatgpt: { state: "passed", checkedAt, latencyMs: 180 }, openaiApi: { state: "http_unverified", checkedAt, latencyMs: 220 } },
        { name: "演示 · 新加坡 02", probeOk: false, successRate: 70, samples: 10, cooldownSeconds: 240, recoveryPasses: 0, modelCompleted: 0, modelInterrupted: 0, chatgpt: { state: "failed", checkedAt, latencyMs: null }, openaiApi: { state: "passed", checkedAt, latencyMs: 160 } },
      ] };
    }
  }
  for (const key of ["requests", "active", "completed", "failed", "lastStatus"] as const) {
    if (typeof detail[key] === "number" && Number.isSafeInteger(detail[key]) && detail[key] >= 0) route[key] = detail[key];
  }
  if (typeof detail.lastError === "string" || detail.lastError === null) route.lastError = detail.lastError;
  if (typeof detail.failRead === "boolean") routeFailRead = detail.failRead;
  if (typeof detail.failSave === "boolean") routeFailSave = detail.failSave;
  if (typeof detail.failStart === "boolean") routeFailStart = detail.failStart;
  if (typeof detail.restoreConflict === "boolean") routeRestoreConflict = detail.restoreConflict;
  if (detail.externalUpdate) { route.revision++; route.settings.listenPort = 15732; route.endpoint = "http://127.0.0.1:15732"; }
  if (detail.externalConfigUpdate) configRevision();
  if (typeof detail.eligible === "boolean") {
    route.stability.eligible = detail.eligible;
    route.stability.profileId = detail.eligible ? "fixture-active" : null;
    route.stability.revisionId = detail.eligible ? "fixture-active-revision" : null;
    route.stability.message = detail.eligible ? "隔离合成策略可用；尚无模型样本，不代表连接已验证。" : initialRoute().stability.message;
    if (!detail.eligible) route.stability.enabled = route.stability.running = false;
  }
  reportRoute();
}
window.addEventListener("routedeck-fixture-routing", event => {
  const detail = (event as CustomEvent<Record<string, unknown>>).detail;
  if (!detail || typeof detail !== "object") return;
  routeScenario({ ...detail });
  // Opt-in refresh preserves the UI's dirty draft/conflict logic.
  if (detail.refresh === true) document.querySelector<HTMLButtonElement>("#local-route-refresh")?.click();
});
if (previewQuery.has("routeScenario")) routeScenario({ scenario: previewQuery.get("routeScenario") });
reportRoute();
window.addEventListener("routedeck-fixture-update", (event) => { updateScenario = (event as CustomEvent).detail.scenario; });
window.addEventListener("routedeck-fixture-programs", (event) => {
  const detail = (event as CustomEvent).detail;
  failProgramSave = detail.failSave ?? failProgramSave;
  cancelProgramPicker = detail.cancelPicker ?? cancelProgramPicker;
  programs.coreRunning = detail.coreRunning ?? programs.coreRunning;
  if (detail.externalUpdate) {
    programs.revision++;
    if (programs.programs[0]) programs.programs[0].name = "其他窗口更新的名称";
  }
  if (detail.missing && programs.programs[0]) programs.programs[0].available = false;
  if (detail.exited) programs.programs.forEach((program) => { program.runningPid = null; program.launchPending = false; });
  if (typeof detail.launchPending === "boolean") programs.programs.forEach((program) => {
    program.launchPending = detail.launchPending;
    if (detail.launchPending) program.runningPid = null;
  });
  if (typeof detail.launchReceiptPid === "number" && detail.launchReceiptPid > 0) programs.programs.forEach((program) => {
    program.runningPid = detail.launchReceiptPid; program.launchPending = false;
  });
});
function programState() {
  for (const program of programs.programs) {
    if (program.binding) {
      const app = fixtureApplications().find((entry) => JSON.stringify(entry.binding) === JSON.stringify(program.binding)) ?? fixtureInstalledApplication();
      program.executable = app.executable;
      program.available = app.availability === "ready";
      program.resolution = { availability: app.availability, detail: app.detail, application: app };
    }
  }
  return structuredClone(programs);
}
function persistPrograms() {
  localStorage.setItem(PROGRAMS_STORAGE_KEY, JSON.stringify(programs));
  return programState();
}

function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`Missing fixture element: ${id}`);
  return found as T;
}

function report(message: string): void {
  element("fixture-status").textContent = message;
  element("fixture-saved-theme").textContent = persisted.theme;
  element("fixture-system-appearance").textContent = persisted.systemDark ? "dark" : "light";
  element("fixture-theme-save-count").textContent = String(themeSaveCount);
  element("fixture-rules-revision").textContent = String(rulesPersisted.revision);
  element("fixture-rules-save-count").textContent = String(rulesSaveCount);
  element("fixture-rules-rollback-count").textContent = String(rulesRollbackCount);
  element("fixture-rules-apply-count").textContent = String(rulesApplyCount);
  element("fixture-network-mutation-count").textContent = String(networkMutationCount);
  element("fixture-network-mutation-count").dataset.failed = String(networkMutationCount > 0);
  element("fixture-browser-request-count").textContent = String(blockedBrowserRequestCount);
  element("fixture-runtime-error-count").textContent = String(runtimeErrorCount);
  element("fixture-runtime-error-count").dataset.failed = String(runtimeErrorCount > 0);
  element("fixture-fail-save").setAttribute("aria-pressed", String(failNextThemeSave));
  element("fixture-rules-fail-save").setAttribute("aria-pressed", String(failNextRulesSave));
  element("fixture-system-light").setAttribute("aria-pressed", String(!persisted.systemDark));
  element("fixture-system-dark").setAttribute("aria-pressed", String(persisted.systemDark));
}

// Installed @tauri-apps/api 2.11.1 exposes mockIPC and shouldMockEvents. Its
// implementation replaces __TAURI_INTERNALS__.invoke/transformCallback and
// __TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener. No hand-rolled native IPC.
const stamp = "2026-09-03T00:00:00.000Z";
const policy = {
  stabilityEnabled: ["stable", "costs"].includes(previewQuery.get("nodeScenario") ?? ""),
  enabled: true,
  autoMaintain: false,
  maxNodes: 10,
  selectedNodes: [
    { name: "演示东京 01（虚构）", latencyMs: 42, jitterMs: 4, bandwidthMbps: 180, score: 95, checkedAt: stamp },
    { name: "演示新加坡 02（虚构）", latencyMs: 68, jitterMs: 8, bandwidthMbps: 140, score: 86, checkedAt: stamp },
  ],
  candidateCount: 2,
  healthyCount: 2,
  lastBenchmarkedAt: stamp,
  benchmarkVersion: 1,
};
const nodeScenario = previewQuery.get("nodeScenario");
const fixtureMetadata = (index: number): NodeMetadata => ({regions: [index ? "SG" : "JP"], regionStatus: "supported", eligible: true, regionReason: "名称地区在 API 支持名单内", ruleVersion: "2026-09-20.1", checkedAt: "2026-09-20", ruleSource: "https://developers.openai.com/api/docs/supported-countries", nameMultiplier: null, multiplierSource: "manual"});
const fixtureNodes: ProxyMap = {
  "演示节点选择": { type: "Selector", all: policy.selectedNodes.map(node => node.name), now: policy.selectedNodes[0].name, udp: true },
  "演示自动测速": { type: "URLTest", all: policy.selectedNodes.map(node => node.name), now: policy.selectedNodes[0].name, fixed: "", udp: true },
  "GLOBAL": { type: "Selector", all: ["演示节点选择", ...policy.selectedNodes.map(node => node.name)], now: "演示节点选择" },
  "🤖 OpenAI 自动灾备": { type: policy.stabilityEnabled ? "Selector" : "Fallback", all: policy.selectedNodes.map(node => node.name), now: policy.selectedNodes[0].name, udp: true, fixed: "", manualNode: null },
  ...Object.fromEntries(policy.selectedNodes.map((node, index) => [node.name, { type: index === 0 ? "Trojan" : "Shadowsocks", alive: index === 0 ? true : undefined, udp: true, trafficMultiplier: 1, metadata: fixtureMetadata(index) }])),
};
if (nodeScenario === "ordinary") { policy.enabled = false; delete fixtureNodes["🤖 OpenAI 自动灾备"]; }
const nodeCalls: { command: string; group: string; proxy?: string }[] = [];
let fixtureCosts: CostSnapshot | null = null;
let costFailure = false;
const costCalls: CostInput[] = [];
window.addEventListener("serylane-fixture-costs", ((event: CustomEvent) => { if (nodeScenario === "costs") costFailure = Boolean(event.detail?.fail); }) as EventListener);
let failNodeChoice = false, failNodeRead = false;
window.addEventListener("serylane-fixture-nodes", ((event: CustomEvent) => {
  if (!nodeScenario) return;
  failNodeChoice = Boolean(event.detail?.failChoice);
  failNodeRead = Boolean(event.detail?.failRead);
}) as EventListener);
const summary = {
  format: "Clash / Mihomo",
  nodeCount: 2,
  proxyGroupCount: 2,
  proxyProviderCount: 0,
  ruleCount: 3,
  ruleProviderCount: 0,
  dnsConfigured: true,
  tunConfigured: false,
  nodeProtocols: ["trojan", "shadowsocks"],
  proxyGroupTypes: ["select", "fallback"],
  unsupportedGroupTypes: [],
  warnings: ["主题预览合成资料，不代表真实网络或节点测试结果。"],
};
const validation = { valid: true, warnings: [], errors: [], nativeCoreValidated: false };
const metadata = { contentType: "text/yaml", etag: null, lastModified: null, bytes: 8192 };

type RulesFixtureHistory = { id: string; createdAt: string; count: number; rules: UserRule[] };
type RulesFixtureState = { revision: number; rules: UserRule[]; history: RulesFixtureHistory[] };
const fixtureTargets = [
  "DIRECT", "REJECT", "REJECT-DROP", "演示节点选择", "🤖 OpenAI 自动灾备",
  ...policy.selectedNodes.map((node) => node.name),
];
const ruleTypes = new Set([
  "DOMAIN", "DOMAIN-SUFFIX", "DOMAIN-KEYWORD", "DOMAIN-REGEX", "DOMAIN-WILDCARD", "GEOSITE",
  "GEOIP", "SRC-GEOIP", "IP-ASN", "SRC-IP-ASN", "IP-CIDR", "IP-CIDR6", "SRC-IP-CIDR", "IP-SUFFIX", "SRC-IP-SUFFIX",
  "SRC-PORT", "DST-PORT", "IN-PORT", "DSCP", "PROCESS-NAME", "PROCESS-PATH", "PROCESS-NAME-REGEX",
  "PROCESS-PATH-REGEX", "PROCESS-NAME-WILDCARD", "PROCESS-PATH-WILDCARD", "NETWORK", "UID",
  "IN-TYPE", "IN-USER", "IN-NAME", "REMATCH-NAME", "SUB-RULE", "AND", "OR", "NOT", "RULE-SET", "MATCH",
]);
const byteLength = (value: string): number => new TextEncoder().encode(value).length;
const copy = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

function isRuleRecord(value: unknown): value is UserRule {
  if (!value || typeof value !== "object") return false;
  const item = value as Partial<UserRule>;
  return typeof item.id === "string" && typeof item.enabled === "boolean"
    && typeof item.rule === "string" && typeof item.note === "string";
}

function readRulesFixtureState(): RulesFixtureState {
  try {
    const value = JSON.parse(localStorage.getItem(RULES_STORAGE_KEY) ?? "null");
    if (value && Number.isSafeInteger(value.revision) && value.revision >= 0
      && Array.isArray(value.rules) && value.rules.every(isRuleRecord)
      && Array.isArray(value.history) && value.history.every((entry: RulesFixtureHistory) =>
        entry && typeof entry.id === "string" && typeof entry.createdAt === "string"
        && Array.isArray(entry.rules) && entry.rules.every(isRuleRecord))) {
      return { revision: value.revision, rules: value.rules, history: value.history.slice(0, 20) };
    }
  } catch {
    // Read only the synthetic fixture key, never desktop settings or profiles.
  }
  return {
    revision: 1,
    rules: [
      { id: "00000000-0000-4000-8000-000000000001", enabled: true, rule: "DOMAIN-SUFFIX,example.invalid,DIRECT", note: "合成直连示例" },
      { id: "00000000-0000-4000-8000-000000000002", enabled: false, rule: "DOMAIN,blocked.example.invalid,REJECT", note: "合成禁用示例" },
    ],
    history: [{ id: "0", createdAt: stamp, count: 0, rules: [] }],
  };
}

let rulesPersisted = readRulesFixtureState();

function userRulesState(): UserRulesState {
  return copy({
    revision: rulesPersisted.revision,
    rules: rulesPersisted.rules,
    history: rulesPersisted.history.map(({ id, createdAt, rules }) => ({ id, createdAt, count: rules.length })),
    targets: fixtureTargets,
    warnings: ["隔离预览仅执行合成校验与应用，不代表 Mihomo 原生校验结果。"],
    routingMode: "rule",
  });
}

function ruleError(code: "INVALID_INPUT" | "STATE_CONFLICT" | "RUNTIME_ERROR" | "IO_ERROR" | "NOT_FOUND", message: string) {
  return { code, stage: "fixture_user_rules", message, retryable: code !== "INVALID_INPUT" };
}

/** Keep commas inside logical-rule parentheses in one field. */
function ruleFields(source: string): string[] {
  let depth = 0;
  let start = 0;
  const fields: string[] = [];
  for (let index = 0; index < source.length; index += 1) {
    if (source[index] === "(") depth += 1;
    else if (source[index] === ")") depth -= 1;
    if (depth < 0) throw new Error("括号不匹配");
    if (source[index] === "," && depth === 0) {
      fields.push(source.slice(start, index).trim());
      start = index + 1;
    }
  }
  if (depth !== 0) throw new Error("括号不匹配");
  fields.push(source.slice(start).trim());
  return fields;
}

function validateRules(input: unknown): UserRulesValidation {
  const errors: string[] = [];
  const warnings = ["浏览器预览校验为合成实现；实际发布版本还须通过 Mihomo 原生校验。"];
  const normalizedRules: UserRule[] = [];
  const ids = new Set<string>();
  if (!Array.isArray(input)) {
    return { valid: false, errors: ["规则必须是数组"], warnings, normalizedRules, preview: "" };
  }
  if (input.length > 1000) errors.push("规则数量超过 1000 条");
  for (const [index, value] of input.entries()) {
    const prefix = `第 ${index + 1} 条`;
    if (!isRuleRecord(value)) { errors.push(`${prefix}规则字段不完整`); continue; }
    const normalized = { ...value, id: value.id.trim() || crypto.randomUUID(), rule: value.rule.trim(), note: value.note.trim() };
    normalizedRules.push(normalized);
    if (ids.has(normalized.id)) errors.push(`${prefix}规则 ID 重复`);
    ids.add(normalized.id);
    if (normalized.id.length > 128) errors.push(`${prefix}规则 ID 过长`);
    if (byteLength(normalized.rule) > 4096 || byteLength(normalized.note) > 512) errors.push(`${prefix}规则或备注过长`);
    if (!normalized.rule || /[\r\n]/.test(normalized.rule)) { errors.push(`${prefix}必须是单行规则`); continue; }
    try {
      const fields = ruleFields(normalized.rule);
      const kind = fields[0];
      const targetIndex = kind === "MATCH" ? 1 : 2;
      if (!ruleTypes.has(kind)) errors.push(`${prefix}不支持的规则类型：${kind}`);
      if (fields.length <= targetIndex || fields.some((part) => !part)) errors.push(`${prefix}规则缺少必要字段`);
      const target = fields[targetIndex];
      if (target && !fixtureTargets.includes(target)) {
        (normalized.enabled ? errors : warnings).push(`${prefix}目标不存在：${target}`);
      }
      if (fields.slice(targetIndex + 1).some((part) => !["no-resolve", "src"].includes(part))) errors.push(`${prefix}存在不支持的规则参数`);
      if (kind === "MATCH" && normalized.enabled) warnings.push(`${prefix} MATCH 会遮蔽后续规则`);
      if (["IP-CIDR", "SRC-IP-CIDR"].includes(kind)) {
        const [address, mask, extra] = (fields[1] ?? "").split("/");
        const octets = address.split(".");
        if (extra !== undefined || octets.length !== 4 || octets.some((part) => !/^\d{1,3}$/.test(part) || Number(part) > 255)
          || mask === undefined || !/^\d{1,2}$/.test(mask) || Number(mask) > 32) errors.push(`${prefix}IPv4 CIDR 格式不正确`);
      }
      if (kind === "IP-CIDR6" && !/^[\da-f:]+\/(\d{1,3})$/i.test(fields[1] ?? "")) errors.push(`${prefix}IPv6 CIDR 格式不正确`);
    } catch (error) { errors.push(`${prefix}${error instanceof Error ? error.message : String(error)}`); }
  }
  const preview = normalizedRules.filter((rule) => rule.enabled).map((rule) => rule.rule).join("\n");
  return { valid: errors.length === 0, errors, warnings, normalizedRules, preview };
}

function decodeRuleLine(line: string): string {
  if (line.startsWith('"')) {
    const value: unknown = JSON.parse(line);
    if (typeof value !== "string") throw new Error("YAML 规则必须是字符串");
    return value;
  }
  if (line.startsWith("'")) {
    if (!line.endsWith("'")) throw new Error("规则引号不完整");
    return line.slice(1, -1).replace(/''/g, "'");
  }
  return line;
}

function parseRulesText(text: string): UserRule[] {
  if (byteLength(text) > 512 * 1024) throw ruleError("INVALID_INPUT", "规则文本超过 512 KiB");
  let pending: Partial<UserRule> | null = null;
  const result: UserRule[] = [];
  try {
    const trimmed = text.trim();
    if (trimmed === "" || trimmed === "rules: []" || trimmed === "[]") return [];
    for (const original of text.split(/\r?\n/)) {
      const line = original.trim();
      if (!line || line === "rules:" || line === "---") continue;
      if (line.startsWith("# mihomo-codex-rule: ")) {
        pending = JSON.parse(line.slice("# mihomo-codex-rule: ".length));
        if (!pending || typeof pending !== "object") throw new Error("规则元数据格式错误");
        continue;
      }
      if (line.startsWith("#")) continue;
      if (/^[\w-]+\s*:/.test(line)) throw new Error("仅接受 rules 字段，其他 YAML 顶层字段已拒绝");
      const rule = decodeRuleLine(line.startsWith("- ") ? line.slice(2).trim() : line);
      result.push({
        id: typeof pending?.id === "string" ? pending.id : crypto.randomUUID(),
        enabled: typeof pending?.enabled === "boolean" ? pending.enabled : true,
        rule,
        note: typeof pending?.note === "string" ? pending.note : "",
      });
      pending = null;
    }
    if (pending) throw new Error("规则元数据后缺少规则行");
    const validationResult = validateRules(result);
    if (!validationResult.valid) throw new Error(validationResult.errors.join("；"));
    return validationResult.normalizedRules;
  } catch (error) {
    throw ruleError("INVALID_INPUT", error instanceof Error ? error.message : String(error));
  }
}

function persistRules(rules: UserRule[]): UserRulesState {
  const previous = rulesPersisted;
  const next: RulesFixtureState = {
    revision: previous.revision + 1,
    rules: copy(rules),
    history: [{ id: String(previous.revision), createdAt: new Date().toISOString(), count: previous.rules.length, rules: copy(previous.rules) }, ...previous.history].slice(0, 20),
  };
  localStorage.setItem(RULES_STORAGE_KEY, JSON.stringify(next));
  rulesPersisted = next;
  return userRulesState();
}

async function writeRules(args: Record<string, unknown>, rollback: boolean): Promise<UserRulesState> {
  if (rollback) rulesRollbackCount += 1;
  else rulesSaveCount += 1;
  report(rollback ? "正在模拟规则回滚" : "正在模拟规则保存与应用");
  if (args.expectedRevision !== rulesPersisted.revision) throw ruleError("STATE_CONFLICT", "规则已被其他窗口更新，请重新读取后合并修改");
  const target = rollback
    ? rulesPersisted.history.find((entry) => entry.id === args.revisionId)?.rules
    : args.rules;
  if (!target) throw ruleError("INVALID_INPUT", "目标历史版本不存在");
  const result = validateRules(target);
  if (!result.valid) throw ruleError("INVALID_INPUT", result.errors.join("；"));
  await new Promise((resolve) => window.setTimeout(resolve, 180));
  // Check again after the asynchronous boundary, mirroring optimistic locking.
  if (args.expectedRevision !== rulesPersisted.revision) throw ruleError("STATE_CONFLICT", "保存期间规则版本已变化，未覆盖其他修改");
  if (failNextRulesSave) {
    failNextRulesSave = false;
    report("已模拟应用失败；规则、版本、历史与合成生效状态全部保留");
    throw ruleError("RUNTIME_ERROR", "模拟 Mihomo 应用失败；已恢复旧规则与运行配置");
  }
  const next = persistRules(result.normalizedRules);
  rulesApplyCount += 1;
  report(`合成规则已${rollback ? "回滚" : "保存"}为版本 ${next.revision}；真实网络未变化`);
  return next;
}

function makeProfile(id: string, displayName: string, enabled: boolean): ProfileRecord {
  return {
    schemaVersion: 1,
    id,
    displayName,
    source: { type: "remote_subscription", host: "subscription.example.invalid", userAgent: "fixture-only" },
    routingMode: nodeScenario === "global" ? "global" : nodeScenario === "direct" ? "direct" : "rule",
    openaiPolicy: { ...policy, enabled: nodeScenario === "ordinary" ? false : enabled },
    activeRevisionId: `${id}-revision`,
    lastKnownGoodRevisionId: null,
    createdAt: stamp,
    updatedAt: stamp,
  };
}

// Every record is synthetic, including usage and provider error text. Neither
// scenario changes nor the mock actions read a subscription or touch the
// live core, proxy settings, active desktop profile, or native filesystem.
let profiles: ProfileRecord[] = [];
let activeProfileId: string | null = null;
const subscriptionCalls = { reads: 0, refresh: 0, activate: 0, delete: 0, create: 0 };
const subscriptionImportScenarios = new Set(["import", "import-empty", "import-403", "import-duplicate", "import-openai-failed"]);
const subscriptionScenarios = new Set(["default", "cards", "empty", ...subscriptionImportScenarios]);
let subscriptionImportScenario: string | null = null;
let subscriptionScenarioRevision = 0;
let subscriptionImportBusy = false;
let subscriptionImportSequence = 0;
let subscriptionImportResult: unknown = null;
const subscriptionImportCalls: { host: string; activateAfterImport: boolean; generateOpenAi: boolean }[] = [];
// Only reserved .invalid URLs are accepted; even fixture input cannot contact
// a provider or expose a real subscription URL through the diagnostic dataset.
const subscriptionImportUrls = new Map<string, string>();
const idleSubscriptionOpenAiTask = (): OpenAiPolicyTask => ({ running: false, profileId: null, phase: "idle", completed: 0, total: 0, message: "合成预览不执行健康检测", startedAt: null, finishedAt: null, error: null, result: null });
let subscriptionImportOpenAiTask = idleSubscriptionOpenAiTask();
const subscriptionSamples = new Map<string, {
  metadata: SubscriptionMetadata;
  fetchedAt: string;
  status?: SubscriptionStatus;
}>();
const subscriptionStamp = "2026-09-07T08:00:00.000Z";
const staleSubscriptionStamp = "2026-09-01T08:00:00.000Z";
const fixtureSubscriptionError = "订阅请求失败：HTTP 403。订阅服务拒绝访问；请检查订阅是否有效或联系服务商。";

function subscriptions(): SubscriptionOverview[] {
  return profiles.map(profile => {
    const sample = subscriptionSamples.get(profile.id);
    return {
      profile: structuredClone(profile), summary: structuredClone(summary), revisionCount: 1,
      latestFetchedAt: sample?.fetchedAt ?? stamp,
      latestMetadata: structuredClone(sample?.metadata ?? metadata),
      latestValidation: structuredClone(validation), active: profile.id === activeProfileId,
      ...(sample?.status ? { status: structuredClone(sample.status) } : {}),
    };
  });
}

function reportSubscriptions() {
  document.documentElement.dataset.fixtureSubscriptionsState = JSON.stringify(subscriptions());
  document.documentElement.dataset.fixtureSubscriptionsCalls = JSON.stringify(subscriptionCalls);
  document.documentElement.dataset.fixtureSubscriptionImportState = JSON.stringify({ enabled: subscriptionImportScenario !== null, scenario: subscriptionImportScenario, busy: subscriptionImportBusy, activeProfileId, profileCount: profiles.length });
  document.documentElement.dataset.fixtureSubscriptionImportCalls = JSON.stringify(subscriptionImportCalls);
  document.documentElement.dataset.fixtureSubscriptionImportResult = JSON.stringify(subscriptionImportResult);
}

function subscriptionScenario(scenario: string) {
  subscriptionScenarioRevision++;
  subscriptionImportScenario = subscriptionImportScenarios.has(scenario) ? scenario : null;
  subscriptionImportUrls.clear();
  subscriptionImportCalls.length = 0;
  subscriptionImportResult = null;
  subscriptionImportOpenAiTask = idleSubscriptionOpenAiTask();
  subscriptionCalls.create = 0;
  subscriptionSamples.clear();
  profiles = scenario === "empty" || scenario === "import-empty" ? [] : [
    makeProfile("fixture-active", "演示订阅 · 活动", true),
    makeProfile("fixture-inactive", "演示订阅 · 备用（可预览删除弹窗）", false),
  ];
  activeProfileId = profiles[0]?.id ?? null;
  if (subscriptionImportScenario && profiles.length) {
    profiles[0].source = { type: "remote_subscription", host: "active.example.invalid", userAgent: "fixture-only" };
    profiles[1].source = { type: "remote_subscription", host: "duplicate.example.invalid", userAgent: "fixture-only" };
    subscriptionImportUrls.set("https://active.example.invalid/subscription", profiles[0].id);
    subscriptionImportUrls.set("https://duplicate.example.invalid/subscription", profiles[1].id);
  }
  if (scenario === "cards") {
    const gib = 1024 ** 3;
    profiles = [
      makeProfile("fixture-active", "日常通勤 · 合成订阅", true),
      makeProfile("fixture-inactive", "备用线路 · 已过期示例", false),
      makeProfile("fixture-no-usage", "团队节点 · 未提供用量", false),
      makeProfile("fixture-stale", "旅行备用 · 上次采样", false),
    ];
    const usages = [
      { uploadBytes: 12 * gib, downloadBytes: 58 * gib, totalBytes: 200 * gib, expiresAt: Date.parse("2026-10-07T00:00:00Z") / 1000 },
      { uploadBytes: 12 * gib, downloadBytes: 103 * gib, totalBytes: 100 * gib, expiresAt: Date.parse("2026-09-01T00:00:00Z") / 1000 },
      null,
      { uploadBytes: 4 * gib, downloadBytes: 38 * gib, totalBytes: 100 * gib, expiresAt: Date.parse("2026-10-01T00:00:00Z") / 1000 },
    ];
    profiles.forEach((profile, index) => {
      const usage = usages[index];
      const stale = profile.id === "fixture-stale";
      profile.updatedAt = stale ? staleSubscriptionStamp : subscriptionStamp;
      profile.source = { type: "remote_subscription", host: `${["daily", "backup", "team", "travel"][index]}.example.invalid`, userAgent: "fixture-only" };
      subscriptionSamples.set(profile.id, {
        metadata: { ...metadata, usage },
        fetchedAt: stale ? staleSubscriptionStamp : subscriptionStamp,
        status: {
          checkedAt: subscriptionStamp, lastError: stale ? fixtureSubscriptionError : null,
          usage, usageUpdatedAt: usage ? stale ? staleSubscriptionStamp : subscriptionStamp : null,
        },
      });
    });
  }
  reportSubscriptions();
}

window.addEventListener("routedeck-fixture-subscriptions", event => {
  const detail = (event as CustomEvent<{ scenario?: string; refresh?: boolean }>).detail;
  if (!detail || !subscriptionScenarios.has(detail.scenario ?? "")) return;
  subscriptionScenario(detail.scenario!);
  if (detail.refresh === true) document.querySelector<HTMLButtonElement>("#subscriptions-refresh-list")?.click();
});
subscriptionScenario(previewQuery.get("subscriptionScenario") ?? "default");

function profileDetails(profileId: string): ProfileDetails {
  const profile = profiles.find((entry) => entry.id === profileId);
  if (!profile) throw new Error("FIXTURE_ONLY: unknown synthetic profile");
  return {
    profile,
    summary,
    revisions: [{
      schemaVersion: 1,
      id: `${profile.id}-revision`,
      profileId: profile.id,
      sourceSha256: "0".repeat(64),
      effectiveSha256: "1".repeat(64),
      fetchedAt: subscriptionSamples.get(profile.id)?.fetchedAt ?? stamp,
      subscription: structuredClone(subscriptionSamples.get(profile.id)?.metadata ?? metadata),
      validation,
      openaiPolicy: profile.openaiPolicy,
    }],
  };
}

function settings(): AppSettings {
  return {
    schemaVersion: 1,
    locale: "zh-CN",
    theme: persisted.theme,
    networkMode: fixtureRuntimeMode,
    mixedPort: 17890,
    controllerPort: 19090,
    updateChannel: "stable",
    ...updatePreferences,
    ...appearanceSettings,
  };
}

let fixtureFeedback: ConnectionFeedback = { revision: 0, operation: 0, phase: "idle", mode: "manual", health: "unchecked", retrying: false, elapsedMs: 0, issue: null, checks: [] };
let feedbackStartedAt = performance.now();
function updateFeedback(change: Partial<ConnectionFeedback>) {
  if (change.operation !== undefined && change.operation !== fixtureFeedback.operation) feedbackStartedAt = performance.now();
  fixtureFeedback = { ...fixtureFeedback, ...change, revision: fixtureFeedback.revision + 1, elapsedMs: performance.now() - feedbackStartedAt };
  void emit("connection-feedback", structuredClone(fixtureFeedback));
}
window.addEventListener("serylane-fixture-feedback", event => updateFeedback((event as CustomEvent).detail));
function fixtureHealthProbe() {
  const operation = fixtureFeedback.operation;
  updateFeedback({ phase: "enabled", health: "checking", checks: [], issue: null });
  window.setTimeout(() => {
    if (fixtureFeedback.operation !== operation) return;
    updateFeedback({ health: "partial", checks: [
      { target: "google", url: "https://example.invalid", expectedStatus: 204, actualStatus: 204, success: true, latencyMs: 800, detail: "合成检测通过" },
      { target: "cloudflare", url: "https://example.invalid", expectedStatus: 204, actualStatus: 204, success: true, latencyMs: 950, detail: "合成检测通过" },
      { target: "openai", url: "https://example.invalid", expectedStatus: 401, actualStatus: null, success: false, latencyMs: 3000, detail: "合成连接超时；未发送网络请求", failureKind: "timeout" },
    ] });
  }, 3000);
}
const readonlyReplies: Record<string, () => unknown> = {
  connection_feedback: () => structuredClone({ ...fixtureFeedback, elapsedMs: fixtureFeedback.health === "checking" ? performance.now() - feedbackStartedAt : fixtureFeedback.elapsedMs }),
  app_info: () => ({ productName: "Serylane", version: `${packageInfo.version} · 合成预览`, targetOs: previewPlatform, targetArch: previewWindows ? "x86_64" : "aarch64" }),
  app_update_status: () => structuredClone(fixtureUpdate),
  get_settings: settings,
  get_session_resume_status: () => fixtureResumeStatus,
  get_startup_status: () => ({
    launchRequested: appearanceSettings.launchAtLogin, registered: fixtureStartupRegistered,
    systemAllows: fixtureStartupAllowed, desiredRunning: fixtureDesiredRunning,
    message: fixtureStartupRegistered === null ? "合成状态：登录项读取失败。"
      : !fixtureStartupRegistered ? (appearanceSettings.launchAtLogin ? "合成状态：设置已开启，但登录项未登记。" : "合成状态：未登记登录项，不随系统启动。")
      : !appearanceSettings.launchAtLogin ? "合成状态：登录项仍存在，与设置不一致。"
      : fixtureStartupAllowed === false ? "合成状态：系统已禁用此登录项。"
      : fixtureStartupAllowed === null ? "合成状态：已登记，系统允许状态尚未核实。"
      : "合成状态：已登记登录项，未操作真实系统。",
  }),
  get_user_rules: userRulesState,
  list_proxy_programs: programState,
  check_system_proxy_compatibility: () => ({ supported: true, systemConfigured: true, compatible: true, expectedProxy: programs.proxyEndpoint, resolvedHttp: programs.proxyEndpoint, resolvedHttps: programs.proxyEndpoint, detail: "合成检查：HTTP/HTTPS 解析已指向本地代理；未读取真实注册表，也未验证真实长连接。" }),
  probe_mihomo: () => ({ available: true, path: "/fixture-only/mihomo", version: "v0.0.0-fixture", message: "纯合成状态，真实内核未启动" }),
  runtime_status: () => ({
    state: fixtureRuntimePhase, phase: fixtureRuntimePhase, binaryAvailable: true,
    binaryPath: "/fixture-only/mihomo", version: "v0.0.0-fixture", configPath: "/fixture-only/config.yaml",
    message: "合成运行态仅用于展示界面；未启动真实内核。", pid: null, startedAt: stamp, lastError: null,
  }),
  system_proxy_status: () => ({ active: fixtureSystemProxyActive, snapshotPath: null, platform: previewPlatform }),
  tun_helper_status: () => ({ supported: previewPlatform !== "linux", state: previewPlatform === "linux" ? "unsupported" : fixtureRuntimeHelperState ?? (runtimeScenarioEnabled ? fixtureRuntimeHelperReady ? "ready" : "requires_approval" : "not_installed"), message: previewPlatform === "linux" ? "Linux 暂未提供 TUN 网络接管" : "合成预览不安装或调用 Helper", protocolVersion: previewPlatform === "macos" ? 2 : 0, runtimeRunning: false, runtimePid: null, runtimeVersion: null, lastError: null }),
  global_traffic_snapshot: () => ({ enabled: true, uploadBytesPerSecond: 32000, downloadBytesPerSecond: 2400000, sampledAt: stamp, interfaces: ["fixture-only"] }),
  list_profiles: () => structuredClone(profiles),
  list_subscriptions: () => { subscriptionCalls.reads++; reportSubscriptions(); return subscriptions(); },
  get_active_profile: () => activeProfileId ? profileDetails(activeProfileId) : null,
  get_openai_policy_task: () => structuredClone(subscriptionImportOpenAiTask),
  get_proxies: () => {
    if (failNodeRead) throw new Error("合成读取失败");
    return { profileId: activeProfileId, revisionId: activeProfileId ? profileDetails(activeProfileId).profile.activeRevisionId : null, costMode: fixtureCosts?.mode ?? "quality", proxies: structuredClone(fixtureNodes) };
  },
  get_rules: () => ({ rules: [
    ...rulesPersisted.rules.filter((rule) => rule.enabled).map((rule) => {
      const [type, payload, target] = ruleFields(rule.rule);
      return { type, payload: type === "MATCH" ? "" : payload, proxy: type === "MATCH" ? payload : target };
    }),
    { type: "DomainSuffix", payload: "example.invalid", proxy: "演示节点选择" },
    { type: "IPCIDR", payload: "192.0.2.0/24", proxy: "DIRECT" },
    { type: "Match", payload: "", proxy: "DIRECT" },
  ] }),
  get_connections: () => ({ uploadTotal: 2048, downloadTotal: 16384, connections: [{
    id: "fixture-connection", metadata: { host: "preview.example.invalid", destinationPort: "443", network: "tcp" },
    chains: ["演示节点选择", "演示东京 01（虚构）"], rule: "DomainSuffix", rulePayload: "example.invalid", upload: 2048, download: 16384,
  }] }),
  runtime_logs: () => [
    { timestamp: stamp, level: "info", source: "fixture", message: "信息日志：主题切换应只调用 set_app_theme。" },
    { timestamp: stamp, level: "warning", source: "fixture", message: "提示日志：所有节点、连接和带宽数值均为虚构。" },
    { timestamp: stamp, level: "error", source: "fixture", message: "错误样式预览：此行不是实际网络错误。" },
  ],
  application_logs: () => {
    document.documentElement.dataset.fixtureApplicationLogReads = String(++applicationLogReads);
    return { retentionHours: appearanceSettings.appLogRetentionDays * 24, maxBytes: 262144, storageError: null, entries: structuredClone(applicationLogEntries) };
  },
  clear_application_logs: () => { applicationLogEntries = []; },
  run_connectivity_diagnostics: () => [{ stage: "fixture", success: true, latencyMs: null, detail: "合成诊断结果；未发送实际网络请求。" }],
  run_network_safety_check: () => ({ success: true, proxyEndpoint: "fixture-only", checks: [] }),
};

function payloadRecord(payload: InvokeArgs | undefined): Record<string, unknown> {
  return payload && !Array.isArray(payload) && !(payload instanceof ArrayBuffer)
    ? payload as Record<string, unknown>
    : {};
}

mockIPC(async (command, payload) => {
  const args = payloadRecord(payload);
  if (command === "recheck_connection") { updateFeedback({ operation: fixtureFeedback.operation + 1 }); fixtureHealthProbe(); return structuredClone(fixtureFeedback); }
  if (command === "get_openai_costs") {
    const p = profileDetails(String(args.profileId)).profile;
    return structuredClone(fixtureCosts ?? { profileId: p.id, profileRevision: p.activeRevisionId, revision: 0, mode: "quality", maxMultiplier: null, allowUnknown: false,
      nodes: policy.selectedNodes.map((n,i) => ({ name: n.name, multiplier: i ? 5 : 1, manualMultiplier: i ? 5 : 1, metadata: fixtureMetadata(i) })) });
  }
  if (command === "save_openai_costs" && nodeScenario === "costs") {
    const input = args.input as CostInput;
    if (!args.confirmed || input.profileId !== activeProfileId || input.revision !== (fixtureCosts?.revision ?? 0)) throw new Error("合成成本版本冲突");
    if (costFailure) { costFailure = false; throw new Error("合成成本保存失败"); }
    fixtureCosts = structuredClone({ ...input, revision: input.revision + 1, nodes: input.nodes.map((n,i) => ({...n, manualMultiplier: n.multiplier, metadata: {...fixtureMetadata(i), multiplierSource: n.multiplier == null ? "unknown" : "manual"}})) });
    costCalls.push(structuredClone(input));
    document.documentElement.dataset.fixtureCostCalls = JSON.stringify(costCalls);
    for (const row of fixtureCosts.nodes) {
      fixtureNodes[row.name].trafficMultiplier = row.multiplier;
      fixtureNodes[row.name].withinCostBudget = input.mode === "quality" || (row.multiplier == null ? input.allowUnknown : input.maxMultiplier == null || row.multiplier <= input.maxMultiplier);
    }
    return structuredClone(fixtureCosts);
  }
  if (nodeScenario && ["select_proxy", "clear_proxy_selection"].includes(command)) {
    const group = String(args.group), node = fixtureNodes[group];
    if (!node || args.profileId !== activeProfileId || args.revisionId !== profileDetails(activeProfileId!).profile.activeRevisionId) throw new Error("合成配置状态冲突");
    nodeCalls.push({ command, group, ...(command === "select_proxy" ? { proxy: String(args.proxy) } : {}) });
    document.documentElement.dataset.fixtureNodeCalls = JSON.stringify(nodeCalls);
    if (failNodeChoice) { failNodeChoice = false; throw new Error("合成节点切换失败"); }
    if (command === "select_proxy") {
      if (!node.all?.includes(String(args.proxy))) throw new Error("合成节点不在组内");
      node.now = String(args.proxy);
      if (group === "🤖 OpenAI 自动灾备" && node.type === "Selector") node.manualNode = node.now;
      else if (node.type !== "Selector") node.fixed = node.now;
    } else if (group === "🤖 OpenAI 自动灾备" && node.type === "Selector") node.manualNode = null;
    else if (node.type === "Selector") throw new Error("Selector 不支持 DELETE");
    else node.fixed = "";
    return;
  }
  if (command === "create_subscription_profile" && subscriptionImportScenario !== null) {
    if (subscriptionImportBusy) throw ruleError("STATE_CONFLICT", "合成订阅正在导入，请等待当前操作完成。");
    if (typeof args.displayName !== "string" || !args.displayName.trim() || args.displayName.trim().length > 128
      || typeof args.url !== "string" || typeof args.userAgent !== "string"
      || typeof args.activateAfterImport !== "boolean" || typeof args.generateOpenAi !== "boolean") {
      throw ruleError("INVALID_INPUT", "合成导入需要名称、URL、User-Agent 及明确的选用和灾备布尔值。");
    }
    let url: URL;
    try {
      url = new URL(args.url);
      if (!["http:", "https:"].includes(url.protocol) || !url.hostname.endsWith(".invalid") || url.username || url.password) throw new Error("invalid");
    } catch { throw ruleError("INVALID_INPUT", "隔离导入仅接受 http(s)://*.invalid 合成地址，不接受真实订阅或账号密码。"); }
    const requestedRevision = subscriptionScenarioRevision;
    const scenario = subscriptionImportScenario;
    subscriptionImportBusy = true;
    subscriptionCalls.create++;
    subscriptionImportCalls.push({ host: url.hostname, activateAfterImport: args.activateAfterImport, generateOpenAi: args.generateOpenAi });
    reportSubscriptions();
    try {
      await new Promise(resolve => window.setTimeout(resolve, 120));
      if (requestedRevision !== subscriptionScenarioRevision) throw ruleError("STATE_CONFLICT", "合成场景已切换，旧导入没有写入新场景。");
      if (scenario === "import-403") throw { code: "SUBSCRIPTION_ERROR", stage: "fixture_subscription", message: fixtureSubscriptionError, retryable: false };
      const existingId = subscriptionImportUrls.get(url.href);
      let profile = profiles.find(entry => entry.id === existingId);
      const created = !profile;
      if (!profile) {
        profile = makeProfile(`fixture-import-${++subscriptionImportSequence}`, args.displayName.trim(), false);
        profile.source = { type: "remote_subscription", host: url.hostname, userAgent: args.userAgent };
        profile.openaiPolicy = { ...policy, enabled: false, autoMaintain: false, selectedNodes: [], candidateCount: 0, healthyCount: 0, lastBenchmarkedAt: null };
        profiles.push(profile);
        subscriptionImportUrls.set(url.href, profile.id);
      }
      // Existing URL reuse is read-only unless explicitly selected. It never
      // invokes the refresh mock or changes the existing name/UA/revision.
      const activated = args.activateAfterImport;
      if (activated) activeProfileId = profile.id;
      const requestOpenAi = args.generateOpenAi && (created || activated);
      const openAiGeneration = !requestOpenAi ? "not_requested" : scenario === "import-openai-failed" ? "failed" : "started";
      const openAiError = openAiGeneration === "failed" ? "合成状态：OpenAI 灾备任务提交失败；订阅已保存，未执行真实节点检测。" : null;
      if (openAiGeneration !== "not_requested") {
        const failed = openAiGeneration === "failed";
        subscriptionImportOpenAiTask = { ...idleSubscriptionOpenAiTask(), running: !failed, profileId: profile.id, phase: failed ? "failed" : "preparing", message: failed ? openAiError! : "合成后台任务已提交；未检测任何真实节点。", startedAt: stamp, finishedAt: failed ? stamp : null, error: openAiError };
      }
      const details = profileDetails(profile.id);
      subscriptionImportResult = { profile: structuredClone(profile), revision: structuredClone(details.revisions[0]), summary: structuredClone(summary), updated: created, created, activated, openAiGeneration, openAiError, observationError: null };
      return structuredClone(subscriptionImportResult);
    } finally {
      subscriptionImportBusy = false;
      reportSubscriptions();
    }
  }
  if (runtimeScenarioEnabled && ["set_network_mode", "start_active_profile", "stop_mihomo", "prepare_tun_active_profile"].includes(command)) {
    const requestedMode = args.mode as NetworkMode;
    fixtureRuntimeCalls.push({ command, ...(command === "set_network_mode" ? { mode: requestedMode } : command === "start_active_profile" ? { mode: fixtureRuntimeMode } : {}) });
    reportRuntimeScenario();
    await new Promise(resolve => window.setTimeout(resolve, fixtureRuntimeDelay));
    if (command === "set_network_mode") {
      if (!["manual", "system_proxy", "tun"].includes(requestedMode)) throw routeError("INVALID_INPUT", "合成网络模式无效。");
      if (fixtureRuntimePhase === "running" && (fixtureRuntimeMode === "tun" || requestedMode === "tun")) throw routeError("STATE_CONFLICT", "切换 TUN 需要先停止。");
      if (fixtureRuntimeFailSave || (fixtureRuntimeFailRollback && fixtureRuntimeCalls.some(call => call.command === "start_active_profile"))) {
        fixtureRuntimeFailSave = false;
        throw routeError("IO_ERROR", "模拟网络模式保存失败；原设置保持不变。");
      }
      fixtureRuntimeMode = requestedMode;
      fixtureSystemProxyActive = requestedMode === "system_proxy" && fixtureRuntimePhase === "running";
      if (fixtureRuntimePhase === "running") { updateFeedback({ operation: fixtureFeedback.operation + 1, mode: requestedMode }); fixtureHealthProbe(); }
      reportRuntimeScenario();
      return settings();
    }
    if (command === "prepare_tun_active_profile") {
      if (!fixtureRuntimeHelperReady) throw routeError("STATE_CONFLICT", "合成 TUN 尚未批准。");
      return null;
    }
    if (command === "start_active_profile") {
      if (fixtureRuntimePhase === "running") throw routeError("STATE_CONFLICT", "合成核心已在运行，不允许重复启动。");
      if (fixtureRuntimeFailStart) {
        fixtureRuntimeFailStart = false;
        fixtureRuntimePhase = "crashed";
        reportRuntimeScenario();
        throw routeError("RUNTIME_ERROR", "模拟核心启动失败；没有启动真实进程。");
      }
      fixtureRuntimePhase = "running";
      fixtureDesiredRunning = true;
      fixtureSystemProxyActive = fixtureRuntimeMode === "system_proxy" && fixtureRuntimeProxyConfirmed;
      updateFeedback({ operation: fixtureFeedback.operation + 1, mode: fixtureRuntimeMode });
      fixtureHealthProbe();
    } else {
      fixtureRuntimePhase = "stopped";
      updateFeedback({ operation: fixtureFeedback.operation + 1, phase: "idle", health: "unchecked" });
      fixtureDesiredRunning = false;
      fixtureResumeStatus = { phase: "idle", message: "合成状态：用户停止，下次保持停止。" };
      fixtureSystemProxyActive = false;
    }
    reportStartup();
    reportRuntimeScenario();
    const result = readonlyReplies.runtime_status();
    if (command === "start_active_profile" && fixtureRuntimeCrashAfterStart) {
      fixtureRuntimePhase = "crashed";
      fixtureSystemProxyActive = false;
      reportRuntimeScenario();
    }
    return result;
  }
  if (["refresh_profile", "activate_profile", "delete_profile"].includes(command)) {
    const action = command === "refresh_profile" ? "refresh" : command === "activate_profile" ? "activate" : "delete";
    subscriptionCalls[action]++;
    reportSubscriptions();
    const profile = profiles.find(entry => entry.id === args.profileId);
    if (!profile) throw ruleError("NOT_FOUND", "隔离合成订阅不存在；未访问真实配置。");
    if (command === "refresh_profile") {
      await new Promise(resolve => window.setTimeout(resolve, 120));
      // A scenario switch during the mock delay must not resurrect old data.
      if (!profiles.includes(profile)) throw ruleError("STATE_CONFLICT", "隔离订阅场景已切换，请重新读取列表。");
      const previous = subscriptionSamples.get(profile.id);
      const checkedAt = new Date().toISOString();
      const usage = previous?.status?.usage ?? previous?.metadata.usage ?? null;
      const failed = profile.id === "fixture-stale";
      subscriptionSamples.set(profile.id, {
        metadata: structuredClone(previous?.metadata ?? metadata),
        fetchedAt: failed ? previous?.fetchedAt ?? stamp : checkedAt,
        status: {
          checkedAt, lastError: failed ? fixtureSubscriptionError : null, usage,
          usageUpdatedAt: usage ? failed ? previous?.status?.usageUpdatedAt ?? previous?.fetchedAt ?? stamp : checkedAt : null,
        },
      });
      reportSubscriptions();
      if (failed) throw { code: "SUBSCRIPTION_ERROR", stage: "fixture_subscription", message: fixtureSubscriptionError, retryable: false };
      const details = profileDetails(profile.id);
      return { profile: structuredClone(profile), revision: details.revisions[0], summary: structuredClone(summary), updated: false };
    }
    if (command === "activate_profile") {
      if (activeProfileId === profile.id) throw ruleError("STATE_CONFLICT", "该合成订阅已在使用，无需重复激活。");
      if (args.revisionId && args.revisionId !== `${profile.id}-revision`) throw ruleError("NOT_FOUND", "隔离订阅版本不存在。");
      activeProfileId = profile.id;
      reportSubscriptions();
      return profileDetails(profile.id); // In-memory selection only; never start a core.
    }
    if (activeProfileId === profile.id) throw ruleError("STATE_CONFLICT", "当前合成订阅正在使用，请先激活其他订阅后再删除。");
    profiles = profiles.filter(entry => entry.id !== profile.id);
    subscriptionSamples.delete(profile.id);
    reportSubscriptions();
    return;
  }
  if (command === "local_route_status") {
    routeCalls.reads++; reportRoute();
    if (routeFailRead) throw routeError("IO_ERROR", "模拟读取路由状态失败；未访问本机服务。");
    return structuredClone(route);
  }
  if (command === "save_local_route") {
    routeCalls.saves++; reportRoute();
    if (args.expectedRevision !== route.revision) throw routeError("STATE_CONFLICT", "路由设置已更新，未覆盖其他修改；当前草稿保留。");
    if (route.enabled || route.codex.hasBackup) throw routeError("STATE_CONFLICT", "请先恢复 Codex 接入并关闭路由再修改设置。");
    const value = args.settings as RouteSettings | undefined;
    if (!value || !Number.isInteger(value.listenPort) || value.listenPort < 1024 || value.listenPort > 65535
      || !["compatible", "native"].includes(value.mode) || !["chatgpt", "openai_api"].includes(value.upstream)
      || typeof value.outboundProxy !== "string") throw routeError("INVALID_INPUT", "路由设置格式无效。");
    if (value.outboundProxy) {
      try {
        const proxy = new URL(value.outboundProxy);
        if (!["http:", "https:", "socks5:", "socks5h:"].includes(proxy.protocol)
          || !["localhost", "127.0.0.1", "[::1]"].includes(proxy.hostname)
          || !proxy.port || Number(proxy.port) === value.listenPort || proxy.username || proxy.password
          || proxy.search || proxy.hash || !["", "/"].includes(proxy.pathname)) throw new Error("invalid");
      } catch { throw routeError("INVALID_INPUT", "出站代理只接受本机明确端口，不接受账号、路径或路由自身。"); }
    }
    await new Promise(resolve => window.setTimeout(resolve, 120));
    if (args.expectedRevision !== route.revision) throw routeError("STATE_CONFLICT", "保存期间路由版本已变化；没有覆盖新版本。");
    if (routeFailSave) { routeFailSave = false; throw routeError("IO_ERROR", "模拟保存失败，旧设置与运行状态保留。"); }
    route.settings = structuredClone(value); route.endpoint = `http://127.0.0.1:${value.listenPort}`; route.revision++;
    reportRoute(); return structuredClone(route); // Save never starts or attaches.
  }
  if (command === "set_local_route_enabled") {
    routeCalls.enables++; reportRoute();
    if (args.confirmed !== true || typeof args.enabled !== "boolean") throw routeError("INVALID_INPUT", "需要单独确认启动或关闭路由。");
    if (args.expectedRevision !== route.revision) throw routeError("STATE_CONFLICT", "路由设置已更新，请刷新后重新确认。");
    if (!args.enabled && route.active > 0) throw routeError("STATE_CONFLICT", "存在进行中请求，未恢复配置或关闭服务。");
    if (args.enabled && routeFailStart) {
      routeFailStart = false; route.enabled = true; route.running = false; route.revision++;
      route.lastError = "隔离合成错误：监听端口被占用，未创建任何真实监听端口。";
      reportRoute(); throw routeError("RUNTIME_ERROR", "模拟启动失败：监听端口已被占用。");
    }
    if (!args.enabled && route.codex.hasBackup) restoreFixtureRoute();
    route.enabled = route.running = args.enabled; route.lastError = null; route.revision++;
    reportRoute(); return structuredClone(route);
  }
  if (command === "set_codex_route") {
    routeCalls.bindings++; reportRoute();
    if (args.confirmed !== true || typeof args.attach !== "boolean") throw routeError("INVALID_INPUT", "接入与恢复需要独立确认。");
    if (args.expectedConfigRevision !== route.codex.configRevision) throw routeError("STATE_CONFLICT", "Codex 配置版本已变化；未覆盖其他修改。");
    if (route.active > 0) throw routeError("STATE_CONFLICT", "存在进行中请求，未改变接入配置。");
    if (args.attach) {
      if (!route.running || route.codex.hasBackup) throw routeError("STATE_CONFLICT", "请先单独启动路由，已有接入备份时不能重复接入。");
      attachFixtureRoute();
    } else {
      if (!route.codex.hasBackup) throw routeError("STATE_CONFLICT", "没有 Serylane 接入备份可恢复。");
      restoreFixtureRoute();
    }
    reportRoute(); return structuredClone(route);
  }
  if (command === "set_openai_stability") {
    routeCalls.stability++; reportRoute();
    if (args.confirmed !== true || typeof args.enabled !== "boolean") throw routeError("INVALID_INPUT", "稳定策略需要独立确认。");
    const active = profiles.find(profile => profile.id === activeProfileId);
    if (active && active.id === args.profileId && active.activeRevisionId === args.revisionId && active.openaiPolicy.enabled) {
      active.openaiPolicy.stabilityEnabled = args.enabled;
      active.activeRevisionId = `fixture-stability-${routeCalls.stability}`;
      // Changing the proxy policy never enables the local route or Codex binding.
      reportRoute(); return;
    }
    if (!route.stability.eligible || args.profileId !== route.stability.profileId || args.revisionId !== route.stability.revisionId)
      throw routeError("STATE_CONFLICT", "当前灾备配置不可用或已更新，请刷新后重新确认。");
    route.stability.enabled = route.stability.running = args.enabled;
    route.stability.revisionId = `fixture-stability-${routeCalls.stability}`;
    route.stability.message = "隔离合成策略状态；没有探测节点、切换真实连接或验证模型流。";
    reportRoute(); return;
  }
  // UI-only settings save is synthetic. Port/mode changes remain fail-closed.
  if (command === "update_settings") {
    const value = args.settings as AppSettings;
    const current = settings();
    if (!value || value.networkMode !== current.networkMode || value.mixedPort !== current.mixedPort || value.controllerPort !== current.controllerPort) {
      networkMutationCount++;
      report("已拦截预览中的网络模式或端口修改");
      throw new Error("FIXTURE_ONLY: 网络模式与端口不可在预览中修改");
    }
    if (!Number.isInteger(value.appLogRetentionDays) || value.appLogRetentionDays < 1 || value.appLogRetentionDays > 90) throw new Error("INVALID_INPUT: 应用日志保留天数必须在 1 到 90 之间");
    if (fixtureSettingsFailSave) {
      fixtureSettingsFailSave = false;
      throw new Error("IO_ERROR: 合成保存失败，原设置及登录项保持不变");
    }
    if (value.launchAtLogin !== appearanceSettings.launchAtLogin) {
      fixtureStartupRegistered = value.launchAtLogin;
      if (value.launchAtLogin) fixtureStartupAllowed = true;
    }
    appearanceSettings = { launchAtLogin: value.launchAtLogin, silentStartup: value.silentStartup, restoreLastSession: value.restoreLastSession, showGlobalTraffic: value.showGlobalTraffic, diagnosticsRetentionDays: value.diagnosticsRetentionDays, appLogRetentionDays: value.appLogRetentionDays };
    reportStartup();
    document.documentElement.dataset.fixtureSettingsSaves = String(++settingsSaveCount);
    report("运行偏好已保存到合成状态；未触及系统设置");
    return settings();
  }
  if (command === "save_update_preferences") {
    const source = args.source as UpdateSource;
    if (source !== updatePreferences.updateSource) fixtureUpdate = { phase: "idle", info: null, downloadedBytes: 0, totalBytes: 0, error: null };
    updatePreferences = { updateSource: source, autoCheckUpdates: Boolean(args.autoCheck), autoDownloadUpdates: Boolean(args.autoDownload) };
    return settings();
  }
  if (command === "check_app_update") {
    await new Promise((resolve) => window.setTimeout(resolve, 180));
    if (updateScenario === "failed") {
      fixtureUpdate = { ...fixtureUpdate, info: null, phase: "failed", error: "合成状态：GitHub HTTP 403；Gitee HTTP 503" };
      throw new Error(fixtureUpdate.error!);
    }
    const latestVersion = updateScenario === "ahead" ? "v0.1.0" : updateScenario === "current" ? `v${packageInfo.version}` : "v9.0.0";
    const source = updatePreferences.updateSource === "github" ? "github" : "gitee";
    fixtureUpdate = { phase: updateScenario === "ahead" ? "ahead" : updateScenario === "current" ? "current" : "available", downloadedBytes: 0, totalBytes: 0, error: null,
      info: { currentVersion: packageInfo.version, latestVersion, available: !["ahead", "current"].includes(updateScenario), ahead: updateScenario === "ahead", notes: "合成更新说明：演示双渠道回退与确认安装。未访问真实发布服务。", publishedAt: stamp, source,
        releaseUrl: `https://${source}.com/${source === "github" ? "CMMUU" : "cmmuu"}/routedeck/releases/tag/${latestVersion}`,
        channels: updatePreferences.updateSource === "auto" ? [{ source: "github", version: null, error: "合成 HTTP 403；已使用 Gitee" }, { source: "gitee", version: latestVersion, error: null }] : [{ source, version: latestVersion, error: null }],
      } };
    return structuredClone(fixtureUpdate);
  }
  if (command === "download_app_update") {
    if (!fixtureUpdate.info?.available || fixtureUpdate.info.latestVersion !== args.versionTag) throw new Error("合成版本冲突");
    cancelUpdateDownload = false;
    fixtureUpdate = { ...fixtureUpdate, phase: "downloading", totalBytes: 20 * 1048576, downloadedBytes: 0, error: null };
    for (let step = 1; step <= 8; step++) {
      await new Promise((resolve) => window.setTimeout(resolve, 180));
      if (cancelUpdateDownload) { fixtureUpdate.phase = "cancelled"; fixtureUpdate.error = "合成下载已取消"; throw new Error(fixtureUpdate.error); }
      fixtureUpdate.downloadedBytes = fixtureUpdate.totalBytes * step / 8;
    }
    if (updateScenario === "bad-signature") { fixtureUpdate.phase = "failed"; fixtureUpdate.error = "合成签名校验失败，已拒绝安装"; throw new Error(fixtureUpdate.error); }
    fixtureUpdate.phase = "ready";
    return structuredClone(fixtureUpdate);
  }
  if (command === "cancel_app_update") { cancelUpdateDownload = true; return; }
  if (command === "install_app_update") {
    if (!args.confirmed || fixtureUpdate.phase !== "ready" || args.versionTag !== fixtureUpdate.info?.latestVersion) throw new Error("合成安装条件不满足");
    document.documentElement.dataset.fixtureUpdateInstalls = String(++updateInstallCount);
    fixtureUpdate.phase = "installing";
    return; // Synthetic receipt only; no processes, files, proxy changes or exit.
  }
  if (command === "list_installed_proxy_applications") return fixtureCatalog(args.refresh === true);
  if (command === "inspect_proxy_application") {
    const app = fixtureInstalledApplication();
    document.documentElement.dataset.fixtureInspectedApplication = String(args.path);
    if ("kind" in app.binding) app.binding.location = String(args.path);
    return app;
  }
  if (command === "choose_proxy_program") {
    return cancelProgramPicker ? null : programPickerPath ?? (previewPlatform === "macos" ? "/Applications/Fixture Codex.app" : previewPlatform === "linux" ? "/usr/share/applications/invalid.fixture.Codex.desktop" : "C:\\Program Files\\Example App\\Example.exe");
  }
  if (command === "save_proxy_program" || command === "delete_proxy_program") {
    await new Promise((resolve) => window.setTimeout(resolve, 100));
    if (args.expectedRevision !== programs.revision) throw ruleError("STATE_CONFLICT", "程序清单已更新，请刷新列表后重试；编辑内容仍保留");
    if (command === "save_proxy_program") {
      if (failProgramSave) { failProgramSave = false; throw ruleError("IO_ERROR", "模拟保存失败；原清单未变化"); }
      const input = args.input as ProgramInput;
      if (!input.name.trim() || (!input.binding && !(previewWindows ? /^[a-z]:\\.+\.exe$/i.test(input.executable) : input.executable.startsWith("/")))) throw ruleError("INVALID_INPUT", "请选择有效的应用或程序文件");
      if (input.binding && "kind" in input.binding && input.binding.kind === "macos" && (input.workingDirectory || input.workingDirectoryRelative)) throw ruleError("INVALID_INPUT", "macOS .app 使用系统原生工作目录，请切换为默认工作目录。");
      const old = programs.programs.find((program) => program.id === input.id);
      if (input.id && !old) throw ruleError("NOT_FOUND", "程序条目已被删除");
      const entry = { ...input, id: input.id ?? crypto.randomUUID(), available: true, runningPid: old?.runningPid ?? null, launchPending: old?.launchPending ?? false };
      if (old) programs.programs[programs.programs.indexOf(old)] = entry;
      else programs.programs.push(entry);
    } else {
      if (!programs.programs.some((program) => program.id === args.programId)) throw ruleError("NOT_FOUND", "程序条目不存在");
      programs.programs = programs.programs.filter((program) => program.id !== args.programId);
    }
    programs.revision++;
    return persistPrograms();
  }
  if (command === "launch_proxy_program") {
    if (args.expectedRevision !== programs.revision) throw ruleError("STATE_CONFLICT", "程序清单已更新，未启动程序，请刷新后重新确认");
    const program = programs.programs.find((entry) => entry.id === args.programId);
    if (!program || !program.available || !programs.coreRunning || program.runningPid || program.launchPending) throw ruleError("STATE_CONFLICT", "合成启动条件不满足");
    program.runningPid = 4567;
    program.launchPending = false;
    document.documentElement.dataset.fixtureProgramLaunches = String(++programLaunches);
    return programState(); // Simulation only. No native process or network calls.
  }
  if (command === "validate_user_rules") return validateRules(args.rules);
  if (command === "parse_user_rules_text") {
    if (typeof args.text !== "string") throw ruleError("INVALID_INPUT", "规则文本必须是字符串");
    return parseRulesText(args.text);
  }
  if (command === "save_user_rules") return writeRules(args, false);
  if (command === "rollback_user_rules") return writeRules(args, true);
  if (command === "set_app_theme") {
    const theme = args.theme;
    if (!THEMES.includes(theme as ThemePreference)) throw new Error("FIXTURE_ONLY: invalid theme");
    themeSaveCount += 1;
    report("正在模拟主题持久化");
    await new Promise((resolve) => window.setTimeout(resolve, 180));
    if (failNextThemeSave) {
      failNextThemeSave = false;
      report("已模拟保存失败；持久化主题未变化");
      throw new Error("FIXTURE_SAVE_FAILED: 模拟主题保存失败，请重试");
    }
    const next = { ...persisted, theme: theme as ThemePreference };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
    persisted = next;
    report("主题已保存到隔离测试空间；网络配置未变化");
    return settings();
  }
  if (command === "get_profile_details") return profileDetails(String(args.profileId));
  if (command === "get_current_node_details") {
    const chain = [String(args.group)];
    for (let count = 0; count < 16 && fixtureNodes[chain[chain.length - 1]]?.now; count++) chain.push(fixtureNodes[chain[chain.length - 1]].now!);
    const name = chain[chain.length - 1];
    return {
      group: String(args.group), nodeName: name,
      routeChain: chain, nodeType: fixtureNodes[name]?.type ?? "Unknown", alive: fixtureNodes[name]?.alive ?? null,
      udp: true, uot: false, xudp: false, tfo: false, mptcp: false, smux: false,
      providerName: "合成 Provider", maskedServer: "*.example.invalid", port: 443,
      network: "tcp", tls: "TLS", dialerProxy: null, interface: null,
      history: [42, 45, 40, 48, 42].map((delayMs) => ({ time: stamp, delayMs })), lastDelayMs: 42,
    };
  }
  if (Object.prototype.hasOwnProperty.call(readonlyReplies, command)) return readonlyReplies[command]();

  // Fail closed: even a future/new command never falls through to real Tauri.
  // This includes start/stop, subscribe, settings/network changes and probes.
  networkMutationCount += 1;
  report(`已拦截真实状态修改调用：${command}`);
  throw new Error(`FIXTURE_ONLY: 已拦截 ${command}；此预览仅允许合成数据与隔离交互状态`);
}, { shouldMockEvents: true });

// Supply a stable, mutable color-scheme MediaQueryList before main.ts imports.
const originalMatchMedia = window.matchMedia.bind(window);
const colorQueries = new Map<string, MediaQueryList>();
window.matchMedia = (query: string): MediaQueryList => {
  if (!/^\(\s*prefers-color-scheme\s*:\s*(dark|light)\s*\)$/.test(query)) {
    return originalMatchMedia(query);
  }
  const existing = colorQueries.get(query);
  if (existing) return existing;
  const target = new EventTarget();
  const mql = Object.assign(target, {
    media: query,
    onchange: null as MediaQueryList["onchange"],
    addListener(callback: ((event: MediaQueryListEvent) => void) | null) {
      if (callback) target.addEventListener("change", callback as EventListener);
    },
    removeListener(callback: ((event: MediaQueryListEvent) => void) | null) {
      if (callback) target.removeEventListener("change", callback as EventListener);
    },
  }) as MediaQueryList;
  Object.defineProperty(mql, "matches", { get: () => query.includes("dark") ? persisted.systemDark : !persisted.systemDark });
  colorQueries.set(query, mql);
  return mql;
};

function setSystemDark(dark: boolean): void {
  if (persisted.systemDark === dark) return;
  const next = { ...persisted, systemDark: dark };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  persisted = next;
  for (const mql of colorQueries.values()) {
    const event = new MediaQueryListEvent("change", { matches: mql.matches, media: mql.media });
    mql.dispatchEvent(event);
    mql.onchange?.call(mql, event);
  }
  report("仅修改预览中的系统外观信号；真实系统设置未变化");
}

function blockBrowserRequest(): never {
  blockedBrowserRequestCount += 1;
  report("已拦截脚本网络请求");
  throw new Error("FIXTURE_ONLY: browser network requests are disabled");
}

// The browser still loads same-origin Vite modules/styles. Application-level
// fetch/XHR/beacon/socket calls are blocked, including localhost API requests.
window.fetch = async () => blockBrowserRequest();
XMLHttpRequest.prototype.open = function () { blockBrowserRequest(); };
navigator.sendBeacon = () => { blockBrowserRequest(); };
const OriginalWebSocket = window.WebSocket;
window.WebSocket = class extends OriginalWebSocket {
  constructor(url: string | URL, protocols?: string | string[]) {
    const parsed = new URL(url, location.href);
    const protocolList = typeof protocols === "string" ? [protocols] : protocols ?? [];
    const isLocalVite = parsed.hostname === location.hostname
      && parsed.port === location.port && protocolList.includes("vite-hmr");
    if (!isLocalVite) blockBrowserRequest();
    super(url, protocols);
  }
};
window.EventSource = class extends EventSource {
  constructor(_url: string | URL, _configuration?: EventSourceInit) {
    blockBrowserRequest();
    // No connection is ever created; required super is unreachable by design.
    super("about:blank");
  }
};

element("fixture-system-light").addEventListener("click", () => setSystemDark(false));
element("fixture-system-dark").addEventListener("click", () => setSystemDark(true));
for (const [phase, message] of [
  ["paused", "合成暂停：其他程序已接管系统代理，未自动覆盖。请检查后手动启动。"],
  ["restoring", "合成进度：正在恢复上次配置与网络模式；未启动真实内核。"],
  ["idle", "合成状态：上次核心已停止，保持停止。未读取真实应用数据。"],
]) element(`fixture-resume-${phase}`).addEventListener("click", () => {
  fixtureResumeStatus = { phase, message };
  element("global-refresh").click();
});
element("fixture-fail-save").addEventListener("click", () => {
  failNextThemeSave = !failNextThemeSave;
  report(failNextThemeSave ? "下一次实际主题保存将失败；请选择不同主题" : "已取消模拟保存失败");
});
element("fixture-rules-fail-save").addEventListener("click", () => {
  failNextRulesSave = !failNextRulesSave;
  report(failNextRulesSave ? "下一次规则保存或回滚将模拟应用失败" : "已取消模拟规则应用失败");
});
element("fixture-rules-conflict").addEventListener("click", () => {
  const externalRules = copy(rulesPersisted.rules);
  externalRules.push({
    id: crypto.randomUUID(), enabled: true,
    rule: `DOMAIN,concurrent-${rulesPersisted.revision + 1}.example.invalid,DIRECT`,
    note: "模拟其他窗口新增；当前未保存草稿应保留",
  });
  const updated = persistRules(externalRules);
  report(`已模拟其他窗口写入版本 ${updated.revision}；当前页面仍持有旧版本，可测试冲突与刷新`);
});
element("fixture-reload").addEventListener("click", () => location.reload());
element("fixture-reset").addEventListener("click", () => {
  localStorage.removeItem(STORAGE_KEY);
  localStorage.removeItem(RULES_STORAGE_KEY);
  location.reload();
});

const banner = element("fixture-banner");
const fixtureTitlebar = element("fixture-titlebar");
let fixtureChromeFrame: number | null = null;
let fixtureChromeHeight = -1;
function scheduleFixtureChromeSize() {
  if (fixtureChromeFrame !== null) return;
  // A root style write can resize an observed banner. Defer it outside the
  // ResizeObserver delivery cycle and never rewrite an unchanged measurement.
  fixtureChromeFrame = window.requestAnimationFrame(() => {
    fixtureChromeFrame = null;
    const height = banner.offsetHeight + fixtureTitlebar.offsetHeight;
    if (height === fixtureChromeHeight) return;
    fixtureChromeHeight = height;
    document.documentElement.style.setProperty("--fixture-banner-height", `${height}px`);
  });
}
const fixtureChromeResize = new ResizeObserver(scheduleFixtureChromeSize);
fixtureChromeResize.observe(banner);
fixtureChromeResize.observe(fixtureTitlebar);
scheduleFixtureChromeSize();

window.addEventListener("error", (event) => {
  runtimeErrorCount += 1;
  report(`未捕获错误：${event.message}`);
});
window.addEventListener("unhandledrejection", (event) => {
  runtimeErrorCount += 1;
  report(`未处理 Promise：${String(event.reason)}`);
});

report("隔离桥接已安装，正在加载真实 src/main.ts");
void import("../../src/main").then(() => {
  // Navigation only, never a command or actual application launch.
  const view = previewQuery.get("view");
  if (view === "routing" || view === "subscriptions" || view === "settings") document.querySelector<HTMLButtonElement>(`[data-view="${view}"]`)?.click();
  report("预览已就绪；可测试主题、规则、路由合成状态、独立确认、失败与版本冲突");
}).catch((error: unknown) => {
  runtimeErrorCount += 1;
  report(`前端加载失败：${error instanceof Error ? error.message : String(error)}`);
});
