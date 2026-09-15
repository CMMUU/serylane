import "./styles.css";
import "./desktop-theme.css";
import "./local-routing.css";
import "./subscription-cards.css";
import "./node-selection.css";
import { generalGroups, currentNodeMarkup, nodeSelectionMarkup, type ProxyMap } from "./node-selection";
import { mountOpenAiCosts, openAiCostsMarkup } from "./openai-costs";
import { subscriptionCardMarkup } from "./subscription-cards";
import { subscriptionImportMarkup, describeSubscriptionImport } from "./subscription-import";
import { NAV_ITEMS, navigationMarkup, type ViewName } from "./ui";
import { preferencesMarkup } from "./settings-view";
import { mountLogs } from "./log-view";
import { installContinuousScrolling } from "./scrolling";
import { sessionResumePresentation, canStopSession, startupModeFromSettings, startupModeSettings, startupModeHelp, startupRegistrationPresentation, type StartupMode } from "./session-resume";
import { canStartRuntime, startRuntimeInMode, type RuntimeStartMode } from "./runtime-start";
import { api, errorMessage, revisionLabel } from "./api";
import { describeAppUpdate } from "./app-update";
import { listen } from "@tauri-apps/api/event";
import { mountRuleManager, ruleManagerMarkup } from "./rule-manager";
import { mountLocalRouting, localRoutingMarkup } from "./local-routing";
import { mountProgramManager, programManagerMarkup, mountProxyCompatibility, proxyCompatibilityMarkup } from "./program-proxy";
import {
  THEME_OPTIONS,
  ThemeController,
  isThemePreference,
  themeColorScheme,
} from "./theme";
import type { ThemePreference, ThemeSnapshot } from "./theme";
import type {
  AppInfo,
  AppSettings,
  AppUpdateInfo,
  AppUpdateStatus,
  UpdateSource,
  BinaryInfo,
  CurrentNodeDetails,
  GlobalTrafficSnapshot,
  NetworkMode,
  NetworkSafetyReport,
  OpenAiPolicyTask,
  ProfileDetails,
  ProfileRecord,
  RuntimeStatus,
  SessionResumeStatus,
  StartupStatus,
  SystemProxyStatus,
  SubscriptionOverview,
  TunHelperStatus,
} from "./types";

const continuousScrolling = installContinuousScrolling();
window.addEventListener("pagehide", () => continuousScrolling.dispose(), { once: true });

const sampleProfile = `mixed-port: 7890
mode: rule
log-level: info
ipv6: false
proxies:
  - name: 本地 SOCKS5 示例
    type: socks5
    server: 127.0.0.1
    port: 1080
proxy-groups:
  - name: PROXY
    type: select
    proxies:
      - 本地 SOCKS5 示例
      - DIRECT
rules:
  - DOMAIN-SUFFIX,example.com,PROXY
  - MATCH,DIRECT
dns:
  enable: true
  enhanced-mode: fake-ip
  nameserver:
    - 1.1.1.1
    - 8.8.8.8
`;

const store: {
  view: ViewName;
  appInfo: AppInfo | null;
  appUpdate: AppUpdateInfo | null;
  settings: AppSettings | null;
  binary: BinaryInfo | null;
  runtime: RuntimeStatus | null;
  systemProxy: SystemProxyStatus | null;
  tunHelper: TunHelperStatus | null;
  profiles: ProfileRecord[];
  subscriptions: SubscriptionOverview[];
  activeProfile: ProfileDetails | null;
  selectedProfile: ProfileDetails | null;
  proxies: Record<string, unknown> | null;
  rules: Record<string, unknown> | null;
  connections: Record<string, unknown> | null;
  openAiTask: OpenAiPolicyTask | null;
  nodeDetails: CurrentNodeDetails | null;
  networkSafety: NetworkSafetyReport | null;
  globalTraffic: GlobalTrafficSnapshot | null;
} = {
  view: "overview",
  appInfo: null,
  appUpdate: null,
  settings: null,
  binary: null,
  runtime: null,
  systemProxy: null,
  tunHelper: null,
  profiles: [],
  subscriptions: [],
  activeProfile: null,
  selectedProfile: null,
  proxies: null,
  rules: null,
  connections: null,
  openAiTask: null,
  nodeDetails: null,
  networkSafety: null,
  globalTraffic: null,
};

const app = document.querySelector<HTMLDivElement>("#app");
if (!app) throw new Error("#app not found");
const appIconUrl = new URL("../assets/brand/app-icon-128.png", import.meta.url).href;
let subscriptionImporting = false;
let subscriptionFormInitialized = false;
let subscriptionDraftDirty = false;
let subscriptionActivationTouched = false;
let highlightedSubscriptionId: string | null = null;
let viewNavigationRevision = 0;
const subscriptionRefreshing = new Set<string>();
let openAiTaskFinishedAt: string | null = null;
let networkModeSwitching = false;
let runtimeActionInFlight = false;
let stabilityActionInFlight = false;
let nodeSelectionBusy = false;
let proxyReadSequence = 0;
let proxyPolling = false;
let overviewGroup = "";
let overviewProfile = "";
let overviewNodeDetails: Record<string, CurrentNodeDetails> = {};
let proxyReadError = "";
let runtimeMutationRevision = 0;
let runtimeReadSequence = 0;
let baseReadSequence = 0;
let settingsSaving = false;
let appUpdateChecking = false;
let appUpdateError: string | null = null;
let appUpdateStatus: AppUpdateStatus = { phase: "idle", info: null, downloadedBytes: 0, totalBytes: 0, error: null };
let appUpdateActionBusy = false;
let appUpdatePollBusy = false;
let appUpdateOperationId = 0;
let automaticUpdateCheckScheduled = false;
let updateCheckAttemptedThisSession = false;
let appearanceFeedback = "";
let sessionResumeStatus: SessionResumeStatus | null = null;
let sessionResumeRevision = 0;
let sessionResumeReadBusy = false;
let sessionResumeReadAgain = false;
let startupStatus: StartupStatus | null = null;
// An asynchronous refresh must not overwrite an unsaved startup-mode choice.
let startupModeDraft: StartupMode | null = null;
const OPENAI_GROUP_NAME = "🤖 OpenAI 自动灾备";
document.documentElement.dataset.view = store.view;

app.innerHTML = `
  <div class="app-shell">
    <aside class="sidebar">
      <div class="brand">
        <div class="brand-mark"><img src="${appIconUrl}" alt="" width="40" height="40" /></div>
        <div><strong>Serylane</strong><span>轻量稳定代理客户端</span></div>
      </div>
      <nav class="nav-list" aria-label="主导航">${navigationMarkup}</nav>
      <div class="sidebar-footer">
        <div class="sidebar-runtime-status">
          <span class="status-dot" id="sidebar-status-dot"></span>
          <span id="sidebar-status">代理核心 · 未读取</span>
        </div>
        <div class="global-traffic-compact" id="global-traffic-compact" aria-label="全局实时流量">
          <span class="traffic-upload"><b>上传</b><strong id="global-upload-rate">—</strong></span>
          <span class="traffic-download"><b>下载</b><strong id="global-download-rate">—</strong></span>
        </div>
      </div>
      <span class="sidebar-version" id="sidebar-version"></span>
    </aside>
    <main class="main-content">
      <header class="topbar" aria-label="应用状态与全局控制">
        <div class="page-heading"><h1 id="page-title">概览</h1><p class="page-subtitle">Codex 模型请求的独立入口</p><div class="application-status-meta"><span id="application-profile-state">未选择订阅</span><span id="application-mode-state">Manual</span></div></div>
        <div class="topbar-actions">
          <span class="core-toolbar-label">代理核心</span>
          <div class="global-secondary-actions">
            <span class="platform-chip" id="platform-chip">检测平台中</span>
            <button class="button status-mode-button" id="global-system-proxy" type="button" aria-pressed="false">系统代理</button>
            <button class="button status-mode-button" id="global-tun" type="button" aria-pressed="false">TUN 模式</button>
            <button class="button button-quiet" id="global-refresh">刷新</button>
          </div>
          <div class="header-runtime" aria-live="polite"><span class="application-status-dot" id="application-status-dot"></span><strong id="application-runtime-state">正在读取</strong></div>
          <button class="button button-primary" id="global-start" title="使用上次保存的网络模式与选用配置启动" aria-label="按上次方案启动" disabled>启动</button>
          <button class="button button-danger" id="global-stop" disabled>停止</button>
        </div>
      </header>

      <div class="page-scroll" id="page-scroll" tabindex="0" role="region" aria-labelledby="page-title">
      <aside class="session-resume-notice is-hidden" id="session-resume-notice" role="status" aria-live="polite"><strong id="session-resume-title"></strong><p id="session-resume-message"></p></aside>
      <section class="view-stack" id="overview-view">
        <div class="hero-grid">
          <article class="connection-card">
            <div class="section-label">运行状态</div>
            <div class="connection-main">
              <span class="large-status-dot" id="connection-dot"></span>
              <div><h2 id="connection-state">读取中</h2><p id="connection-message">正在检查内核和配置。</p></div>
            </div>
            <div class="connection-meta">
              <span>核心</span><strong id="runtime-version">—</strong>
              <span>PID</span><strong id="runtime-pid">—</strong>
              <span>配置</span><strong id="runtime-config">—</strong>
            </div>
          </article>
          <article class="health-card">
            <div class="section-label">网络接管</div>
            <div class="health-row"><span>模式</span><strong id="overview-mode">—</strong></div>
            <div class="health-row"><span>本地代理</span><strong id="overview-endpoint">—</strong></div>
            <div class="health-row"><span>系统代理</span><strong id="overview-system-proxy">—</strong></div>
            <div class="health-row"><span>当前档案</span><strong id="overview-profile">—</strong></div>
          </article>
        </div>
        <article class="panel control-center-panel">
          <div class="control-center-heading">
            <div>
              <div class="section-label">网络控制中心</div>
              <h2>接管方式与路由策略</h2>
              <p id="control-profile-caption">当前订阅：未选择</p>
            </div>
            <span class="control-state-pill" id="control-runtime-pill">已停止</span>
          </div>
          <div class="control-grid">
            <label class="control-tile" for="home-system-proxy">
              <span class="control-icon proxy-icon">S</span>
              <span class="control-copy">
                <strong>系统代理</strong>
                <small>让遵循系统代理的应用接入 Mihomo</small>
              </span>
              <span class="toggle-switch">
                <input id="home-system-proxy" type="checkbox" />
                <span class="toggle-track"><span class="toggle-thumb"></span></span>
              </span>
            </label>
            <label class="control-tile" for="home-tun">
              <span class="control-icon tun-icon">T</span>
              <span class="control-copy">
                <strong>TUN 模式</strong>
                <small>接管更完整的系统 IP 流量</small>
              </span>
              <span class="toggle-switch">
                <input id="home-tun" type="checkbox" />
                <span class="toggle-track"><span class="toggle-thumb"></span></span>
              </span>
            </label>
            <div class="control-tile routing-tile">
              <span class="control-icon routing-icon">R</span>
              <span class="control-copy">
                <strong>订阅路由</strong>
                <small>决定当前订阅如何处理全部连接</small>
              </span>
              <div class="routing-segments" id="home-routing-mode" role="group" aria-label="订阅路由模式">
                <button type="button" data-routing-mode="global">全局</button>
                <button type="button" data-routing-mode="rule">规则</button>
                <button type="button" data-routing-mode="direct">直连</button>
              </div>
            </div>
          </div>
          <p class="control-hint" id="control-hint">系统代理和 TUN 互斥；切换时应用会自动停止、应用设置并恢复运行。</p>
        </article>
        <article class="panel" id="overview-nodes-panel">
          <div class="panel-heading"><div><h2>当前选用节点</h2><p class="node-choice-help">按策略组展示所选出口；规则模式下，不同请求可能使用不同节点。</p></div><button class="button button-quiet" id="overview-nodes-refresh">刷新节点</button></div>
          <div id="overview-nodes-content"><p class="node-choice-help">启动核心后读取节点信息。</p></div>
        </article>
        <div class="metrics-grid">
          <article class="metric-card"><span>配置档案</span><strong id="metric-profiles">0</strong><small>profiles</small></article>
          <article class="metric-card"><span>节点</span><strong id="metric-nodes">—</strong><small>active profile</small></article>
          <article class="metric-card"><span>规则</span><strong id="metric-rules">—</strong><small>active profile</small></article>
          <article class="metric-card"><span>运行阶段</span><strong id="metric-phase">—</strong><small>runtime state</small></article>
        </div>
        <article class="panel subscription-onboarding" id="overview-subscription-guide" hidden>
          <div><h2>先选用一个配置</h2><p>在「订阅」统一添加和选用订阅，本地 YAML 仍在「配置」管理。</p></div>
          <button class="button button-quiet" id="overview-go-subscriptions" type="button">前往订阅</button>
        </article>
      </section>

      <section class="view-stack is-hidden" id="subscriptions-view">
        <article class="panel subscriptions-panel">
          <div class="panel-heading subscription-panel-heading">
            <div>
              <div class="section-label">SUBSCRIPTIONS</div>
              <h2>订阅源与更新状态</h2>
            </div>
            <div class="toolbar">
              <button class="button button-quiet" id="subscriptions-run-safety">安全检查</button>
              <button class="button button-quiet" id="subscriptions-refresh-all">刷新全部</button>
              <button class="button button-primary" id="subscriptions-add" type="button" aria-controls="managed-subscription-panel" aria-expanded="false">＋ 添加订阅</button>
            </div>
          </div>
          <div class="subscription-workspace">
            ${subscriptionImportMarkup}
            <p id="subscriptions-feedback" class="subscription-feedback" role="status" aria-live="polite" hidden></p>
            <div class="subscription-summary-grid">
              <div><span>活动订阅</span><strong id="subscription-summary-active">—</strong></div>
              <div><span>配置规模</span><strong id="subscription-summary-nodes">尚未读取</strong></div>
              <div><span>最近更新</span><strong id="subscription-summary-updated">—</strong></div>
              <div><span>网络安全</span><strong id="subscription-summary-safety">等待检查</strong></div>
            </div>
            <section class="subscription-list-card" aria-labelledby="subscription-list-title">
              <div class="subscription-card-heading">
                <div><h3 id="subscription-list-title">订阅列表</h3><p id="subscription-list-caption">读取中</p></div>
                <button class="button button-quiet" id="subscriptions-refresh-list">刷新列表</button>
              </div>
              <div id="subscription-manager-list" class="subscription-manager-list empty-state">还没有远程订阅</div>
            </section>
          </div>
        </article>
      </section>

      <section class="view-stack is-hidden" id="profiles-view">
        <div class="split-grid">
          <article class="panel">
            <div class="panel-heading"><div><div class="section-label">PROFILES</div><h2>配置档案</h2></div><button class="button button-quiet" id="profiles-refresh">刷新列表</button></div>
            <div id="profile-list" class="profile-list empty-state">还没有配置档案</div>
          </article>
          <article class="panel" id="profile-detail-panel">
            <div class="panel-heading"><div><div class="section-label">DETAIL</div><h2 id="profile-detail-title">选择一个配置</h2></div></div>
            <div id="profile-detail" class="empty-state">在左侧选择配置后查看版本和校验结果。</div>
          </article>
        </div>
        <article class="panel">
          <div class="panel-heading"><div><div class="section-label">LOCAL YAML</div><h2>本地 YAML 配置</h2></div><label class="button button-quiet file-button" for="yaml-file">打开 YAML</label></div>
          <input id="yaml-file" type="file" accept=".yaml,.yml,text/plain" hidden />
          <div class="editor-toolbar">
            <input id="inline-name" class="profile-name" value="本地配置" aria-label="配置名称" />
            <button class="button button-quiet" id="load-sample">载入示例</button>
            <button class="button button-quiet" id="inspect-yaml">只检查</button>
            <span class="editor-spacer"></span>
            <span id="yaml-summary" class="hint">等待配置</span>
            <button class="button button-primary" id="create-inline">创建并激活</button>
          </div>
          <textarea id="yaml-source" class="config-editor" placeholder="粘贴 Clash Meta / Mihomo YAML…" spellcheck="false"></textarea>
        </article>
      </section>

      <section class="view-stack is-hidden" id="proxies-view">
        <article class="panel proxy-groups-panel">
          <div class="panel-heading"><div><div class="section-label">PROXY GROUPS</div><h2>代理组与节点</h2></div><div class="toolbar"><button class="button button-quiet" id="proxies-current-node">当前节点</button><button class="button button-quiet" id="proxies-refresh">刷新</button></div></div>
          <div id="openai-policy-card" class="openai-policy-card"></div>
          ${openAiCostsMarkup}
          <div id="proxy-groups" class="card-list empty-state">启动 Mihomo 后查看代理组。</div>
        </article>
      </section>

      <section class="view-stack is-hidden" id="routing-view">
        ${localRoutingMarkup}
      </section>
      <section class="view-stack is-hidden" id="programs-view">
        ${programManagerMarkup}
      </section>

      <section class="view-stack is-hidden" id="rules-view">
        ${ruleManagerMarkup}
      </section>

      <section class="view-stack is-hidden" id="connections-view">
        <article class="panel">
          <div class="panel-heading"><div><div class="section-label">CONNECTIONS</div><h2>活动连接</h2></div><button class="button button-quiet" id="connections-refresh">刷新</button></div>
          <div id="connection-totals" class="summary-strip"></div>
          <div class="table-wrap"><table><thead><tr><th>目标</th><th>网络</th><th>规则</th><th>代理链</th><th>流量</th><th></th></tr></thead><tbody id="connections-body"><tr><td colspan="6">启动后加载连接</td></tr></tbody></table></div>
        </article>
      </section>

      <section class="view-stack is-hidden" id="logs-view">
        <article class="panel">
          <div class="panel-heading"><div><div class="section-label">LOGS</div><h2>应用与 Mihomo 日志</h2></div><div class="toolbar"><button class="button button-quiet" id="logs-refresh">刷新</button><button class="button button-danger" id="logs-clear">清空</button></div></div>
          <div id="log-list" class="log-list empty-state">暂无日志</div>
        </article>
      </section>

      <section class="view-stack is-hidden" id="diagnostics-view">
        <article class="panel">
          <div class="panel-heading"><div><div class="section-label">DIAGNOSTICS</div><h2>分层诊断</h2></div><button class="button button-primary" id="run-diagnostics">立即诊断</button></div>
          <div id="diagnostic-list" class="diagnostic-list"></div>
        </article>
      </section>

      <section class="view-stack is-hidden" id="settings-view">
        ${preferencesMarkup}
        ${proxyCompatibilityMarkup}
        <article class="panel tun-helper-panel">
          <div class="panel-heading">
            <div><div class="section-label">PRIVILEGED TUN</div><h2 id="tun-panel-heading">TUN 权限</h2></div>
            <span class="control-state-pill" id="tun-helper-state">正在检查</span>
          </div>
          <div class="tun-helper-summary">
            <div class="tun-helper-mark">T</div>
            <div><strong id="tun-helper-title">正在检查 TUN 权限</strong><p id="tun-helper-message">正在读取当前系统的 TUN 运行方式。</p></div>
          </div>
          <div class="about-grid tun-helper-details">
              <span id="tun-helper-protocol-label">协议版本</span><strong id="tun-helper-protocol">—</strong>
            <span>特权内核</span><strong id="tun-helper-runtime">未运行</strong>
          </div>
          <div class="toolbar tun-helper-actions">
            <button class="button button-primary" id="tun-helper-install" type="button">安装 Helper</button>
            <button class="button button-quiet" id="tun-helper-repair" type="button">修复 Helper</button>
            <button class="button button-quiet" id="tun-helper-open-settings" type="button">打开系统设置</button>
            <button class="button button-danger" id="tun-helper-uninstall" type="button">卸载 Helper</button>
          </div>
        </article>
        <article class="panel">
          <div class="section-label">VERSIONS</div>
          <div class="about-grid"><span>应用</span><strong id="about-app">—</strong><span>Mihomo</span><strong id="about-core">—</strong><span>平台</span><strong id="about-platform">—</strong></div>
        </article>
      </section>
      </div>

    </main>
      <div class="toast" id="toast" role="status" aria-live="polite"></div>
      <div class="confirmation-modal-backdrop is-hidden" id="confirmation-modal" role="presentation" aria-hidden="true">
        <section class="confirmation-modal" role="alertdialog" aria-modal="true" aria-labelledby="confirmation-title" aria-describedby="confirmation-message">
          <div class="confirmation-modal-copy">
            <h2 id="confirmation-title">确认操作</h2>
            <p id="confirmation-message"></p>
          </div>
          <div class="confirmation-modal-actions">
            <button class="button button-quiet" type="button" data-confirmation-action="cancel">取消</button>
            <button class="button button-danger" type="button" data-confirmation-action="confirm">确认</button>
          </div>
        </section>
      </div>
      <div class="node-modal-backdrop is-hidden" id="node-details-modal" role="presentation" aria-hidden="true">
        <section class="node-details-modal" role="dialog" aria-modal="true" aria-labelledby="node-details-title">
          <div id="node-details-content"></div>
        </section>
      </div>
  </div>
`;

