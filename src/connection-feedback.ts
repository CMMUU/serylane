import type { ConnectionFeedback, UserMessage } from "./types";

const escape = (value: string) => value.replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));
const actionNames: Record<string, string> = { retry: "重试", refresh: "刷新状态", settings: "打开设置", profiles: "查看配置", subscriptions: "查看订阅", diagnostics: "查看诊断", recheck: "重新检查", details: "查看详情" };
export function friendlyError(error: unknown): UserMessage {
  const value = error && typeof error === "object" ? error as { userMessage?: UserMessage; code?: string } : {};
  if (value.userMessage && typeof value.userMessage.title === "string" && typeof value.userMessage.description === "string") return value.userMessage;
  // No parsing of English exception text: unknown errors remain unknown.
  const known: Record<string, [string, string, string]> = {
    CONFIG_ERROR: ["代理配置需要检查", "请检查当前选用的配置，再重新开启代理。", "profiles"],
    NETWORK_CHECK_FAILED: ["连接检查暂未通过", "检测请求未收到预期响应，请查看诊断结果。", "diagnostics"],
    STATE_CONFLICT: ["当前操作暂未完成", "配置或运行状态可能正在变化，请刷新后重试。", "refresh"],
    CORE_ERROR: ["代理服务操作未完成", "请重试或查看应用日志。", "retry"],
    PLATFORM_ERROR: ["系统设置操作未完成", "请检查系统权限和当前网络设置。", "settings"],
  };
  const [title, description, action] = known[value.code ?? ""] ?? ["操作暂未完成", "请重试，或在应用日志中查看具体原因。", "diagnostics"];
  return { title, description, action, details: value.code ?? "" };
}
export function feedbackPresentation(state: ConnectionFeedback | null, issue: UserMessage | null = null) {
  if (issue) return { visible: true, title: issue.title, description: issue.description, tone: "error", busy: false, action: issue.action, details: issue.details };
  if (!state || state.phase === "idle") return { visible: false, title: "", description: "", tone: "neutral", busy: false, action: "", details: "" };
  if (state.issue && state.phase === "failed") return feedbackPresentation(null, state.issue);
  const progress: Record<string, string> = { validating: "正在检查代理配置…", starting: "正在启动代理服务…", applying: "正在应用代理设置…" };
  if (progress[state.phase]) return { visible: true, title: progress[state.phase], description: "完成后会更新状态，请稍候。", tone: "neutral", busy: true, action: "", details: "" };
  const enabled = state.mode === "system_proxy" ? "系统代理已开启" : state.mode === "tun" ? "TUN 模式已开启" : "本地代理已开启";
  if (state.health === "checking") return { visible: true, title: enabled, description: state.retrying ? "连接检查用时较长，正在重试。" : "正在检查网络连接。", tone: "neutral", busy: true, action: "", details: "" };
  const checks = state.checks;
  const basicOk = checks.some(c => c.target !== "openai" && c.success);
  const ai = checks.find(c => c.target === "openai");
  let description = "联网状态尚未检查。";
  if (state.health === "healthy") description = "基础连接检查通过。OpenAI 接口可达，模型会话尚未验证。";
  else if (state.health === "partial") description = basicOk && ai && !ai.success
    ? "基础连接正常，OpenAI 检查未通过。代理保持开启。"
    : "部分检测请求未收到预期响应。代理保持开启，可查看详情。";
  else if (state.health === "unavailable") {
    description = checks.some(c => c.failureKind === "certificate")
      ? "安全连接校验未通过。请确认系统日期和时间，并查看详情。"
      : checks.some(c => c.failureKind === "tls")
        ? "连接建立时中断。代理保持开启，可以重新检查。"
        : "连接检查暂未通过。代理保持开启，请重试或查看诊断。";
  }
  return { visible: true, title: enabled, description, tone: ["partial", "unavailable"].includes(state.health) ? "warning" : "neutral", busy: false, action: state.mode === "tun" ? "diagnostics" : "recheck", details: state.issue?.details ?? "" };
}
export const connectionFeedbackMarkup = `<aside id="connection-feedback" class="connection-feedback is-hidden" aria-label="代理状态与连接检查">
  <div class="connection-feedback-summary" role="status" aria-live="polite" aria-atomic="true"><strong id="connection-feedback-title"></strong><p id="connection-feedback-description"></p></div>
  <div class="connection-feedback-controls"><span id="connection-feedback-elapsed" aria-live="off"></span><button id="connection-feedback-action" class="button button-quiet" type="button"></button>
  <details id="connection-feedback-details"><summary>技术详情</summary><pre id="connection-feedback-detail-text"></pre><button id="connection-feedback-diagnostics" class="button button-quiet" type="button">查看诊断</button></details></div>
</aside>`;
export function mountConnectionFeedback(root: HTMLElement, action: (name: string) => Promise<void>) {
  let state: ConnectionFeedback | null = null, issue: UserMessage | null = null, receivedAt = performance.now(), actionBusy = false;
  const find = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#connection-feedback-${id}`)!;
  let presentation = feedbackPresentation(null);
  function render() {
    presentation = feedbackPresentation(state, issue);
    root.classList.toggle("is-hidden", !presentation.visible);
    root.dataset.tone = presentation.tone;
    find("title").textContent = presentation.title;
    find("description").textContent = presentation.description;
    const button = find<HTMLButtonElement>("action");
    button.hidden = !presentation.action;
    button.disabled = actionBusy;
    button.textContent = actionNames[presentation.action] ?? "查看详情";
    const lines = state?.checks.map(c => `${c.target === "openai" ? "OpenAI" : c.target === "google" ? "Google" : "Cloudflare"} · ${c.success ? "通过" : "未通过"} · ${c.latencyMs} ms\n${c.detail}`) ?? [];
    find("detail-text").textContent = [presentation.details, ...lines].filter(Boolean).join("\n\n") || "当前没有更多诊断信息。";
    tick();
  }
  function tick() {
    find("elapsed").textContent = presentation.busy && state ? `已等待 ${Math.floor((state.elapsedMs + performance.now() - receivedAt) / 1000)} 秒` : "";
  }
  const timer = window.setInterval(tick, 500);
  const invoke = async (name: string) => {
    if (actionBusy) return;
    if (name === "details") { (find("details") as HTMLDetailsElement).open = true; return; }
    actionBusy = true; render();
    try { await action(name); } catch (error) { issue = friendlyError(error); }
    finally { actionBusy = false; render(); }
  };
  find("action").addEventListener("click", () => void invoke(presentation.action));
  find("diagnostics").addEventListener("click", () => void invoke("diagnostics"));
  return {
    accept(next: ConnectionFeedback) {
      if (state && next.revision < state.revision) return;
      if (next.phase === "failed" || (["validating", "starting", "applying", "enabled"].includes(next.phase)
        && (!state || next.operation > state.operation))) issue = null;
      state = next; receivedAt = performance.now(); render();
    },
    showError(error: unknown) { issue = friendlyError(error); render(); },
    clearIssue() { issue = null; render(); },
    snapshot: () => state,
    dispose() { window.clearInterval(timer); },
  };
}
// Text escaping is shared with tests for any future markup-based diagnostics.
export const escapeFeedbackText = escape;