const $ = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector);
const $$ = <T extends HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
let confirmationResolver: ((confirmed: boolean) => void) | null = null;
let confirmationReturnFocus: HTMLElement | null = null;
const viewScrollPositions: Partial<Record<ViewName, number>> = {};
let scrollRestoreFrame: number | null = null;

function syncModalScrollLock() {
  const confirmationOpen =
    $("#confirmation-modal")?.classList.contains("is-hidden") === false;
  const nodeDetailsOpen =
    $("#node-details-modal")?.classList.contains("is-hidden") === false;
  document.documentElement.classList.toggle(
    "is-modal-open",
    confirmationOpen || nodeDetailsOpen,
  );
}

function restoreViewScroll(view: ViewName) {
  continuousScrolling.cancel();
  const scroller = $("#page-scroll");
  if (!scroller) return;
  if (scrollRestoreFrame !== null) window.cancelAnimationFrame(scrollRestoreFrame);
  scrollRestoreFrame = window.requestAnimationFrame(() => {
    scrollRestoreFrame = null;
    if (store.view !== view) return;
    scroller.scrollTo({ top: viewScrollPositions[view] ?? 0, behavior: "instant" });
  });
}

function escapeHtml(value: unknown): string {
  return String(value ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

function toast(
  message: string,
  kind: "info" | "success" | "error" = "info",
  placement: "bottom-right" | "top-right" = "bottom-right",
) {
  const element = $("#toast");
  if (!element) return;
  element.textContent = message;
  element.dataset.kind = kind;
  element.dataset.placement = placement;
  element.classList.add("is-visible");
  window.setTimeout(() => element.classList.remove("is-visible"), 4200);
}

function closeConfirmation(confirmed: boolean) {
  const modal = $("#confirmation-modal");
  modal?.classList.add("is-hidden");
  modal?.setAttribute("aria-hidden", "true");
  syncModalScrollLock();
  const resolve = confirmationResolver;
  const returnFocus = confirmationReturnFocus;
  confirmationResolver = null;
  confirmationReturnFocus = null;
  resolve?.(confirmed);
  window.requestAnimationFrame(() => {
    if (!confirmationResolver && returnFocus?.isConnected) returnFocus.focus();
  });
}

function confirmAction(options: {
  title: string;
  message: string;
  confirmLabel?: string;
  returnFocus?: HTMLElement | null;
}): Promise<boolean> {
  if (confirmationResolver) closeConfirmation(false);
  const modal = $("#confirmation-modal");
  const title = $("#confirmation-title");
  const message = $("#confirmation-message");
  const confirmButton = $(
    "#confirmation-modal [data-confirmation-action='confirm']",
  ) as HTMLButtonElement | null;
  if (!modal || !title || !message || !confirmButton) return Promise.resolve(false);

  title.textContent = options.title;
  message.textContent = options.message;
  confirmButton.textContent = options.confirmLabel ?? "确认";
  const destructive = /删除|清空|恢复|停止|关闭|卸载/.test(options.title);
  confirmButton.classList.toggle("button-danger", destructive);
  confirmButton.classList.toggle("button-primary", !destructive);
  confirmationReturnFocus = options.returnFocus ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
  modal.classList.remove("is-hidden");
  modal.setAttribute("aria-hidden", "false");
  syncModalScrollLock();

  return new Promise((resolve) => {
    confirmationResolver = resolve;
    window.requestAnimationFrame(() => {
      if (confirmationResolver === resolve) $("#confirmation-modal [data-confirmation-action='cancel']")?.focus();
    });
  });
}

async function action<T>(label: string, operation: () => Promise<T>): Promise<T | null> {
  try {
    const result = await operation();
    if (label) toast(label, "success");
    return result;
  } catch (error) {
    toast(errorMessage(error), "error");
    return null;
  }
}

function formatBytes(value: unknown): string {
  const number = Number(value ?? 0);
  if (!Number.isFinite(number) || number <= 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB"];
  const index = Math.min(Math.floor(Math.log(number) / Math.log(1024)), units.length - 1);
  return `${(number / 1024 ** index).toFixed(index ? 1 : 0)} ${units[index]}`;
}

function formatTrafficRate(bytesPerSecond: number | null | undefined): string {
  const value = Math.max(0, Number(bytesPerSecond ?? 0));
  const units = ["B/s", "KB/s", "MB/s", "GB/s"];
  let scaled = value;
  let index = 0;
  while (scaled >= 1024 && index < units.length - 1) {
    scaled /= 1024;
    index += 1;
  }
  const digits = index === 0 || scaled >= 100 ? 0 : 1;
  return `${scaled.toFixed(digits)} ${units[index]}`;
}

function renderGlobalTraffic() {
  const container = $("#global-traffic-compact");
  if (!container) return;
  const enabled = store.settings?.showGlobalTraffic ?? store.globalTraffic?.enabled ?? true;
  container.classList.toggle("is-hidden", !enabled);
  if (!enabled) return;
  const upload = store.globalTraffic ? formatTrafficRate(store.globalTraffic.uploadBytesPerSecond) : "—";
  const download = store.globalTraffic ? formatTrafficRate(store.globalTraffic.downloadBytesPerSecond) : "—";
  $("#global-upload-rate")!.textContent = upload;
  $("#global-download-rate")!.textContent = download;
  const interfaces = store.globalTraffic?.interfaces ?? [];
  container.title = interfaces.length
    ? `系统全局流量 · ${interfaces.join(", ")}`
    : "系统全局流量";
}

function formatDate(value: string | null | undefined): string {
  return value ? new Date(value).toLocaleString() : "—";
}

function formatPolicyDate(value: string | null | undefined): string {
  if (!value) return "—";
  const date = new Date(value);
  const now = new Date();
  const sameDay =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();
  return sameDay
    ? `今天 ${date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`
    : date.toLocaleString([], {
        month: "numeric",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
}

function modeLabel(mode: NetworkMode | undefined): string {
  return mode === "system_proxy" ? "System Proxy" : mode === "tun" ? "TUN" : "Manual";
}

function phaseLabel(phase: string | undefined): string {
  const labels: Record<string, string> = {
    uninitialized: "未初始化",
    stopped: "已停止",
    validating: "校验中",
    starting: "启动中",
    running: "运行中",
    stopping: "停止中",
    crashed: "异常退出",
    recovering: "恢复中",
  };
  return labels[phase ?? "stopped"] ?? phase ?? "已停止";
}

async function refreshBase() {
  const requestedBaseRead = ++baseReadSequence;
  const requestedThemeRevision = themeController.mutationRevision;
  const requestedRuntimeRevision = runtimeMutationRevision;
  const requestedDuringRuntimeWrite = runtimeActionInFlight || networkModeSwitching || settingsSaving;
  const result = await action("", async () => {
    const [
      appInfo,
      settings,
      binary,
      runtime,
      systemProxy,
      tunHelper,
      profiles,
      subscriptions,
      activeProfile,
      openAiTask,
      globalTraffic,
    ] =
      await Promise.all([
        api.appInfo(),
        api.settings(),
        api.binary(),
        api.runtime(),
        api.systemProxy(),
        api.tunHelperStatus(),
        api.profiles(),
        api.subscriptions(),
        api.activeProfile(),
        api.openAiPolicyTask(),
        api.globalTraffic(),
      ]);
    // A read started before/during a runtime or settings write must not put
    // the old network mode and runtime status back into the toolbar.
    if (requestedBaseRead !== baseReadSequence || requestedDuringRuntimeWrite || requestedRuntimeRevision !== runtimeMutationRevision || runtimeActionInFlight || networkModeSwitching || settingsSaving) return;
    if (!themeController.sync(settings.theme, requestedThemeRevision)) {
      settings.theme = themeController.snapshot.preference;
    }
    if (activeProfile?.profile.id !== store.activeProfile?.profile.id || activeProfile?.profile.activeRevisionId !== store.activeProfile?.profile.activeRevisionId || runtime?.phase !== "running") {
      proxyReadSequence++;
      store.proxies = null;
      overviewNodeDetails = {};
    }
    Object.assign(store, {
      appInfo,
      settings,
      binary,
      runtime,
      systemProxy,
      tunHelper,
      profiles,
      subscriptions,
      activeProfile,
      openAiTask,
      globalTraffic,
    });
    return true;
  });
  if (result !== true) return false;
  renderHeader();
  renderOverview();
  renderProfiles();
  renderSubscriptions();
  renderSettings();
  renderOpenAiPolicy();
  renderGlobalTraffic();
  void openAiCosts.refresh();
  scheduleAutomaticUpdateCheck();
  void refreshSessionResume();
  void refreshProxies(true);
  return true;
}

function renderSessionResume() {
  const presentation = sessionResumePresentation(sessionResumeStatus);
  $("#session-resume-notice")!.classList.toggle("is-hidden", !presentation.visible);
  $("#session-resume-title")!.textContent = presentation.title;
  $("#session-resume-message")!.textContent = presentation.message;
  $("#session-resume-status")!.textContent = sessionResumeStatus?.message ?? "";
  if (store.settings) {
    const mode = startupModeDraft ?? startupModeFromSettings(store.settings);
    $("#session-resume-help")!.textContent = (startupModeDraft ? "待保存：" : "") + startupModeHelp(store.settings, mode);
  }
  const registration = startupRegistrationPresentation(startupStatus);
  $("#startup-registration-status")!.textContent = registration.text;
  $("#startup-registration-status")!.classList.toggle("startup-registration-issue", registration.issue);
}

async function refreshSessionResume(force = false) {
  if (sessionResumeReadBusy) {
    if (force) sessionResumeReadAgain = true;
    return;
  }
  sessionResumeReadBusy = true;
  const revision = sessionResumeRevision;
  try {
    const status = await api.sessionResume();
    const registration = store.view === "settings" || startupStatus === null || force
      ? await api.startupStatus().catch(() => null)
      : undefined;
    if (revision !== sessionResumeRevision) return;
    if (registration !== undefined) startupStatus = registration;
    const wasBusy = sessionResumePresentation(sessionResumeStatus).busy;
    sessionResumeStatus = status;
    renderSessionResume();
    renderHeader();
    if (wasBusy && !sessionResumePresentation(status).busy) void refreshBase();
  } catch {
    // Status reads never start a core or guess whether restoration succeeded.
    if (revision === sessionResumeRevision) $("#session-resume-status")!.textContent = "恢复状态暂时无法读取，可点击刷新重试。";
  } finally {
    sessionResumeReadBusy = false;
    if (sessionResumeReadAgain) {
      sessionResumeReadAgain = false;
      void refreshSessionResume();
    }
  }
}

function renderHeader() {
  const running = store.runtime?.phase === "running";
  const mode = store.settings?.networkMode;
  const controlsBusy = networkModeSwitching || runtimeActionInFlight;
  ($("#global-refresh") as HTMLButtonElement).disabled = controlsBusy;
  const systemProxyActive = Boolean(
    running && mode === "system_proxy" && store.systemProxy?.active,
  );
  const tunActive = Boolean(running && mode === "tun");
  $("#platform-chip")!.textContent = store.appInfo
    ? `${store.appInfo.targetOs} · ${store.appInfo.targetArch}`
    : "—";
  $("#application-runtime-state")!.textContent = networkModeSwitching
    ? "正在切换网络"
    : runtimeActionInFlight
      ? "正在处理"
      : phaseLabel(store.runtime?.phase);
  $("#application-profile-state")!.textContent =
    store.activeProfile?.profile.displayName ?? "未选择订阅";
  $("#application-mode-state")!.textContent = modeLabel(mode);
  $("#application-status-dot")!.classList.toggle("is-running", running);
  $("#application-status-dot")!.classList.toggle("is-busy", controlsBusy);
  $("#sidebar-version")!.textContent = store.appInfo ? `v${store.appInfo.version}` : "";
  $("#page-title")!.textContent = NAV_ITEMS.find((item) => item.id === store.view)!.label;
  document.documentElement.dataset.runtimeRunning = String(running);
  $("#sidebar-status")!.textContent = !store.runtime ? "代理核心 · 未读取" : running ? "代理核心 · 运行中" : `代理核心 · ${phaseLabel(store.runtime.phase)}`;
  $("#sidebar-status-dot")!.classList.toggle("is-running", running);
  renderGlobalTraffic();
  const systemProxyButton = $("#global-system-proxy") as HTMLButtonElement;
  const tunButton = $("#global-tun") as HTMLButtonElement;
  systemProxyButton.classList.toggle("is-active", systemProxyActive);
  systemProxyButton.setAttribute("aria-pressed", String(systemProxyActive));
  systemProxyButton.disabled = controlsBusy || !store.settings || !store.activeProfile;
  tunButton.classList.toggle("is-active", tunActive);
  tunButton.setAttribute("aria-pressed", String(tunActive));
  tunButton.title = store.tunHelper?.message ?? "TUN 使用最小权限 Helper 接管系统流量";
  tunButton.disabled = controlsBusy || !store.settings || !store.activeProfile;
  ($("#global-start") as HTMLButtonElement).disabled = !canStartRuntime(store.runtime) || !store.settings || !store.activeProfile || controlsBusy;
  ($("#global-stop") as HTMLButtonElement).disabled = !canStopSession(store.runtime?.phase, sessionResumeStatus, startupStatus) || controlsBusy;
  $("#about-app")!.textContent = store.appInfo?.version ?? "—";
  $("#about-core")!.textContent = store.binary?.version ?? "未找到";
  $("#about-platform")!.textContent = store.appInfo
    ? `${store.appInfo.targetOs} ${store.appInfo.targetArch}`
    : "—";
}

function renderOverview() {
  $("#overview-subscription-guide")!.hidden = !store.appInfo || Boolean(store.activeProfile);
  const runtime = store.runtime;
  const running = runtime?.phase === "running";
  $("#connection-state")!.textContent = phaseLabel(runtime?.phase);
  $("#connection-message")!.textContent = runtime?.message ?? "等待运行时状态";
  $("#connection-dot")!.classList.toggle("is-running", running);
  $("#runtime-version")!.textContent = runtime?.version ?? store.binary?.version ?? "未找到";
  $("#runtime-pid")!.textContent = runtime?.pid ? String(runtime.pid) : "—";
  $("#runtime-config")!.textContent = runtime?.configPath ?? "—";
  $("#overview-mode")!.textContent = modeLabel(store.settings?.networkMode);
  $("#overview-endpoint")!.textContent = store.settings
    ? `127.0.0.1:${store.settings.mixedPort}`
    : "—";
  $("#overview-system-proxy")!.textContent = store.systemProxy?.active ? "已接管" : "未接管";
  $("#overview-profile")!.textContent = store.activeProfile?.profile.displayName ?? "未选择";
  $("#control-profile-caption")!.textContent = store.activeProfile
    ? `当前订阅：${store.activeProfile.profile.displayName}`
    : "当前订阅：未选择";
  $("#control-runtime-pill")!.textContent = phaseLabel(runtime?.phase);
  $("#control-runtime-pill")!.classList.toggle("is-running", running);
  const systemProxySwitch = $("#home-system-proxy") as HTMLInputElement;
  const tunSwitch = $("#home-tun") as HTMLInputElement;
  systemProxySwitch.checked = store.settings?.networkMode === "system_proxy";
  tunSwitch.checked = store.settings?.networkMode === "tun";
  systemProxySwitch.disabled = networkModeSwitching || runtimeActionInFlight || !store.settings || !store.activeProfile;
  tunSwitch.disabled = networkModeSwitching || runtimeActionInFlight || !store.settings || !store.activeProfile;
  const routingMode = store.activeProfile?.profile.routingMode ?? "rule";
  $$("#home-routing-mode button").forEach((button) => {
    button.classList.toggle("is-active", button.dataset.routingMode === routingMode);
    (button as HTMLButtonElement).disabled = !store.activeProfile;
  });
  $("#control-hint")!.textContent = store.activeProfile
    ? `${store.activeProfile.profile.displayName} · ${modeLabel(store.settings?.networkMode)} · ${routingMode === "global" ? "全局" : routingMode === "direct" ? "直连" : "规则"}`
    : "先创建并激活一个订阅，再使用网络控制中心。";
  $("#metric-profiles")!.textContent = String(store.profiles.length);
  $("#metric-nodes")!.textContent = store.activeProfile?.summary
    ? String(store.activeProfile.summary.nodeCount + store.activeProfile.summary.proxyProviderCount)
    : "—";
  $("#metric-rules")!.textContent = store.activeProfile?.summary
    ? String(store.activeProfile.summary.ruleCount + store.activeProfile.summary.ruleProviderCount)
    : "—";
  $("#metric-phase")!.textContent = phaseLabel(runtime?.phase);
  renderOverviewNodes();
}

function profileSourceLabel(profile: ProfileRecord): string {
  if (profile.source.type === "remote_subscription") {
    return profile.source.host;
  }
  return profile.source.type === "local_file" ? "本地文件" : "内联配置";
}

function renderSubscriptions() {
  const list = $("#subscription-manager-list");
  if (!list) return;
  const subscriptions = store.subscriptions;
  if (store.appInfo && !subscriptionFormInitialized) {
    subscriptionFormInitialized = true;
    if (!subscriptions.length) openSubscriptionForm(false);
  }
  if (!subscriptionActivationTouched && !subscriptionImporting) {
    ($("#managed-subscription-activate") as HTMLInputElement).checked = Boolean(store.appInfo) && !store.activeProfile;
  }
  renderSubscriptionActivationHint();
  const active = subscriptions.find((subscription) => subscription.active);
  const totalNodes = subscriptions.reduce(
    (total, subscription) =>
      total +
      (subscription.summary
        ? subscription.summary.nodeCount
        : 0),
    0,
  );
  const updatedTimes = subscriptions
    .map((subscription) => subscription.latestFetchedAt)
    .filter((value): value is string => Boolean(value))
    .sort();
  const latest = updatedTimes[updatedTimes.length - 1];
  $("#subscription-summary-active")!.textContent = active?.profile.displayName ?? "未选择";
  const providerCount = subscriptions.reduce((total, subscription) => total + (subscription.summary?.proxyProviderCount ?? 0), 0);
  $("#subscription-summary-nodes")!.textContent = `${totalNodes} 节点 · ${providerCount} 提供器`;
  $("#subscription-summary-updated")!.textContent = formatPolicyDate(latest);
  $("#subscription-summary-safety")!.textContent = store.networkSafety
    ? store.networkSafety.success
      ? store.networkSafety.warnings?.length ? "联网正常 · 部分检测未通过" : "代理预检通过"
      : "代理预检失败"
    : store.runtime?.phase === "running"
      ? "等待检查"
      : "内核未运行";
  $("#subscription-list-caption")!.textContent = `${subscriptions.length} 个订阅 · ${subscriptions.filter((subscription) => subscription.profile.openaiPolicy.autoMaintain).length} 个自动维护`;

  if (!subscriptions.length) {
    list.className = "subscription-manager-list empty-state";
    list.textContent = "还没有远程订阅，可在顶部添加。";
    return;
  }
  list.className = "subscription-manager-list";
  const expanded = new Set(Array.from(list.querySelectorAll<HTMLDetailsElement>(".subscription-more[open]"))
    .map((details) => details.closest<HTMLElement>("[data-subscription-id]")?.dataset.subscriptionId));
  list.innerHTML = subscriptions.map((subscription) => subscriptionCardMarkup(subscription, store.openAiTask, Date.now(), subscriptionRefreshing.has(subscription.profile.id))).join("");
  for (const details of list.querySelectorAll<HTMLDetailsElement>(".subscription-more")) {
    details.open = expanded.has(details.closest<HTMLElement>("[data-subscription-id]")?.dataset.subscriptionId);
  }
  for (const card of list.querySelectorAll<HTMLElement>("[data-subscription-id]")) {
    card.tabIndex = -1;
    card.classList.toggle("is-imported", card.dataset.subscriptionId === highlightedSubscriptionId);
  }
}

function renderSubscriptionActivationHint() {
  const selected = ($("#managed-subscription-activate") as HTMLInputElement).checked;
  $("#managed-subscription-activate-hint")!.textContent = !selected
    ? "仅保存，不替换当前配置；需要时再点击卡片上的「选用」。"
    : store.runtime?.phase === "running"
      ? "校验成功后切换运行中的配置，可能影响现有连接；不会改变系统代理或 TUN 模式。"
      : "校验成功后设为当前配置，不会自动启动核心或开启系统代理。";
}

function openSubscriptionForm(focus = true) {
  if (subscriptionImporting) return;
  if (!subscriptionDraftDirty) {
    subscriptionActivationTouched = false;
    ($("#managed-subscription-activate") as HTMLInputElement).checked = Boolean(store.appInfo) && !store.activeProfile;
  }
  $("#managed-subscription-panel")!.hidden = false;
  $("#subscriptions-add")!.setAttribute("aria-expanded", "true");
  renderSubscriptionActivationHint();
  if (focus && store.view === "subscriptions") $("#managed-subscription-url")!.focus();
}

function closeSubscriptionForm(returnFocus = true) {
  $("#managed-subscription-panel")!.hidden = true;
  $("#subscriptions-add")!.setAttribute("aria-expanded", "false");
  if (returnFocus && store.view === "subscriptions") $("#subscriptions-add")!.focus();
}

function renderProfiles() {
  const list = $("#profile-list");
  if (!list) return;
  if (!store.profiles.length) {
    list.className = "profile-list empty-state";
    list.textContent = "还没有配置档案";
  } else {
    list.className = "profile-list";
    list.innerHTML = store.profiles
      .map((profile) => {
        const active = store.activeProfile?.profile.id === profile.id;
        const selected = store.selectedProfile?.profile.id === profile.id;
        return `
          <button class="profile-row ${active ? "is-active" : ""} ${selected ? "is-selected" : ""}" data-profile-id="${profile.id}">
            <span class="profile-status"></span>
            <span><strong>${escapeHtml(profile.displayName)}</strong><small>${escapeHtml(profileSourceLabel(profile))}</small></span>
            <time>${escapeHtml(formatDate(profile.updatedAt))}</time>
          </button>
        `;
      })
      .join("");
  }
  renderProfileDetails();
}

function renderProfileDetails() {
  const details = store.selectedProfile ?? store.activeProfile;
  const title = $("#profile-detail-title");
  const container = $("#profile-detail");
  if (!title || !container) return;
  if (!details) {
    title.textContent = "选择一个配置";
    container.className = "empty-state";
    container.textContent = "在左侧选择配置后查看版本和校验结果。";
    return;
  }
  const { profile, summary, revisions } = details;
  title.textContent = profile.displayName;
  container.className = "profile-detail";
  container.innerHTML = `
    <div class="detail-grid">
      <span>来源</span><strong>${escapeHtml(profileSourceLabel(profile))}</strong>
      <span>路由模式</span><strong>${profile.routingMode === "global" ? "全局" : profile.routingMode === "direct" ? "直连" : "规则"}</strong>
      <span>节点</span><strong>${summary ? summary.nodeCount + summary.proxyProviderCount : "—"}</strong>
      <span>代理组</span><strong>${summary?.proxyGroupCount ?? "—"}</strong>
      <span>规则</span><strong>${summary ? summary.ruleCount + summary.ruleProviderCount : "—"}</strong>
      <span>OpenAI 灾备</span><strong>${profile.openaiPolicy.enabled ? `${profile.openaiPolicy.selectedNodes.length} 个节点 · 自动维护${profile.openaiPolicy.autoMaintain ? "开启" : "关闭"}` : "未启用"}</strong>
    </div>
    <div class="profile-actions">
      <button class="button button-primary" data-profile-action="activate" data-profile-id="${profile.id}">激活</button>
      <button class="button button-quiet" data-profile-action="refresh" data-profile-id="${profile.id}" ${profile.source.type !== "remote_subscription" ? "disabled" : ""}>更新</button>
      <button class="button button-quiet" data-profile-action="rollback" data-profile-id="${profile.id}" ${!profile.lastKnownGoodRevisionId ? "disabled" : ""}>回滚</button>
      <button class="button button-danger" data-profile-action="delete" data-profile-id="${profile.id}" ${store.activeProfile?.profile.id === profile.id ? "disabled" : ""}>删除</button>
    </div>
    <h3>版本记录</h3>
    <div class="revision-list">
      ${revisions.map((revision) => `
        <button class="revision-row ${revision.id === profile.activeRevisionId ? "is-active" : ""}" data-profile-action="revision" data-profile-id="${profile.id}" data-revision-id="${revision.id}">
          <span>${escapeHtml(revisionLabel(revision))}</span>
          <small>${revision.validation.nativeCoreValidated ? "core validated" : "not validated"} · ${escapeHtml(revision.effectiveSha256.slice(0, 12))}</small>
        </button>
      `).join("") || '<div class="empty-state">暂无版本</div>'}
    </div>
    ${summary?.warnings.length ? `<div class="warning-box">${summary.warnings.map(escapeHtml).join("<br>")}</div>` : ""}
  `;
}

function renderSettings() {
  if (!store.settings) return;
  themeController.sync(store.settings.theme);
  ($("#settings-mode") as HTMLSelectElement).value = store.settings.networkMode;
  document.querySelectorAll<HTMLInputElement>('[name="settings-network-mode"]').forEach((radio) => { radio.checked = radio.value === store.settings!.networkMode; });
  ($("#settings-mixed-port") as HTMLInputElement).value = String(store.settings.mixedPort);
  ($("#settings-controller-port") as HTMLInputElement).value = String(store.settings.controllerPort);
  const startupSelect = $("#settings-startup-mode") as HTMLSelectElement;
  const savedStartupMode = startupModeFromSettings(store.settings);
  const legacyOption = startupSelect.querySelector<HTMLOptionElement>('[value="custom"]')!;
  legacyOption.hidden = savedStartupMode !== "custom";
  legacyOption.disabled = savedStartupMode !== "custom";
  startupSelect.value = startupModeDraft ?? savedStartupMode;
  ($("#settings-global-traffic") as HTMLInputElement).checked =
    store.settings.showGlobalTraffic;
  ($("#settings-auto-check-updates") as HTMLInputElement).checked =
    store.settings.autoCheckUpdates;
  ($("#settings-auto-download-updates") as HTMLInputElement).checked = store.settings.autoDownloadUpdates;
  ($("#settings-update-source") as HTMLSelectElement).value = store.settings.updateSource;
  ($("#settings-retention") as HTMLInputElement).value = String(
    store.settings.diagnosticsRetentionDays,
  );
  ($("#settings-app-log-retention") as HTMLInputElement).value = String(store.settings.appLogRetentionDays);
  renderGlobalTraffic();
  renderAppUpdate();
  renderTunHelper();
  renderSessionResume();
}

function renderAppUpdate() {
  const presentation = describeAppUpdate(appUpdateStatus);
  const state = $("#app-update-state")!;
  state.textContent = presentation.badge;
  state.classList.toggle("is-hidden", appUpdateStatus.phase === "idle");
  $("#app-update-feedback")!.classList.toggle("is-hidden", appUpdateStatus.phase === "idle");
  state.classList.toggle("is-running", ["current", "ready"].includes(presentation.state));
  state.classList.toggle("is-warning", presentation.state === "available");
  state.classList.toggle("is-error", presentation.state === "failed");
  $("#app-update-title")!.textContent = presentation.label;
  $("#app-update-message")!.textContent = presentation.detail;
  $("#app-update-current")!.textContent =
    store.appInfo?.version ?? store.appUpdate?.currentVersion ?? "—";
  $("#app-update-latest")!.textContent = store.appUpdate?.latestVersion ?? "尚未检查";
  $("#app-update-date")!.textContent = formatDate(store.appUpdate?.publishedAt);
  $("#app-update-source")!.textContent = store.appUpdate ? (store.appUpdate.source === "github" ? "GitHub" : "Gitee") : "—";
  const channels = $("#app-update-channels")!;
  channels.replaceChildren(...(store.appUpdate?.channels ?? []).map((channel) => {
    const item = document.createElement("li");
    item.classList.toggle("is-warning", Boolean(channel.error));
    item.textContent = `${channel.source === "github" ? "GitHub" : "Gitee"} · ${channel.error ?? channel.version ?? "尚未检查"}`;
    return item;
  }));
  channels.classList.toggle("is-hidden", !channels.childElementCount);
  const busy = presentation.busy || appUpdateActionBusy || appUpdateChecking;
  $("#app-update-panel")!.setAttribute("aria-busy", String(busy));
  for (const control of document.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>("#update-preferences-form input, #update-preferences-form select, #update-preferences-form button")) control.disabled = busy;

  const checkButton = $("#app-update-check") as HTMLButtonElement;
  checkButton.disabled = busy;
  checkButton.textContent = appUpdateChecking ? "正在检查…" : "检查更新";
  const openButton = $("#app-update-open") as HTMLButtonElement;
  openButton.classList.toggle("is-hidden", !presentation.canOpen);
  openButton.disabled = !presentation.canOpen || busy;
  openButton.title = presentation.canOpen ? store.appUpdate?.releaseUrl ?? "" : "";
  for (const [id, visible] of [["download", presentation.canDownload], ["install", presentation.canInstall], ["cancel", presentation.state === "downloading"]] as const) {
    const button = $(`#app-update-${id}`) as HTMLButtonElement;
    button.classList.toggle("is-hidden", !visible);
    button.disabled = !visible || (id !== "cancel" && busy);
  }
  $("#app-update-progress")!.classList.toggle("is-hidden", !["downloading", "ready"].includes(presentation.state));
  ($("#app-update-progress-bar") as HTMLProgressElement).value = presentation.progress;
  $("#app-update-progress-text")!.textContent = `${(appUpdateStatus.downloadedBytes / 1048576).toFixed(1)} / ${(appUpdateStatus.totalBytes / 1048576).toFixed(1)} MiB · ${Math.floor(presentation.progress)}%`;

  const notes = $("#app-update-notes") as HTMLDetailsElement;
  const showNotes = Boolean(store.appUpdate?.available && store.appUpdate.notes);
  notes.classList.toggle("is-hidden", !showNotes);
  if (!showNotes) notes.open = false;
  $("#app-update-notes-content")!.textContent = showNotes ? store.appUpdate!.notes : "";
}

function acceptAppUpdate(status: AppUpdateStatus) {
  appUpdateStatus = status;
  store.appUpdate = status.info;
  appUpdateError = status.error;
  renderAppUpdate();
}

async function checkForAppUpdate(silent: boolean) {
  if (appUpdateChecking || appUpdateActionBusy || describeAppUpdate(appUpdateStatus).busy || appUpdateStatus.phase === "ready" && silent) return;
  updateCheckAttemptedThisSession = true;
  appUpdateChecking = true;
  appUpdateError = null;
  appUpdateStatus = { ...appUpdateStatus, phase: "checking", error: null };
  renderAppUpdate();
  try {
    const result = await api.checkAppUpdate();
    acceptAppUpdate(result);
    if (result.info?.available) {
      toast(`发现 Serylane 新版本 ${result.info.latestVersion}`, "info");
    } else if (!silent) {
      toast(describeAppUpdate(result).label, "info");
    }
  } catch (error) {
    appUpdateError = errorMessage(error);
    appUpdateStatus = { ...appUpdateStatus, phase: "failed", info: null, error: appUpdateError };
    store.appUpdate = null;
    if (!silent) toast(appUpdateError, "error");
  } finally {
    appUpdateChecking = false;
    renderAppUpdate();
  }
  if (appUpdateStatus.phase === "available" && store.settings?.autoDownloadUpdates) await downloadAppUpdate();
}

async function downloadAppUpdate() {
  if (appUpdateActionBusy || appUpdateChecking || !describeAppUpdate(appUpdateStatus).canDownload || !store.appUpdate) return;
  const version = store.appUpdate.latestVersion;
  const operationId = ++appUpdateOperationId;
  appUpdateActionBusy = true;
  appUpdateStatus = { ...appUpdateStatus, phase: "downloading", error: null, downloadedBytes: 0 };
  renderAppUpdate();
  const poll = window.setInterval(async () => {
    if (appUpdatePollBusy) return;
    appUpdatePollBusy = true;
    try {
      const status = await api.appUpdateStatus();
      // A late polling reply must not overwrite a completed command response.
      if (operationId === appUpdateOperationId && appUpdateActionBusy && appUpdateStatus.phase === "downloading" && status.phase === "downloading") acceptAppUpdate(status);
    } catch { /* The command below reports the final native error. */ }
    finally { appUpdatePollBusy = false; }
  }, 400);
  try {
    acceptAppUpdate(await api.downloadAppUpdate(version));
    toast("更新包已验证；可在方便时确认安装", "success");
  } catch (error) {
    try { acceptAppUpdate(await api.appUpdateStatus()); }
    catch { acceptAppUpdate({ ...appUpdateStatus, phase: "failed", error: errorMessage(error) }); }
    if (appUpdateStatus.phase !== "cancelled") toast(errorMessage(error), "error");
  } finally {
    window.clearInterval(poll);
    appUpdateActionBusy = false;
    renderAppUpdate();
  }
}

function scheduleAutomaticUpdateCheck() {
  if (
    automaticUpdateCheckScheduled ||
    !store.settings?.autoCheckUpdates
  ) {
    return;
  }
  automaticUpdateCheckScheduled = true;
  window.setTimeout(() => {
    if (updateCheckAttemptedThisSession || !store.settings?.autoCheckUpdates) return;
    void checkForAppUpdate(true);
  }, 18_000);
  window.setInterval(() => {
    if (store.settings?.autoCheckUpdates) void checkForAppUpdate(true);
  }, 6 * 60 * 60 * 1_000);
}

function tunHelperStateLabel(state: TunHelperStatus["state"] | undefined): string {
  const labels: Record<TunHelperStatus["state"], string> = {
    unsupported: "当前不可用",
    not_installed: "未安装",
    requires_approval: "等待批准",
    ready: "已就绪",
    outdated: "需要更新",
    unreachable: "连接异常",
  };
  return state ? labels[state] : "正在检查";
}

function renderTunHelper() {
  const helper = store.tunHelper;
  const state = helper?.state;
  const windows = store.appInfo?.targetOs === "windows";
  const stateElement = $("#tun-helper-state")!;
  stateElement.textContent = windows && state === "requires_approval" ? "需要管理员权限" : tunHelperStateLabel(state);
  stateElement.classList.toggle("is-running", state === "ready");
  stateElement.classList.toggle(
    "is-warning",
    state === "requires_approval" || state === "outdated" || state === "unreachable",
  );
  $("#tun-panel-heading")!.textContent = windows ? "Windows TUN 权限" : "TUN 权限服务";
  $("#network-mode-help")!.textContent = windows
    ? "Windows TUN 需要先从托盘退出应用，再右键以管理员身份运行。关闭窗口会保留托盘运行；停止内核或退出应用才会关闭 TUN。使用系统代理前，请先关闭其他代理客户端的系统代理。"
    : "切换网络模式和端口前需要先停止 Mihomo。首次开启 TUN 会先安装最小权限 Helper，并在旧网络模式仍运行时完成预检。";
  $("#tun-helper-title")!.textContent = windows
    ? state === "ready" ? "管理员会话已就绪" : "以管理员身份运行后可开启 TUN"
    : state === "ready" ? "最小权限 TUN Helper 已就绪" : tunHelperStateLabel(state);
  $("#tun-helper-message")!.textContent =
    helper?.message ?? "正在读取当前系统的 TUN 运行方式。";
  $("#tun-helper-protocol-label")!.textContent = windows ? "运行方式" : "协议版本";
  $("#tun-helper-protocol")!.textContent = windows ? "管理员会话（无常驻服务）" : helper?.protocolVersion
    ? `v${helper.protocolVersion}`
    : "—";
  $("#tun-helper-runtime")!.textContent = helper?.runtimeRunning
    ? `运行中 · PID ${helper.runtimePid ?? "—"}`
    : "未运行";
  const running = store.runtime?.phase === "running";
  const install = $("#tun-helper-install") as HTMLButtonElement;
  const repair = $("#tun-helper-repair") as HTMLButtonElement;
  const open = $("#tun-helper-open-settings") as HTMLButtonElement;
  const uninstall = $("#tun-helper-uninstall") as HTMLButtonElement;
  install.classList.toggle("is-hidden", windows || state !== "not_installed");
  repair.classList.toggle(
    "is-hidden",
    windows || (state !== "outdated" && state !== "unreachable"),
  );
  open.classList.toggle("is-hidden", windows || state !== "requires_approval");
  uninstall.classList.toggle(
    "is-hidden",
    windows || !state || state === "unsupported" || state === "not_installed",
  );
  install.disabled = networkModeSwitching;
  repair.disabled = networkModeSwitching || running;
  open.disabled = networkModeSwitching;
  uninstall.disabled = networkModeSwitching || running;
}

const systemAppearance = window.matchMedia("(prefers-color-scheme: dark)");
const themeController = new ThemeController({
  systemDark: () => systemAppearance.matches,
  persist: async (theme) => {
    const settings = await api.setTheme(theme);
    if (!isThemePreference(settings.theme)) throw new Error("保存主题返回了无效设置");
    // A theme response must not overwrite unrelated settings updated meanwhile.
    store.settings = store.settings ? { ...store.settings, theme: settings.theme } : settings;
    return settings.theme;
  },
  render: renderAppearance,
});

function renderAppearance(snapshot: ThemeSnapshot) {
  const root = document.documentElement;
  root.dataset.theme = snapshot.resolved;
  root.dataset.themePreference = snapshot.selected;
  root.style.colorScheme = themeColorScheme(snapshot.resolved);
  const meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]');
  if (meta) meta.content = { light: "#f5f5f7", dark: "#1e1f22", purple: "#191222" }[snapshot.resolved];
  const busy = snapshot.saving || settingsSaving;
  document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]").forEach((button) => {
    const selected = button.dataset.themeChoice === snapshot.selected;
    button.setAttribute("aria-checked", String(selected));
    button.tabIndex = selected ? 0 : -1;
    // Keep focus on a radio while a write is pending; event handlers reject re-entry.
    button.disabled = !store.settings;
    button.setAttribute("aria-disabled", String(busy || !store.settings));
  });
  $(".appearance-grid")!.setAttribute("aria-busy", String(snapshot.saving));
  const submit = document.querySelector<HTMLButtonElement>('#settings-form button[type="submit"]');
  if (submit) submit.disabled = busy || runtimeActionInFlight || networkModeSwitching;
  $("#appearance-status")!.textContent = snapshot.saving
    ? "正在保存外观…"
    : appearanceFeedback || (snapshot.selected === "system"
      ? `正在跟随系统 · 当前为${snapshot.resolved === "dark" ? "深色" : "浅色"}`
      : `${THEME_OPTIONS.find((option) => option.id === snapshot.selected)!.label}主题 · 自动保存，不影响代理状态`);
}

async function selectTheme(preference: ThemePreference) {
  if (!store.settings || settingsSaving || themeController.snapshot.saving) return;
  appearanceFeedback = "";
  try {
    await themeController.select(preference);
    themeController.refresh();
  } catch (error) {
    appearanceFeedback = "保存失败，已恢复此前外观，请重试。";
    toast(errorMessage(error), "error");
    themeController.refresh();
  }
}

systemAppearance.addEventListener("change", () => {
  appearanceFeedback = "";
  themeController.refresh();
});

document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]").forEach((button) => {
  button.addEventListener("click", () => {
    const preference = button.dataset.themeChoice;
    if (isThemePreference(preference)) void selectTheme(preference);
  });
  button.addEventListener("keydown", (event) => {
    const buttons = [...document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]")];
    const current = buttons.indexOf(button);
    let next: number;
    if (event.key === "ArrowRight" || event.key === "ArrowDown") next = (current + 1) % buttons.length;
    else if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = (current + buttons.length - 1) % buttons.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = buttons.length - 1;
    else return;
    event.preventDefault();
    if (settingsSaving || themeController.snapshot.saving || buttons[next].disabled) return;
    buttons[next].focus();
    const preference = buttons[next].dataset.themeChoice;
    if (isThemePreference(preference)) void selectTheme(preference);
  });
});
themeController.refresh();

async function refreshRuntimeOnly(allowDuringRuntimeAction = false) {
  const requestedDuringWrite = runtimeActionInFlight || networkModeSwitching || settingsSaving;
  if (requestedDuringWrite && !allowDuringRuntimeAction) return;
  const requestedRevision = runtimeMutationRevision;
  const requestedSequence = ++runtimeReadSequence;
  const [runtime, systemProxy, tunHelper] = await Promise.all([api.runtime(), api.systemProxy(), api.tunHelperStatus()]);
  if (requestedRevision !== runtimeMutationRevision || requestedSequence !== runtimeReadSequence
    || (!allowDuringRuntimeAction && (runtimeActionInFlight || networkModeSwitching || settingsSaving))) return;
  store.runtime = runtime;
  store.systemProxy = systemProxy;
  store.tunHelper = tunHelper;
  if (runtime.phase !== "running") {
    store.networkSafety = null;
    store.proxies = null;
    overviewNodeDetails = {};
    proxyReadSequence++;
    renderProxies();
  }
  renderTunHelper();
  renderHeader();
  renderOverview();
  renderSubscriptions();
}

async function startRuntime(mode: RuntimeStartMode) {
  const result = await startRuntimeInMode(mode, {
    state: () => ({ settings: store.settings, runtime: store.runtime, systemProxyActive: Boolean(store.systemProxy?.active), hasProfile: Boolean(store.activeProfile), busy: runtimeActionInFlight || networkModeSwitching || settingsSaving }),
    setBusy: (busy) => { runtimeMutationRevision++; runtimeActionInFlight = busy; renderHeader(); renderOverview(); renderAppearance(themeController.snapshot); },
    setSettings: (settings) => { store.settings = settings; },
    setRuntime: (runtime) => { store.runtime = runtime; },
    readRuntime: api.runtime,
    readSettings: api.settings,
    setNetworkMode: api.setNetworkMode,
    startActive: api.startActive,
    ensureTunReady: ensureTunHelperReady,
    refresh: async () => { store.settings = await api.settings(); await refreshRuntimeOnly(true); renderSettings(); },
  });
  if (result.kind === "needs-profile") {
    toast("请先创建并激活一个配置档案", "error");
    navigate("profiles");
  } else if (result.kind === "failed") {
    let message = errorMessage(result.error);
    if (result.restored) message += "；已恢复之前的网络模式，未重新启动代理";
    if (result.rollbackError) message += `；回滚失败：${errorMessage(result.rollbackError)}`;
    toast(message, "error");
  } else if (result.refreshError) {
    toast(`运行状态刷新失败，请点击刷新核对：${errorMessage(result.refreshError)}`, "error");
  } else if (result.kind === "started") {
    toast(store.settings?.networkMode === "system_proxy" ? "Mihomo 已启动，系统代理已开启" : store.settings?.networkMode === "tun" ? "Mihomo 已启动，TUN 模式已开启" : "Mihomo 已启动，仅使用本地代理端口", "success");
  }
  void refreshSessionResume(true);
}

async function stopRuntime() {
  if (runtimeActionInFlight || networkModeSwitching) return;
  runtimeActionInFlight = true;
  runtimeMutationRevision++;
  renderHeader();
  renderAppearance(themeController.snapshot);
  try {
    const result = await action("Mihomo 已停止", () => api.stop());
    if (result) store.runtime = result;
  } finally {
    runtimeActionInFlight = false;
    runtimeMutationRevision++;
    renderAppearance(themeController.snapshot);
    await refreshRuntimeOnly();
    void refreshSessionResume(true);
  }
}

function toggleGlobalNetworkMode(mode: "system_proxy" | "tun") {
  if (!store.settings || !store.activeProfile) return;
  const running = store.runtime?.phase === "running";
  const active = mode === "system_proxy"
    ? Boolean(running && store.settings.networkMode === mode && store.systemProxy?.active)
    : Boolean(running && store.settings.networkMode === mode);
  if (active) {
    void switchNetworkMode("manual");
  } else if (store.settings.networkMode === mode && !running) {
    void startRuntime(mode);
  } else if (store.settings.networkMode === mode) {
    void (async () => {
      await stopRuntime();
      await startRuntime(mode);
    })();
  } else {
    void switchNetworkMode(mode);
  }
}

async function switchNetworkMode(mode: NetworkMode) {
  if (!store.settings) return;
  if (networkModeSwitching || runtimeActionInFlight || settingsSaving) return;
  const wasRunning = store.runtime?.phase === "running";
  const currentMode = store.settings.networkMode;
  if (mode === currentMode) return;
  const systemSwitch = $("#home-system-proxy") as HTMLInputElement;
  const tunSwitch = $("#home-tun") as HTMLInputElement;
  let modeChanged = false;
  networkModeSwitching = true;
  runtimeMutationRevision++;
  renderAppearance(themeController.snapshot);
  systemSwitch.disabled = true;
  tunSwitch.disabled = true;
  renderHeader();
  try {
    if (mode === "tun" && !(await ensureTunHelperReady())) return;
    if (wasRunning) await api.stop();
    store.settings = await api.setNetworkMode(mode);
    modeChanged = true;
    if (store.activeProfile && (wasRunning || mode !== "manual")) {
      store.runtime = await api.startActive();
    }
    toast(
      mode === "system_proxy"
        ? "系统代理已开启"
        : mode === "tun"
          ? "TUN 模式已开启"
          : "已切换为 Manual 模式",
      "success",
    );
  } catch (error) {
    let message = errorMessage(error);
    if (modeChanged) {
      try {
        store.settings = await api.setNetworkMode(currentMode);
        if (store.activeProfile && wasRunning) {
          store.runtime = await api.startActive();
        }
        message += "；已恢复之前的网络模式";
      } catch (rollbackError) {
        message += `；回滚失败：${errorMessage(rollbackError)}`;
      }
    }
    toast(message, "error");
  } finally {
    networkModeSwitching = false;
    runtimeMutationRevision++;
    renderAppearance(themeController.snapshot);
    await refreshBase();
  }
}

async function ensureTunHelperReady(): Promise<boolean> {
  let helper = await action("", () => api.tunHelperStatus());
  if (!helper) return false;
  store.tunHelper = helper;
  renderTunHelper();

  if (helper.state === "not_installed") {
    helper = await action("TUN Helper 已提交安装", () => api.installTunHelper());
  } else if (helper.state === "outdated" || helper.state === "unreachable") {
    helper = await action("TUN Helper 已修复", () => api.repairTunHelper());
  }
  if (!helper) return false;
  store.tunHelper = helper;
  renderTunHelper();

  if (helper.state === "requires_approval") {
    if (store.appInfo?.targetOs === "windows") {
      toast(helper.message, "info");
      navigate("settings");
      return false;
    }
    await action("", () => api.openTunHelperSettings());
    toast("请在系统设置中批准 Serylane TUN Helper，然后再次开启 TUN", "error");
    return false;
  }
  if (helper.state !== "ready") {
    toast(helper.message, "error");
    return false;
  }
  const prepared = await action("TUN 环境预检完成", () => api.prepareTun());
  return prepared !== null;
}

async function switchRoutingMode(mode: "global" | "rule" | "direct") {
  const active = store.activeProfile;
  if (!active) {
    toast("请先创建并激活订阅", "error");
    navigate("profiles");
    return;
  }
  const details = await action(
    mode === "global" ? "已切换为全局代理" : mode === "direct" ? "已切换为直连" : "已切换为规则模式",
    () => api.setProfileRoutingMode(active.profile.id, mode),
  );
  if (!details) return;
  store.activeProfile = details;
  if (store.selectedProfile?.profile.id === details.profile.id) {
    store.selectedProfile = details;
  }
  renderOverview();
  renderProfiles();
}

async function createSubscription(
  name: string,
  url: string,
  userAgent: string,
  generateOpenAi: boolean,
  activateAfterImport: boolean,
) {
  if (subscriptionImporting) {
    toast("已有订阅正在导入，请等待当前校验完成", "info");
    return;
  }
  subscriptionImporting = true;
  const form = $("#managed-subscription-form") as HTMLFormElement;
  const fields = $("#managed-subscription-fields") as HTMLFieldSetElement;
  const button = $("#managed-subscription-import-button") as HTMLButtonElement;
  const add = $("#subscriptions-add") as HTMLButtonElement;
  const status = $("#managed-subscription-import-status")!;
  const feedback = $("#subscriptions-feedback")!;
  const requestedViewRevision = viewNavigationRevision;
  let focusInterrupted = false;
  const trackFocus = (event: FocusEvent) => {
    if (event.target instanceof HTMLElement && event.target !== document.body && !form.contains(event.target)) focusInterrupted = true;
  };
  document.addEventListener("focusin", trackFocus);
  fields.disabled = true;
  add.disabled = true;
  form.setAttribute("aria-busy", "true");
  button.textContent = "正在校验…";
  feedback.hidden = true;
  status.className = "import-status is-loading";
  status.textContent = "正在获取订阅并执行 Mihomo 原生校验。首次导入可能需要 1～2 分钟，请保持窗口打开。";
  try {
    const result = await api.createSubscriptionProfile(
      name,
      url,
      userAgent,
      generateOpenAi,
      activateAfterImport,
    );
    // The write has succeeded. A later list/status read must never report it as
    // an import failure or encourage another submission of the same URL.
    const description = describeSubscriptionImport(result);
    form.reset();
    subscriptionDraftDirty = false;
    subscriptionActivationTouched = false;
    status.textContent = "";
    highlightedSubscriptionId = result.profile.id;
    closeSubscriptionForm(false);
    feedback.hidden = false;
    feedback.className = `subscription-feedback${description.warning ? " is-warning" : ""}`;
    feedback.textContent = description.text;
    toast(description.text, description.warning ? "info" : "success");
    try {
      if (!await refreshBase()) throw new Error("订阅列表尚未更新");
      const card = Array.from(document.querySelectorAll<HTMLElement>("[data-subscription-id]"))
        .find((item) => item.dataset.subscriptionId === result.profile.id);
      if (!card) throw new Error("订阅列表尚未更新");
      if (store.view === "subscriptions" && viewNavigationRevision === requestedViewRevision && !focusInterrupted) {
        card.focus({ preventScroll: true });
        card.scrollIntoView({ block: "nearest" });
      }
    } catch {
      feedback.textContent += " 列表状态未能刷新，请点击「刷新列表」；无需重复添加。";
    }
  } catch (error) {
    const message = errorMessage(error);
    status.className = "import-status is-error";
    status.textContent = message;
    toast(message, "error");
  } finally {
    document.removeEventListener("focusin", trackFocus);
    subscriptionImporting = false;
    fields.disabled = false;
    add.disabled = false;
    form.setAttribute("aria-busy", "false");
    button.textContent = "验证并添加";
  }
}

async function createInline() {
  const name = ($("#inline-name") as HTMLInputElement).value.trim();
  const source = ($("#yaml-source") as HTMLTextAreaElement).value.trim();
  if (!name || !source) {
    toast("请填写名称和 YAML 配置", "error");
    return;
  }
  const result = await action("配置已创建、校验并激活", () =>
    api.createInlineProfile(name, source),
  );
  if (!result) return;
  await refreshBase();
  store.selectedProfile = await api.profileDetails(result.profile.id);
  renderProfiles();
}

async function handleProfileAction(target: HTMLElement) {
  const actionName = target.dataset.profileAction;
  const profileId = target.dataset.profileId;
  if (!actionName || !profileId) return;
  if (actionName === "activate") {
    await action("配置已激活", () => api.activateProfile(profileId));
  } else if (actionName === "refresh") {
    await action("订阅已更新", () => api.refreshProfile(profileId));
  } else if (actionName === "rollback") {
    await action("已回滚到上一稳定版本", () => api.rollbackProfile(profileId));
  } else if (actionName === "delete") {
    await action("配置已删除", () => api.deleteProfile(profileId));
    store.selectedProfile = null;
  } else if (actionName === "revision") {
    await action("指定版本已激活", () =>
      api.activateProfile(profileId, target.dataset.revisionId),
    );
  }
  await refreshBase();
  if (actionName !== "delete") {
    store.selectedProfile = await api.profileDetails(profileId);
  }
  renderProfiles();
}

async function runNetworkSafetyCheck() {
  if (store.runtime?.phase !== "running") {
    toast("请先启动 Mihomo，再执行本地代理安全检查", "info");
    return;
  }
  const report = await action("", () => api.networkSafety());
  if (!report) return;
  store.networkSafety = report;
  renderSubscriptions();
  const summary = report.checks
    .map((check) => `${check.target} ${check.actualStatus ?? "失败"}`)
    .join(" · ");
  toast(report.warnings?.length
    ? `基础联网正常；${report.warnings.join("；")}`
    : `代理安全预检通过：${summary}`, report.warnings?.length ? "info" : "success");
}

async function refreshAllSubscriptions() {
  if (subscriptionRefreshing.size) {
    toast("已有订阅正在刷新，请稍后再试", "info");
    return;
  }
  if (!store.subscriptions.length) {
    toast("当前没有远程订阅", "info");
    return;
  }
  const button = $("#subscriptions-refresh-all") as HTMLButtonElement;
  button.disabled = true;
  button.textContent = "正在刷新…";
  let updated = 0;
  let failed = 0;
  const subscriptionIds = store.subscriptions.map(({ profile }) => profile.id);
  for (const id of subscriptionIds) subscriptionRefreshing.add(id);
  renderSubscriptions();
  try {
    for (const id of subscriptionIds) {
      try {
        const result = await api.refreshProfile(id);
        if (result.updated) updated += 1;
      } catch {
        failed += 1;
      } finally {
        subscriptionRefreshing.delete(id);
      }
    }
    toast(failed ? `刷新完成：${failed} 个失败，请查看对应卡片；${updated} 个配置有更新` : `订阅检查完成，${updated} 个配置有更新`, failed ? "error" : "success");
  } finally {
    for (const id of subscriptionIds) subscriptionRefreshing.delete(id);
    button.disabled = false;
    button.textContent = "刷新全部";
    await refreshBase();
  }
}

async function handleSubscriptionAction(target: HTMLElement) {
  const actionName = target.dataset.subscriptionAction;
  const profileId = target.dataset.profileId;
  if (!actionName || !profileId) return;
  if (subscriptionRefreshing.has(profileId)) return;
  if (actionName === "versions") {
    store.selectedProfile = await api.profileDetails(profileId);
    renderProfiles();
    navigate("profiles");
    return;
  }
  if (actionName === "openai-generate") {
    await startOpenAiGeneration(profileId);
    return;
  }
  if (actionName === "openai-cancel") {
    await cancelOpenAiGeneration();
    return;
  }
  if (actionName === "refresh") {
    subscriptionRefreshing.add(profileId);
    renderSubscriptions();
    try {
      await action("订阅检查完成，用量以服务商返回信息为准", () => api.refreshProfile(profileId));
    } finally {
      subscriptionRefreshing.delete(profileId);
    }
  } else if (actionName === "activate") {
    const activated = await action("", () => api.activateProfile(profileId));
    if (activated) toast("订阅已激活", "success");
  } else if (actionName === "delete") {
    const subscription = store.subscriptions.find(
      (subscription) => subscription.profile.id === profileId,
    );
    const profile = subscription?.profile;
    if (subscription?.active) {
      toast("当前订阅正在使用，请先激活其他订阅后再删除", "info", "top-right");
      return;
    }
    const confirmed = await confirmAction({
      title: "删除订阅",
      message: `确定删除“${profile?.displayName ?? "未命名订阅"}”及其本地版本记录？此操作不可撤销。`,
      confirmLabel: "确认删除",
      returnFocus: target,
    });
    if (!confirmed) {
      return;
    }
    const deleted = await action("", async () => {
      await api.deleteProfile(profileId);
      return true;
    });
    if (!deleted) return;
    toast("订阅删除成功", "success", "top-right");
  }
  await refreshBase();
}

function openAiTaskPhaseLabel(task: OpenAiPolicyTask): string {
  const labels: Record<OpenAiPolicyTask["phase"], string> = {
    idle: "等待创建",
    preparing: "准备独立检测环境",
    checking: "检测 OpenAI 可达性",
    bandwidth: "评估节点质量",
    applying: "校验并应用配置",
    completed: "配置已生成",
    failed: "生成失败",
    cancelled: "任务已停止",
  };
  return labels[task.phase];
}

function renderOpenAiPolicy() {
  const container = $("#openai-policy-card");
  if (!container) return;
  if (store.proxies && (nodeSelectionBusy || container.contains(document.activeElement) && document.activeElement?.tagName === "SELECT")) return;
  const active = store.activeProfile;
  if (!active) {
    container.innerHTML = `
      <div class="openai-policy-empty">
        <span class="openai-mark">AI</span>
        <div><h3>OpenAI 自动灾备</h3><p>激活一个包含显式节点的订阅后即可生成。</p></div>
      </div>`;
    return;
  }

  const policy = active.profile.openaiPolicy;
  const task = store.openAiTask;
  const taskForActive = task?.profileId === active.profile.id;
  const running = Boolean(taskForActive && task?.running);
  const anotherTaskRunning = Boolean(task?.running && !taskForActive);
  const runningProfileName = anotherTaskRunning
    ? store.subscriptions.find((subscription) => subscription.profile.id === task?.profileId)
        ?.profile.displayName ?? "其他订阅"
    : null;
  const proxyMap = (store.proxies?.proxies ?? {}) as Record<string, any>;
  const runtimeGroup = store.runtime?.phase === "running" ? proxyMap[OPENAI_GROUP_NAME] : undefined;
  const currentNode = runtimeGroup?.now ?? policy.selectedNodes[0]?.name ?? "—";
  const progress = task?.total
    ? Math.min(100, Math.round((task.completed / task.total) * 100))
    : running
      ? 6
      : 0;
  const statusText = running
    ? `${openAiTaskPhaseLabel(task!)} · ${task!.completed}/${task!.total || "—"}`
    : anotherTaskRunning
      ? `${runningProfileName} 正在生成 OpenAI 容灾`
    : policy.enabled
      ? `${policy.selectedNodes.length} 个节点 · ${policy.healthyCount}/${policy.candidateCount} 个候选通过`
      : "尚未创建托管策略";
  container.innerHTML = `
    <div class="openai-policy-head">
      <div class="openai-policy-title">
        <span class="openai-mark">AI</span>
        <div>
          <div class="openai-title-line"><h3>OpenAI 自动灾备</h3><span class="managed-badge">托管策略</span></div>
          <p>${escapeHtml(statusText)}</p>
        </div>
      </div>
      <div class="openai-policy-actions">
        <button class="button button-quiet" data-openai-action="stability" aria-pressed="${Boolean(policy.stabilityEnabled)}" ${!policy.enabled || policy.selectedNodes.length < 2 || task?.running || stabilityActionInFlight ? "disabled" : ""}>${policy.stabilityEnabled ? runtimeGroup?.manualNode ? "稳定优先：手动暂停" : "稳定优先：已开启" : "启用稳定优先"}</button>
        ${running
          ? '<button class="button button-danger" data-openai-action="cancel">停止检测</button>'
          : `<button class="button button-primary" data-openai-action="generate" ${anotherTaskRunning ? "disabled" : ""}>${anotherTaskRunning ? "其他订阅生成中" : policy.enabled ? "重新筛选 10 个" : "生成 10 个节点"}</button>`}
        <button class="button button-quiet" data-openai-action="health" ${!policy.enabled || Boolean(task?.running) || store.runtime?.phase !== "running" ? "disabled" : ""}>立即健康检查</button>
        <button class="button button-quiet" data-openai-action="details" ${!policy.enabled || store.runtime?.phase !== "running" ? "disabled" : ""}>节点详情</button>
        ${policy.enabled && !task?.running ? '<button class="button button-danger" data-openai-action="disable">停用</button>' : ""}
      </div>
    </div>
    ${running ? `
      <div class="openai-progress" aria-label="${escapeHtml(task!.message)}">
        <span style="width:${progress}%"></span>
      </div>
      <p class="openai-progress-copy">${escapeHtml(task!.message)}</p>
    ` : ""}
    ${taskForActive && task?.phase === "failed" && task.error ? `<div class="openai-error">${escapeHtml(task.error)}</div>` : ""}
    ${runtimeGroup && policy.enabled ? nodeSelectionMarkup(OPENAI_GROUP_NAME, runtimeGroup, nodeSelectionBusy || Boolean(task?.running), policy.selectedNodes.map(node => node.name).filter(name => runtimeGroup.all?.includes(name)), proxyMap) : '<p class="node-choice-help">生成灾备并启动核心后，可手动选择候选节点。不需要 OpenAI 灾备时，直接使用下方普通代理组。</p>'}
    <div class="openai-policy-stats">
      <div><span>${runtimeGroup?.now ? "当前节点" : "候选首选（非运行状态）"}</span><strong>${escapeHtml(currentNode)}</strong></div>
      <div><span>自动维护</span><strong>${policy.autoMaintain ? "订阅更新后执行" : "仅手动执行"}</strong></div>
      <div><span>上次筛选</span><strong>${formatPolicyDate(policy.lastBenchmarkedAt)}</strong></div>
      <div><span>故障策略</span><strong>${runtimeGroup?.manualNode ? "手动固定 · 暂停自动切换" : runtimeGroup?.fixed ? "手动优先 · 失效后由核心回退" : policy.stabilityEnabled ? "稳定优先 · 保持节点与故障冷却" : "基础 Fallback · 可能自动回切"}</strong></div>
    </div>
    <p class="hint">稳定优先可独立用于系统代理或 TUN，无需开启本地路由或接入 Codex。基础探测不是模型请求验证；已断开的流无法无缝续接。</p>
  `;
}

async function startOpenAiGeneration(profileId?: string) {
  const targetProfileId = profileId ?? store.activeProfile?.profile.id;
  if (!targetProfileId) {
    toast("请选择需要生成 OpenAI 容灾的订阅", "error");
    return;
  }
  const profile = store.subscriptions.find(
    (subscription) => subscription.profile.id === targetProfileId,
  )?.profile ?? (store.activeProfile?.profile.id === targetProfileId
    ? store.activeProfile.profile
    : null);
  const task = await action(
    `${profile?.displayName ?? "订阅"}：OpenAI 容灾生成已启动`,
    () => api.startOpenAiPolicyGeneration(targetProfileId, true),
  );
  if (!task) return;
  store.openAiTask = task;
  renderOpenAiPolicy();
  renderSubscriptions();
}

async function cancelOpenAiGeneration() {
  const task = await action("正在停止检测", () => api.cancelOpenAiPolicyGeneration());
  if (!task) return;
  store.openAiTask = task;
  renderOpenAiPolicy();
  renderSubscriptions();
}

async function refreshOpenAiTask() {
  const previous = store.openAiTask;
  const task = await action("", () => api.openAiPolicyTask());
  if (!task) return;
  store.openAiTask = task;
  renderOpenAiPolicy();
  renderSubscriptions();
  if (
    previous?.running &&
    !task.running &&
    task.finishedAt &&
    task.finishedAt !== openAiTaskFinishedAt
  ) {
    openAiTaskFinishedAt = task.finishedAt;
    if (task.phase === "completed") {
      const completedProfile = store.subscriptions.find(
        (subscription) => subscription.profile.id === task.profileId,
      );
      const applied = completedProfile?.active;
      toast(
        `${completedProfile?.profile.displayName ?? "订阅"}：${task.message}${applied ? "，已应用" : "，激活后生效"}`,
        "success",
      );
      await refreshBase();
      if (store.runtime?.phase === "running" && task.profileId === store.activeProfile?.profile.id) {
        await Promise.all([refreshProxies(), refreshRules()]);
      }
    } else if (task.phase === "failed") {
      toast(task.error ?? task.message, "error");
    } else if (task.phase === "cancelled") {
      toast("OpenAI 节点筛选已停止", "info");
    }
  }
}

async function refreshProxies(quiet = false) {
  const profile = store.activeProfile?.profile;
  const sequence = ++proxyReadSequence;
  const runtimeRevision = runtimeMutationRevision;
  const runtimePid = store.runtime?.pid;
  const contextCurrent = () => sequence === proxyReadSequence && runtimeRevision === runtimeMutationRevision && store.runtime?.pid === runtimePid
    && profile?.id === store.activeProfile?.profile.id && profile?.activeRevisionId === store.activeProfile?.profile.activeRevisionId;
  if (store.runtime?.phase !== "running" || !profile) {
    store.proxies = null;
    overviewNodeDetails = {};
    renderProxies();
    renderOverviewNodes();
    return;
  }
  try {
    const result = await api.proxies();
    if (!contextCurrent() || store.runtime?.phase !== "running") return;
    if (result.profileId !== profile.id || result.revisionId !== profile.activeRevisionId) throw new Error("节点信息与当前配置不一致，请刷新重试");
    store.proxies = result;
    proxyReadError = "";
    renderProxies();
    renderOverviewNodes();
    if (store.view !== "overview") return;
    const map = (result.proxies ?? {}) as ProxyMap;
    const groups = generalGroups(map, profile.routingMode);
    const general = groups.includes(overviewGroup) ? overviewGroup : groups[0];
    const targets = [general, profile.routingMode === "rule" && profile.openaiPolicy.enabled && map[OPENAI_GROUP_NAME] ? OPENAI_GROUP_NAME : null].filter((value): value is string => Boolean(value));
    const details = await Promise.all(targets.map(async group => {
      try { return [group, await api.currentNodeDetails(group)] as const; }
      catch { return [group, undefined] as const; }
    }));
    if (!contextCurrent() || store.runtime?.phase !== "running") return;
    overviewNodeDetails = Object.fromEntries(details.filter((entry): entry is readonly [string, CurrentNodeDetails] => Boolean(entry[1])));
    renderOverviewNodes();
  } catch (error) {
    if (!contextCurrent()) return;
    store.proxies = null;
    overviewNodeDetails = {};
    proxyReadError = errorMessage(error);
    renderProxies();
    renderOverviewNodes();
    if (!quiet) toast(proxyReadError, "error");
  }
}

function renderOverviewNodes() {
  const container = $("#overview-nodes-content");
  if (!container) return;
  const profile = store.activeProfile?.profile;
  if (overviewProfile !== profile?.id) { overviewProfile = profile?.id ?? ""; overviewGroup = ""; overviewNodeDetails = {}; }
  const running = store.runtime?.phase === "running";
  if (!running || !profile || !store.proxies || profile.routingMode === "direct") {
    container.innerHTML = `<p class="node-choice-help">${escapeHtml(!running ? "核心未运行，没有当前生效的代理节点。" : !profile ? "请先选用配置。" : profile.routingMode === "direct" ? "当前为直连模式，不使用代理节点。切回规则或全局模式后可选点。" : proxyReadError || "正在读取节点信息…")}</p>`;
    return;
  }
  const map = (store.proxies.proxies ?? {}) as ProxyMap;
  const groups = generalGroups(map, profile.routingMode);
  if (!groups.includes(overviewGroup)) overviewGroup = groups[0] ?? "";
  const html = `${groups.length > 1 ? `<label class="overview-node-selector">查看普通策略组（仅切换展示）<select id="overview-node-group">${groups.map(name => `<option value="${escapeHtml(name)}" ${overviewGroup === name ? "selected" : ""}>${escapeHtml(name)}</option>`).join("")}</select></label>` : ""}
    <div class="overview-node-grid">${overviewGroup ? currentNodeMarkup(profile.routingMode === "global" ? "全局代理" : "普通代理", overviewGroup, map, overviewNodeDetails[overviewGroup]) : '<p class="node-choice-help">当前配置没有可展示的普通代理组，请在「配置」检查节点与分流规则。</p>'}
    ${profile.routingMode === "rule" && profile.openaiPolicy.enabled && map[OPENAI_GROUP_NAME] ? currentNodeMarkup("OpenAI 专用出口", OPENAI_GROUP_NAME, map, overviewNodeDetails[OPENAI_GROUP_NAME]) : ""}</div>
    <p class="node-choice-help">${profile.routingMode === "global" ? "全局模式按 GLOBAL 组路由，不使用 OpenAI 专用分流规则。" : profile.openaiPolicy.enabled ? "OpenAI 专用出口仅用于命中对应规则的请求；用户自定义规则可能优先。" : "OpenAI 灾备未启用，普通代理选点可独立使用。"} 切换只影响后续新连接，不主动断开已有连接。</p>`;
  if (!container.contains(document.activeElement) && container.innerHTML !== html) container.innerHTML = html;
}

async function changeNode(group: string, node: string | null, returnFocus?: HTMLElement) {
  const profile = store.activeProfile?.profile;
  if (nodeSelectionBusy || !profile?.activeRevisionId || store.runtime?.phase !== "running" || !store.proxies || store.openAiTask?.running) return;
  nodeSelectionBusy = true;
  proxyReadSequence++;
  try {
    const costNode = node ? ((store.proxies?.proxies ?? {}) as ProxyMap)[node] : null;
    if (node && group === OPENAI_GROUP_NAME && (costNode?.trafficMultiplier == null || store.proxies?.costMode === "value" && costNode?.withinCostBudget === false)) {
      if (!await confirmAction({ title: "确认节点费用", message: costNode?.trafficMultiplier == null ? "此节点倍率未知，无法保证流量成本。手动选择会优先于自动成本策略，仍要继续吗？" : `此节点为 ${costNode.trafficMultiplier}×，不符合自动成本预算。手动选择会优先于预算，仍要继续吗？`, confirmLabel: "继续手动选择", returnFocus })) return;
    }
    const confirmed = await confirmAction({ title: node === null ? "恢复自动选择？" : "应用所选节点？", message: `${group}${node === null ? "：恢复该组的自动策略。" : ` → ${node}。`}只影响命中此组的后续新连接，不主动断开已有连接。${group === OPENAI_GROUP_NAME && profile.openaiPolicy.stabilityEnabled && node !== null ? "手动固定期间暂停稳定策略的自动切换，节点故障时需手动更换或恢复自动。" : "Fallback / URLTest 的手动选择为优先使用，失效时核心仍可能回退。"}`, confirmLabel: node === null ? "恢复自动" : "应用节点", returnFocus });
    if (!confirmed) return;
    if (profile.id !== store.activeProfile?.profile.id || profile.activeRevisionId !== store.activeProfile?.profile.activeRevisionId || store.runtime?.phase !== "running") {
      toast("配置或运行状态已变化，请重新选择", "error"); return;
    }
    await action(node === null ? "已恢复自动选择" : "已提交节点选择，正在读取实际出口", async () => {
      if (node === null) await api.clearProxySelection(group, profile.id, profile.activeRevisionId!);
      else await api.selectProxy(group, node, profile.id, profile.activeRevisionId!);
      return true;
    });
  } finally {
    nodeSelectionBusy = false;
    await refreshProxies(true);
  }
}

function renderProxies() {
  // Do not replace a focused native dropdown or an unapplied selection draft
  // during background polling. Explicit refresh/confirmation updates it later.
  if ($("#proxies-view")?.contains(document.activeElement) && !store.proxies) (document.activeElement as HTMLElement)?.blur();
  if (store.proxies && (nodeSelectionBusy || $("#proxies-view")?.contains(document.activeElement) && document.activeElement?.tagName === "SELECT")) return;
  renderOpenAiPolicy();
  const container = $("#proxy-groups");
  if (!container) return;
  const proxyMap = (store.runtime?.phase === "running" ? store.proxies?.proxies ?? {} : {}) as Record<string, any>;
  const groups = Object.entries(proxyMap).filter(
    ([name, value]) => name !== OPENAI_GROUP_NAME && Array.isArray(value?.all),
  );
  if (!groups.length) {
    container.className = "card-list empty-state";
    container.textContent = proxyReadError || "启动核心后可查看普通代理组，无需启用 OpenAI 灾备。";
    return;
  }
  container.className = "card-list";
  container.innerHTML = groups
    .map(([name, value]) => {
      return `
        <article class="proxy-card" data-group="${escapeHtml(name)}" tabindex="-1">
          <div><h3>${escapeHtml(name)}</h3><p>${escapeHtml(value.type)} · UDP ${value.udp ? "支持" : "未知"}</p></div>
          ${nodeSelectionMarkup(name, value, nodeSelectionBusy || Boolean(store.openAiTask?.running))}
          <div class="proxy-card-footer">
            <span>当前：${escapeHtml(value.now ?? "—")}</span>
            <div class="toolbar">
              <button class="button button-quiet proxy-details" data-group="${escapeHtml(name)}">详情</button>
              <button class="button button-quiet proxy-delay" data-proxy="${escapeHtml(value.now ?? name)}">测速</button>
            </div>
          </div>
        </article>`;
    })
    .join("");
}

function preferredCurrentGroup(): string | null {
  const proxyMap = (store.proxies?.proxies ?? {}) as Record<string, any>;
  const groups = generalGroups(proxyMap, store.activeProfile?.profile.routingMode ?? "rule");
  if (groups.length) return groups.includes(overviewGroup) ? overviewGroup : groups[0];
  return null;
}

function renderNodeDetails() {
  const details = store.nodeDetails;
  const container = $("#node-details-content");
  if (!container || !details) return;
  const history = details.history.filter((sample) => sample.delayMs > 0).slice(-5);
  const delays = history.map((sample) => sample.delayMs);
  const minDelay = delays.length ? Math.min(...delays) : 0;
  const maxDelay = delays.length ? Math.max(...delays) : 0;
  const bars = history.length
    ? history
        .map((sample, index) => {
          const ratio = maxDelay === minDelay
            ? 0.5
            : (sample.delayMs - minDelay) / (maxDelay - minDelay);
          return `<span title="${sample.delayMs} ms" style="height:${Math.round(18 + ratio * 16)}px;opacity:${0.65 + index * 0.07}"></span>`;
        })
        .join("")
    : '<em>暂无历史</em>';
  const average = delays.length
    ? Math.round(delays.reduce((sum, delay) => sum + delay, 0) / delays.length)
    : null;
  const latestTime = history[history.length - 1]?.time;
  const detailsRows = [
    ["节点名称", details.nodeName],
    ["传输网络", details.network?.toUpperCase() ?? "未声明"],
    ["服务器", details.maskedServer ?? "由 Provider 托管"],
    ["TLS", details.tls ?? "未声明"],
    ["端口", details.port ?? "—"],
    ["UDP", details.udp == null ? "未知" : details.udp ? "支持" : "关闭"],
    ["Provider", details.providerName ?? "本地配置"],
    ["探测状态", details.alive == null ? "尚未验证" : details.alive ? "最近探测可达" : "最近探测失败"],
  ];
  container.innerHTML = `
    <header class="node-modal-header">
      <div class="node-modal-identity"><span class="node-modal-mark">${escapeHtml(Array.from(details.nodeName).slice(0, 2).join("").toUpperCase())}</span><div><h2 id="node-details-title">当前节点信息</h2><p>${escapeHtml(details.group)} · 当前生效链路</p></div></div>
      <button class="button button-quiet" data-node-modal-action="close">关闭</button>
    </header>
    <div class="node-health-summary">
      <div><span>状态</span><strong>${details.alive == null ? "尚未验证" : details.alive ? "最近探测可达" : "最近探测失败"}</strong></div>
      <div><span>延迟</span><strong>${details.lastDelayMs == null ? "—" : `${details.lastDelayMs} ms`}</strong></div>
      <div><span>协议</span><strong>${escapeHtml(details.nodeType)} · ${escapeHtml(details.tls ?? "无 TLS")}</strong></div>
      <div><span>最近检测</span><strong>${latestTime ? formatPolicyDate(latestTime) : "暂无"}</strong></div>
    </div>
    <section class="node-route-card"><span>当前代理链路</span><strong>${details.routeChain.map(escapeHtml).join(" <i>›</i> ")}</strong></section>
    <div class="node-detail-grid">
      ${detailsRows.map(([label, value]) => `<div><span>${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong></div>`).join("")}
    </div>
    <section class="node-history-card">
      <div><strong>最近 ${history.length || 0} 次检测</strong><span>${average == null ? "暂无有效延迟记录" : `平均 ${average} ms · ${details.alive === false ? "等待故障切换" : "未触发切换"}`}</span></div>
      <div class="node-latency-bars">${bars}</div>
    </section>
    <footer class="node-modal-footer">
      <span>敏感凭据已隐藏，仅展示诊断所需信息</span>
      <div class="toolbar">
        <button class="button button-quiet" data-node-modal-action="retest">重新测速</button>
        <button class="button button-primary" data-node-modal-action="switch">切换节点</button>
      </div>
    </footer>`;
}

async function openNodeDetails(group: string | null) {
  if (!group) {
    toast("当前没有可展示的代理组", "info");
    return;
  }
  if (store.runtime?.phase !== "running") {
    toast("请先启动 Mihomo，再读取当前节点信息", "info");
    return;
  }
  const details = await action("", () => api.currentNodeDetails(group));
  if (!details) return;
  store.nodeDetails = details;
  renderNodeDetails();
  $("#node-details-modal")!.classList.remove("is-hidden");
  $("#node-details-modal")!.setAttribute("aria-hidden", "false");
  syncModalScrollLock();
  const close = $("#node-details-modal [data-node-modal-action='close']") as HTMLButtonElement;
  close?.focus();
}

function closeNodeDetails() {
  $("#node-details-modal")?.classList.add("is-hidden");
  $("#node-details-modal")?.setAttribute("aria-hidden", "true");
  syncModalScrollLock();
}

function focusNodeGroup(group: string) {
  closeNodeDetails();
  navigate("proxies");
  renderProxies();
  const card = group === OPENAI_GROUP_NAME ? $("#openai-policy-card") : $$(".proxy-card").find((element) => element.dataset.group === group);
  card?.scrollIntoView({ behavior: "smooth", block: "center" });
  (card?.querySelector("select, button") as HTMLElement | null)?.focus();
}

async function refreshRules() {
  const result = await action("", () => api.rules());
  if (!result) return;
  store.rules = result;
  renderRules();
}

function renderRules() {
  const query = ($("#rule-search") as HTMLInputElement)?.value.toLowerCase() ?? "";
  const rules = ((store.rules?.rules ?? []) as any[]).filter((rule) =>
    JSON.stringify(rule).toLowerCase().includes(query),
  );
  $("#rules-body")!.innerHTML =
    rules
      .map(
        (rule) => `<tr><td>${escapeHtml(rule.type)}</td><td>${escapeHtml(rule.payload)}</td><td>${escapeHtml(rule.proxy)}</td></tr>`,
      )
      .join("") || '<tr><td colspan="3">没有匹配规则</td></tr>';
}

async function refreshConnections() {
  const result = await action("", () => api.connections());
  if (!result) return;
  store.connections = result;
  renderConnections();
}

function renderConnections() {
  const payload = store.connections ?? {};
  const connections = (payload.connections ?? []) as any[];
  $("#connection-totals")!.innerHTML = `<span>上传 <strong>${formatBytes(payload.uploadTotal)}</strong></span><span>下载 <strong>${formatBytes(payload.downloadTotal)}</strong></span><span>连接 <strong>${connections.length}</strong></span>`;
  $("#connections-body")!.innerHTML =
    connections
      .map((connection) => {
        const metadata = connection.metadata ?? {};
        const target = metadata.host || metadata.destinationIP || "—";
        const chains = Array.isArray(connection.chains) ? connection.chains.join(" → ") : "—";
        return `
          <tr>
            <td><strong>${escapeHtml(target)}</strong><small>:${escapeHtml(metadata.destinationPort)}</small></td>
            <td>${escapeHtml(metadata.network ?? metadata.type)}</td>
            <td>${escapeHtml(connection.rule)} ${escapeHtml(connection.rulePayload)}</td>
            <td>${escapeHtml(chains)}</td>
            <td>↑ ${formatBytes(connection.upload)}<br>↓ ${formatBytes(connection.download)}</td>
            <td><button class="button button-danger close-connection" data-connection-id="${escapeHtml(connection.id)}">关闭</button></td>
          </tr>
        `;
      })
      .join("") || '<tr><td colspan="6">暂无活动连接</td></tr>';
}

async function refreshLogs() {
  await logsView.refresh();
}

async function runDiagnostics() {
  const checks: Array<{ label: string; status: string; detail: string }> = [];
  const binary = await action("", () => api.binary());
  checks.push({
    label: "Mihomo sidecar",
    status: binary?.available ? "pass" : "fail",
    detail: binary?.version ?? binary?.message ?? "未找到",
  });
  const runtime = await action("", () => api.runtime());
  checks.push({
    label: "运行状态",
    status: runtime?.phase === "running" ? "pass" : "warn",
    detail: runtime?.message ?? "未运行",
  });
  checks.push({
    label: "活动配置",
    status: store.activeProfile ? "pass" : "fail",
    detail: store.activeProfile?.profile.displayName ?? "未激活配置",
  });
  checks.push({
    label: "系统代理",
    status:
      store.settings?.networkMode !== "system_proxy" || store.systemProxy?.active
        ? "pass"
        : "warn",
    detail: store.systemProxy?.active ? "已保存快照并接管" : "未接管",
  });
  if (runtime?.phase === "running") {
    const proxyResult = await action("", () => api.proxies());
    checks.push({
      label: "Mihomo API",
      status: proxyResult ? "pass" : "fail",
      detail: proxyResult ? "控制接口可访问" : "控制接口请求失败",
    });
    const networkChecks = await action("", () => api.diagnostics());
    for (const check of networkChecks ?? []) {
      checks.push({
        label:
          check.stage === "local_proxy"
            ? "本地代理端口"
            : check.stage === "controller"
              ? "控制接口端口"
              : "实际代理请求",
        status: check.success ? "pass" : "fail",
        detail:
          check.detail +
          (check.latencyMs != null ? " · " + check.latencyMs + " ms" : ""),
      });
    }
  }
  $("#diagnostic-list")!.innerHTML = checks
    .map((check) => `<div class="diagnostic-item status-${check.status}"><span class="diagnostic-icon">${check.status === "pass" ? "✓" : check.status === "warn" ? "!" : "×"}</span><div><strong>${escapeHtml(check.label)}</strong><p>${escapeHtml(check.detail)}</p></div></div>`)
    .join("");
}

const logsView = mountLogs($("#logs-view")!, { api, confirm: confirmAction });
const openAiCosts = mountOpenAiCosts({
  profile: () => store.activeProfile ? { id: store.activeProfile.profile.id, revision: store.activeProfile.profile.activeRevisionId } : null,
  read: api.openAiCosts, save: api.saveOpenAiCosts,
  confirm: message => confirmAction({ title: "节点成本策略", message, confirmLabel: "确认", returnFocus: $("#openai-cost-settings summary") ?? undefined }),
  changed: () => { void refreshProxies(true); },
});
const programManager = mountProgramManager($("#programs-view")!, { api, confirm: confirmAction, error: errorMessage });
const localRouting = mountLocalRouting($("#routing-view")!, { api, confirm: confirmAction, error: errorMessage });
mountProxyCompatibility($("#proxy-compatibility-panel")!, api.systemProxyCompatibility, errorMessage);

const ruleManager = mountRuleManager($("#rules-view")!, {
  api,
  confirm: confirmAction,
  error: errorMessage,
  onApplied: async () => {
    if (store.runtime?.phase === "running") await refreshRules();
  },
});

function navigate(view: ViewName) {
  const previousView = store.view;
  if (previousView !== view) viewNavigationRevision++;
  const scroller = $("#page-scroll");
  if (previousView !== view && scroller) {
    viewScrollPositions[previousView] = scroller.scrollTop;
  }
  store.view = view;
  document.documentElement.dataset.view = view;
  $("#page-title")!.textContent = NAV_ITEMS.find((item) => item.id === view)!.label;
  $$(".nav-item").forEach((button) => {
    const active = button.dataset.view === view;
    button.classList.toggle("is-active", active);
    if (active) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  });
  $$(".view-stack").forEach((element) =>
    element.classList.toggle("is-hidden", element.id !== `${view}-view`),
  );
  if (view === "proxies") {
    void openAiCosts.refresh();
    void refreshOpenAiTask();
    if (store.runtime?.phase === "running") void refreshProxies();
  }
  if (view === "overview") void refreshProxies(true);
  if (view === "subscriptions") renderSubscriptions();
  if (view === "settings") void refreshSessionResume(true);
  if (view === "programs") void programManager.refresh();
  if (view === "routing") void localRouting.refresh();
  if (view === "rules") {
    void ruleManager.refresh();
    if (store.runtime?.phase === "running") void refreshRules();
  }
  if (view === "connections" && store.runtime?.phase === "running") void refreshConnections();
  if (view === "logs") void refreshLogs();
  if (view === "diagnostics") void runDiagnostics();
  if (previousView !== view) restoreViewScroll(view);
}

$$<HTMLButtonElement>(".nav-item").forEach((button) =>
  button.addEventListener("click", () => navigate(button.dataset.view as ViewName)),
);
$("#global-refresh")!.addEventListener("click", () => void refreshBase());
$("#global-start")!.addEventListener("click", () => void startRuntime("previous"));
$("#global-stop")!.addEventListener("click", () => void stopRuntime());
$("#global-system-proxy")!.addEventListener("click", () =>
  toggleGlobalNetworkMode("system_proxy"),
);
$("#global-tun")!.addEventListener("click", () => toggleGlobalNetworkMode("tun"));
$("#home-system-proxy")!.addEventListener("change", (event) => {
  const enabled = (event.target as HTMLInputElement).checked;
  void switchNetworkMode(enabled ? "system_proxy" : "manual");
});
$("#home-tun")!.addEventListener("change", (event) => {
  const enabled = (event.target as HTMLInputElement).checked;
  void switchNetworkMode(enabled ? "tun" : "manual");
});
$("#home-routing-mode")!.addEventListener("click", (event) => {
  const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
    "[data-routing-mode]",
  );
  if (button?.dataset.routingMode) {
    void switchRoutingMode(
      button.dataset.routingMode as "global" | "rule" | "direct",
    );
  }
});
$("#profiles-refresh")!.addEventListener("click", () => void refreshBase());
$("#subscriptions-refresh-list")!.addEventListener("click", () => void refreshBase());
$("#subscriptions-refresh-all")!.addEventListener("click", () => void refreshAllSubscriptions());
$("#subscriptions-run-safety")!.addEventListener("click", () => void runNetworkSafetyCheck());
$("#proxies-refresh")!.addEventListener("click", () => void refreshProxies());
$("#proxies-current-node")!.addEventListener("click", () =>
  void openNodeDetails(preferredCurrentGroup()),
);
$("#rules-refresh")!.addEventListener("click", () => void refreshRules());
$("#connections-refresh")!.addEventListener("click", () => void refreshConnections());
$("#run-diagnostics")!.addEventListener("click", () => void runDiagnostics());

$("#overview-go-subscriptions")!.addEventListener("click", () => navigate("subscriptions"));
$("#subscriptions-add")!.addEventListener("click", () => openSubscriptionForm());
$("#managed-subscription-cancel")!.addEventListener("click", () => closeSubscriptionForm());
$("#managed-subscription-form")!.addEventListener("input", () => { subscriptionDraftDirty = true; });
$("#managed-subscription-activate")!.addEventListener("change", () => {
  subscriptionActivationTouched = true;
  renderSubscriptionActivationHint();
});
$("#managed-subscription-form")!.addEventListener("submit", (event) => {
  event.preventDefault();
  void createSubscription(
    ($("#managed-subscription-name") as HTMLInputElement).value.trim(),
    ($("#managed-subscription-url") as HTMLInputElement).value.trim(),
    ($("#managed-subscription-ua") as HTMLInputElement).value.trim() || "clash.meta",
    ($("#managed-subscription-openai") as HTMLInputElement).checked,
    ($("#managed-subscription-activate") as HTMLInputElement).checked,
  );
});

$("#load-sample")!.addEventListener("click", () => {
  ($("#yaml-source") as HTMLTextAreaElement).value = sampleProfile;
  $("#yaml-summary")!.textContent = "已载入示例";
});
$("#yaml-file")!.addEventListener("change", (event) => {
  const file = (event.target as HTMLInputElement).files?.[0];
  if (!file) return;
  const reader = new FileReader();
  reader.onload = () => {
    ($("#yaml-source") as HTMLTextAreaElement).value = String(reader.result ?? "");
    ($("#inline-name") as HTMLInputElement).value = file.name.replace(/\.(ya?ml)$/i, "");
    $("#yaml-summary")!.textContent = `${file.size.toLocaleString()} 字节`;
  };
  reader.readAsText(file);
});
$("#inspect-yaml")!.addEventListener("click", async () => {
  const source = ($("#yaml-source") as HTMLTextAreaElement).value;
  const summary = await action("", () => api.inspect(source));
  if (summary) {
    $("#yaml-summary")!.textContent = `${summary.nodeCount} 节点 · ${summary.proxyGroupCount} 组 · ${summary.ruleCount} 规则`;
    toast(summary.warnings[0] ?? "配置结构检查通过", summary.warnings.length ? "info" : "success");
  }
});
$("#create-inline")!.addEventListener("click", () => void createInline());

$("#profile-list")!.addEventListener("click", async (event) => {
  const row = (event.target as HTMLElement).closest<HTMLElement>("[data-profile-id]");
  if (!row?.dataset.profileId) return;
  store.selectedProfile = await api.profileDetails(row.dataset.profileId);
  renderProfiles();
});
$("#profile-detail")!.addEventListener("click", (event) => {
  const target = (event.target as HTMLElement).closest<HTMLElement>("[data-profile-action]");
  if (target) void handleProfileAction(target);
});

$("#subscription-manager-list")!.addEventListener("click", (event) => {
  const target = (event.target as HTMLElement).closest<HTMLElement>(
    "[data-subscription-action]",
  );
  if (target) void handleSubscriptionAction(target);
});

$("#confirmation-modal")!.addEventListener("click", (event) => {
  const target = event.target as HTMLElement;
  if (target.id === "confirmation-modal") {
    closeConfirmation(false);
    return;
  }
  const button = target.closest<HTMLButtonElement>("[data-confirmation-action]");
  if (button?.dataset.confirmationAction === "cancel") closeConfirmation(false);
  if (button?.dataset.confirmationAction === "confirm") closeConfirmation(true);
});

$("#proxies-view")!.addEventListener("submit", (event) => {
  const form = (event.target as HTMLElement).closest<HTMLFormElement>("form[data-node-group]");
  if (!form) return;
  event.preventDefault();
  const selected = form.querySelector("select")?.value;
  if (selected) void changeNode(form.dataset.nodeGroup!, selected, form.querySelector("button") ?? undefined);
});
$("#proxies-view")!.addEventListener("click", (event) => {
  const button = (event.target as HTMLElement).closest<HTMLElement>("[data-node-auto]");
  if (button) void changeNode(button.dataset.nodeAuto!, null, button);
});
$("#overview-nodes-refresh")!.addEventListener("click", () => void refreshProxies());
$("#overview-nodes-content")!.addEventListener("change", event => {
  if ((event.target as HTMLElement).id !== "overview-node-group") return;
  overviewGroup = (event.target as HTMLSelectElement).value;
  (event.target as HTMLElement).blur();
  renderOverviewNodes();
  void refreshProxies(true);
});
$("#overview-nodes-content")!.addEventListener("click", event => {
  const target = (event.target as HTMLElement).closest<HTMLElement>("[data-overview-node-group], [data-overview-node-details]");
  if (target?.dataset.overviewNodeGroup) focusNodeGroup(target.dataset.overviewNodeGroup);
  else if (target?.dataset.overviewNodeDetails) void openNodeDetails(target.dataset.overviewNodeDetails);
});
$("#proxy-groups")!.addEventListener("click", async (event) => {
  const detailsButton = (event.target as HTMLElement).closest<HTMLButtonElement>(
    ".proxy-details",
  );
  if (detailsButton?.dataset.group) {
    await openNodeDetails(detailsButton.dataset.group);
    return;
  }
  const button = (event.target as HTMLElement).closest<HTMLButtonElement>(".proxy-delay");
  if (!button?.dataset.proxy) return;
  const result = await action("", () => api.testProxyDelay(button.dataset.proxy!));
  if (result) toast(`延迟：${escapeHtml(result.delay ?? "—")} ms`, "success");
});

$("#openai-policy-card")!.addEventListener("click", async (event) => {
  const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
    "[data-openai-action]",
  );
  const actionName = button?.dataset.openaiAction;
  if (!actionName || !store.activeProfile) return;
  if (actionName === "stability") {
    if (stabilityActionInFlight || store.openAiTask?.running) return;
    const profile = store.activeProfile.profile;
    if (!profile.activeRevisionId || !profile.openaiPolicy.enabled || profile.openaiPolicy.selectedNodes.length < 2) return;
    const enabled = !profile.openaiPolicy.stabilityEnabled;
    stabilityActionInFlight = true;
    try {
      if (!await confirmAction({ title: enabled ? "启用稳定优先？" : "恢复基础灾备？", message: "将保存并应用新的核心配置，请避开重要请求。稳定优先保持正常节点，连续失败后才切换并冷却；不会主动清空连接，不会接入 Codex 本地路由。", confirmLabel: "确认应用", returnFocus: button })) return;
      await action("灾备策略已更新", () => api.setOpenAiStability(enabled, profile.id, profile.activeRevisionId!, true));
      await refreshBase();
      if (store.runtime?.phase === "running") await refreshProxies();
    } finally { stabilityActionInFlight = false; renderOpenAiPolicy(); }
  } else if (actionName === "generate") {
    await startOpenAiGeneration();
  } else if (actionName === "cancel") {
    await cancelOpenAiGeneration();
  } else if (actionName === "health") {
    const result = await action("", () =>
      api.testProxyGroup(
        OPENAI_GROUP_NAME,
        "https://api.openai.com/v1/models",
        "401",
        8_000,
      ),
    );
    if (result) {
      toast(`健康检查完成：${Object.keys(result).length} 个节点可达`, "success");
      await refreshProxies();
    }
  } else if (actionName === "details") {
    await openNodeDetails(OPENAI_GROUP_NAME);
  } else if (actionName === "disable") {
    const policy = await action("OpenAI 自动灾备已停用", () =>
      api.disableOpenAiPolicy(store.activeProfile!.profile.id),
    );
    if (policy) {
      await refreshBase();
      if (store.runtime?.phase === "running") {
        await Promise.all([refreshProxies(), refreshRules()]);
      }
    }
  }
});

$("#node-details-modal")!.addEventListener("click", async (event) => {
  const target = event.target as HTMLElement;
  if (target.id === "node-details-modal") {
    closeNodeDetails();
    return;
  }
  const button = target.closest<HTMLButtonElement>("[data-node-modal-action]");
  const actionName = button?.dataset.nodeModalAction;
  const details = store.nodeDetails;
  if (!actionName) return;
  if (actionName === "close") {
    closeNodeDetails();
  } else if (actionName === "retest" && details) {
    const result = await action("", () => api.testProxyDelay(details.nodeName));
    if (result) toast(`延迟：${result.delay ?? "—"} ms`, "success");
    await openNodeDetails(details.group);
  } else if (actionName === "switch" && details) {
    focusNodeGroup(details.group);
  }
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Tab" && !$("#confirmation-modal")!.classList.contains("is-hidden")) {
    const controls = $$<HTMLButtonElement>("#confirmation-modal button:not(:disabled)");
    const first = controls[0], last = controls[controls.length - 1];
    if (first && last && ((event.shiftKey ? document.activeElement === first : document.activeElement === last)
        || !$("#confirmation-modal")!.contains(document.activeElement))) {
      event.preventDefault();
      (event.shiftKey ? last : first)?.focus();
    }
  }
  if (event.key === "Escape" && !$("#confirmation-modal")!.classList.contains("is-hidden")) {
    closeConfirmation(false);
    return;
  }
  if (event.key === "Escape" && !$("#node-details-modal")!.classList.contains("is-hidden")) {
    closeNodeDetails();
  }
});

$("#rule-search")!.addEventListener("input", renderRules);
$("#connections-body")!.addEventListener("click", async (event) => {
  const button = (event.target as HTMLElement).closest<HTMLButtonElement>(".close-connection");
  if (!button?.dataset.connectionId) return;
  await action("连接已关闭", () => api.closeConnection(button.dataset.connectionId!));
  await refreshConnections();
});


$("#tun-helper-install")!.addEventListener("click", async () => {
  const helper = await action("TUN Helper 已提交安装", () => api.installTunHelper());
  if (helper) store.tunHelper = helper;
  await refreshBase();
});

$("#tun-helper-repair")!.addEventListener("click", async () => {
  const helper = await action("TUN Helper 已修复", () => api.repairTunHelper());
  if (helper) store.tunHelper = helper;
  await refreshBase();
});

$("#tun-helper-open-settings")!.addEventListener("click", async () => {
  await action("", () => api.openTunHelperSettings());
});

$("#tun-helper-uninstall")!.addEventListener("click", async (event) => {
  const confirmed = await confirmAction({
    title: "卸载 TUN Helper",
    message: "卸载后 TUN 模式将停止使用，Manual 与系统代理不受影响。",
    confirmLabel: "确认卸载",
    returnFocus: event.currentTarget as HTMLElement,
  });
  if (!confirmed) return;
  await action("TUN Helper 已卸载", () => api.uninstallTunHelper());
  await refreshBase();
});

$("#app-update-check")!.addEventListener("click", () => {
  void checkForAppUpdate(false);
});

$("#app-update-open")!.addEventListener("click", async () => {
  const update = store.appUpdate;
  if (!update) return;
  await action("已打开 Serylane 官方更新页面", () =>
    api.openOfficialRelease(update.source, update.latestVersion),
  );
});

$("#update-preferences-form")!.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (appUpdateActionBusy || appUpdateChecking) return;
  appUpdateActionBusy = true;
  renderAppUpdate();
  try {
    store.settings = await api.saveUpdatePreferences(
      ($("#settings-update-source") as HTMLSelectElement).value as UpdateSource,
      ($("#settings-auto-check-updates") as HTMLInputElement).checked,
      ($("#settings-auto-download-updates") as HTMLInputElement).checked,
    );
    acceptAppUpdate(await api.appUpdateStatus());
    renderSettings();
    scheduleAutomaticUpdateCheck();
    toast("更新偏好已保存；网络设置未改变", "success");
  } catch (error) { toast(errorMessage(error), "error"); }
  finally { appUpdateActionBusy = false; renderAppUpdate(); }
});

$("#app-update-download")!.addEventListener("click", () => { void downloadAppUpdate(); });
$("#app-update-cancel")!.addEventListener("click", async () => {
  await action("正在取消下载…", () => api.cancelAppUpdate());
});
$("#app-update-install")!.addEventListener("click", async (event) => {
  if (appUpdateActionBusy || !describeAppUpdate(appUpdateStatus).canInstall || !store.appUpdate) return;
  const version = store.appUpdate.latestVersion;
  const confirmed = await confirmAction({
    title: `安装 Serylane ${version}`,
    message: "Serylane 安装包已通过签名和 SHA-256 校验。继续后将暂时停止代理并重启 Serylane，正在进行的 Codex 对话、下载等连接可能中断。确认现在安装吗？",
    confirmLabel: "确认安装并重启",
    returnFocus: event.currentTarget as HTMLElement,
  });
  if (!confirmed) return;
  appUpdateActionBusy = true;
  appUpdateStatus = { ...appUpdateStatus, phase: "installing" };
  renderAppUpdate();
  try { await api.installAppUpdate(version, true); }
  catch (error) {
    try { acceptAppUpdate(await api.appUpdateStatus()); }
    catch { acceptAppUpdate({ ...appUpdateStatus, phase: "failed", error: errorMessage(error) }); }
    toast(errorMessage(error), "error");
    await refreshBase();
  } finally { appUpdateActionBusy = false; renderAppUpdate(); }
});

document.querySelectorAll<HTMLInputElement>('[name="settings-network-mode"]').forEach((radio) => {
  radio.addEventListener("change", () => {
    if (radio.checked) ($("#settings-mode") as HTMLSelectElement).value = radio.value;
  });
});
$("#settings-startup-mode")!.addEventListener("change", () => {
  if (settingsSaving || !store.settings) return;
  const selected = ($("#settings-startup-mode") as HTMLSelectElement).value as StartupMode;
  startupModeDraft = selected === startupModeFromSettings(store.settings) ? null : selected;
  renderSessionResume();
});
$("#settings-startup-check")!.addEventListener("click", () => { void refreshSessionResume(true); });
$("#settings-form")!.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!store.settings || settingsSaving || runtimeActionInFlight || networkModeSwitching || themeController.snapshot.saving) return;
  const mode = ($("#settings-mode") as HTMLSelectElement).value as NetworkMode;
  const settings: AppSettings = {
    ...startupModeSettings(store.settings, startupModeDraft ?? startupModeFromSettings(store.settings)),
    networkMode: mode,
    mixedPort: Number(($("#settings-mixed-port") as HTMLInputElement).value),
    controllerPort: Number(($("#settings-controller-port") as HTMLInputElement).value),
    theme: themeController.snapshot.preference,
    showGlobalTraffic: ($("#settings-global-traffic") as HTMLInputElement).checked,
    diagnosticsRetentionDays: Number(
      ($("#settings-retention") as HTMLInputElement).value,
    ),
    appLogRetentionDays: Number(($("#settings-app-log-retention") as HTMLInputElement).value),
  };
  settingsSaving = true;
  runtimeMutationRevision++;
  sessionResumeRevision++;
  startupStatus = null;
  ($("#settings-startup-mode") as HTMLSelectElement).disabled = true;
  themeController.refresh();
  try {
    if (mode === "tun" && mode !== store.settings.networkMode) {
      if (!(await ensureTunHelperReady())) return;
    }
    const updated = await action("设置已保存", async () => {
      if (mode !== store.settings!.networkMode) {
        await api.setNetworkMode(mode);
      }
      return api.updateSettings(settings);
    });
    if (updated) {
      store.settings = updated;
      startupModeDraft = null;
      renderSettings();
      renderOverview();
      renderGlobalTraffic();
    }
  } finally {
    settingsSaving = false;
    runtimeMutationRevision++;
    ($("#settings-startup-mode") as HTMLSelectElement).disabled = false;
    themeController.refresh();
    void refreshSessionResume(true);
  }
});

window.setInterval(() => {
  if (sessionResumePresentation(sessionResumeStatus).busy) void refreshSessionResume();
  if (store.view === "logs") void refreshLogs();
  if (store.runtime?.phase === "running") {
    void refreshRuntimeOnly();
    if (store.view === "connections") void refreshConnections();
  }
}, 3_000);

window.setInterval(() => {
  if (store.openAiTask?.running) void refreshOpenAiTask();
}, 1_000);

window.setInterval(() => {
  if (proxyPolling || nodeSelectionBusy || document.hidden || !["overview", "proxies"].includes(store.view) || store.runtime?.phase !== "running") return;
  proxyPolling = true;
  void refreshProxies(true).finally(() => { proxyPolling = false; });
}, 10_000);

void listen<GlobalTrafficSnapshot>("global-traffic", (event) => {
  store.globalTraffic = event.payload;
  renderGlobalTraffic();
});

void listen<SessionResumeStatus>("session-resume-status", (event) => {
  sessionResumeRevision++;
  sessionResumeStatus = event.payload;
  renderSessionResume();
  renderHeader();
  if (!sessionResumePresentation(event.payload).busy) void refreshBase();
}).then(() => refreshSessionResume()).catch(() => { void refreshSessionResume(); });

void listen<NetworkSafetyReport>("network-safety-report", (event) => {
  store.networkSafety = event.payload;
  renderSubscriptions();
  if (event.payload.warnings?.length) toast(event.payload.warnings.join("；"), "info");
});

void listen<string>("navigate-view", (event) => {
  if (event.payload === "overview") navigate("overview");
});

void refreshBase();
