import type { RouteDiagnostic, RouteProbe, RouteSettings, RouteSnapshot } from "./types";
import { preferenceSwitch } from "./ui";

const escape = (value: string) => value.replace(/[&<>"']/g, c =>
  ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

export function routeSettingsError(s: RouteSettings): { field: "port" | "mode" | "upstream" | "proxy"; message: string } | null {
  if (!Number.isInteger(s.listenPort) || s.listenPort < 1024 || s.listenPort > 65535) return { field: "port", message: "路由端口必须为 1024–65535。" };
  if (!["native", "compatible"].includes(s.mode)) return { field: "mode", message: "请选择有效的传输模式。" };
  if (!["chatgpt", "openai_api"].includes(s.upstream)) return { field: "upstream", message: "请选择有效的官方入口。" };
  if (s.outboundProxy) {
    try {
      const u = new URL(s.outboundProxy);
      if (!["http:", "https:", "socks5:", "socks5h:"].includes(u.protocol) || !["127.0.0.1", "localhost", "[::1]"].includes(u.hostname)
          || !u.port || Number(u.port) === s.listenPort || u.username || u.password || u.search || u.hash || !["", "/"].includes(u.pathname))
        return { field: "proxy", message: "出站代理仅支持本机地址和明确端口，不能包含账号、路径或指向路由自身。" };
    } catch { return { field: "proxy", message: "出站代理地址格式不正确。" }; }
  }
  return null;
}
export function validateRouteSettings(s: RouteSettings): string | null {
  return routeSettingsError(s)?.message ?? null;
}
export function modelHealthLabel(n: RouteSnapshot["stability"]["nodes"][number]): string {
  if (n.modelInterrupted > 0) return `已观察到 ${n.modelInterrupted} 次模型流异常`;
  if (n.modelCompleted > 0) return `已完成 ${n.modelCompleted} 次模型流`;
  return "模型流尚未验证";
}
export function probeLabel(probe?: RouteProbe, now = Date.now()): string {
  if (!probe || !probe.checkedAt || now / 1000 - probe.checkedAt > 240 || probe.state === "unknown") return "未检测 / 已过期 / 暂不可用";
  if (probe.state === "http_unverified") return "HTTP 可达，预期响应未验证";
  if (probe.state === "failed") return "检测失败";
  return `基础检测通过${probe.latencyMs == null ? "" : ` · ${probe.latencyMs} ms`}`;
}
export function routeDiagnosticLabel(d: RouteDiagnostic): string {
  const stage = ({request:"发送请求",response_headers:"等待响应头",stream:"读取数据流",stream_event:"模型返回事件"} as Record<string,string>)[d.stage] ?? "连接阶段";
  return `${new Date(d.timestamp).toLocaleString()} · ${d.target === "openai_api" ? "OpenAI API" : "ChatGPT"} · ${stage} · ${d.elapsedMs} ms · ${d.httpStatus == null ? "未收到上游 HTTP 响应" : `上游 HTTP ${d.httpStatus}`} · ${d.attribution === "verified" ? "实际出口已核对" : "实际出口未确认"}\n${d.code}：${d.message}`;
}
export function routeMetricsMarkup(state: Pick<RouteSnapshot, "requests" | "active" | "completed" | "failed"> | null): string {
  return [["请求",state?.requests],["进行中",state?.active],["转发完成",state?.completed],["未确认完整（含取消）",state?.failed]]
    .map(([label,value]) => `<div><dt>${label}</dt><dd>${typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : "—"}</dd></div>`).join("");
}
const infoIcon = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M12 11v6m0-10v1"/></svg>';
const selectChevron = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg>';
export const localRoutingMarkup = `
<div class="local-route-layout">
  <p id="local-route-feedback" class="local-route-feedback" role="status" aria-live="polite" data-quiet="true">正在读取路由状态…</p>
  <article class="local-route-workspace" aria-label="Codex 路由设置与接入">
    <section class="local-route-service" aria-labelledby="local-route-service-title">
      <h2 id="local-route-service-title">路由服务</h2>
      <div class="local-route-service-state"><span id="local-route-status">尚未读取</span>
        <label class="local-route-toggle"><span>启用路由</span>${preferenceSwitch("local-route-enabled")}</label>
      </div>
      <p class="local-route-hint local-route-pending" id="local-route-pending" role="status" hidden></p>
      <div class="local-route-endpoint-field"><span id="local-route-endpoint-label">端点</span>
        <div class="local-route-endpoint-row"><code id="local-route-endpoint" aria-labelledby="local-route-endpoint-label">—</code><button class="button button-quiet" id="local-route-refresh" type="button">刷新状态</button></div>
      </div>
      <p class="local-route-hint">默认关闭，仅转发模型请求，不接管系统代理。</p>
      <p class="local-route-hint local-route-state-help" id="local-route-start-help" hidden></p>
    </section>
    <section class="local-route-transport" aria-labelledby="local-route-transport-title">
      <h2 id="local-route-transport-title">连接方式</h2>
      <form id="local-route-form" novalidate><fieldset id="local-route-fields" disabled>
        <div class="local-route-field"><label for="local-route-mode">传输模式</label>
          <div class="local-route-select"><select id="local-route-mode" aria-describedby="local-route-mode-help local-route-mode-error"><option value="compatible">兼容模式 · HTTP/SSE</option><option value="native">原生模式 · WebSocket</option></select>${selectChevron}</div>
          <p class="local-route-hint" id="local-route-mode-help">可选原生 WebSocket 模式</p><p class="local-route-field-error" id="local-route-mode-error" hidden></p>
        </div>
        <div class="local-route-fields-row">
          <div class="local-route-field"><label for="local-route-port">监听端口</label><input id="local-route-port" type="number" min="1024" max="65535" required value="15731" aria-describedby="local-route-port-error" /><p class="local-route-field-error" id="local-route-port-error" hidden></p></div>
          <div class="local-route-field"><label for="local-route-upstream">官方入口</label><div class="local-route-select"><select id="local-route-upstream" aria-describedby="local-route-upstream-error"><option value="chatgpt">ChatGPT 登录</option><option value="openai_api">OpenAI API Key</option></select>${selectChevron}</div><p class="local-route-field-error" id="local-route-upstream-error" hidden></p></div>
        </div>
        <div class="local-route-field"><label for="local-route-proxy">出站代理（可选）</label>
          <input id="local-route-proxy" type="text" placeholder="留空使用 Serylane 本地代理" autocomplete="off" spellcheck="false" aria-describedby="local-route-proxy-help local-route-proxy-error" />
          <p class="local-route-hint" id="local-route-proxy-help">仅本机 HTTP(S) / SOCKS5，无账号密码。</p><p class="local-route-field-error" id="local-route-proxy-error" hidden></p>
        </div>
      </fieldset><div class="local-route-actions"><button class="button button-primary" id="local-route-save" type="submit">保存设置</button><button class="button button-quiet" id="local-route-reset" type="button">撤销编辑</button></div></form>
      <p class="local-route-hint">保存不启动、不接入。</p>
      <p class="local-route-hint local-route-state-help" id="local-route-form-help" hidden></p>
    </section>
    <section class="local-route-binding" aria-labelledby="local-route-binding-title">
      <h2 id="local-route-binding-title">接入与恢复</h2>
      <dl class="local-route-info"><dt>当前提供方</dt><dd id="local-route-provider">—</dd><dt>接入状态</dt><dd id="local-route-binding">尚未读取</dd><dt>当前入口</dt><dd id="local-route-codex-endpoint">—</dd></dl>
      <div class="local-route-actions"><button class="button button-primary" id="local-route-attach" type="button" disabled>接入 Codex</button><button class="button button-quiet" id="local-route-restore" type="button" disabled>恢复原配置</button></div>
      <p class="local-route-hint" id="local-route-binding-help">先启动路由，再单独确认接入。</p>
      <p class="local-route-hint">接入前备份配置，保留官方登录；不修改 auth.json。</p>
      <p class="local-route-hint">在新会话核对接入；必要时自行重启 Codex。</p>
      <p class="local-route-hint local-route-state-help" id="local-route-binding-warning" hidden></p>
      <p class="local-route-hint local-route-backup" id="local-route-backup" hidden></p>
    </section>
    <section class="local-route-statistics" aria-labelledby="local-route-statistics-title"><h2 id="local-route-statistics-title">本次请求</h2>
      <dl class="local-route-metrics" id="local-route-metrics">${routeMetricsMarkup(null)}</dl>
      <p class="local-route-hint local-route-state-help" id="local-route-last-error" hidden></p>
      <p class="local-route-hint local-route-diagnostic" id="local-route-diagnostic" hidden></p>
    </section>
  </article>
  <article class="local-route-stability-panel" aria-labelledby="local-route-stability-title">
    <div class="local-route-stability-heading"><h2 id="local-route-stability-title">OpenAI 稳定灾备</h2><label class="local-route-toggle"><span>稳定策略</span>${preferenceSwitch("local-route-stability")}</label></div>
    <div class="local-route-stability-overview"><div class="local-route-stability-summary"><strong id="local-route-node">尚无托管节点</strong><p class="local-route-hint" id="local-route-stability-message">先在「代理」生成灾备，再启用稳定策略。</p><p class="local-route-hint">最近 15 分钟评分 · 故障冷却 5 分钟 · 连续 3 次恢复检查</p></div>
      <div class="local-route-evidence"><p>${infoIcon}<span>基础可达不等于模型流已验证</span></p><p id="local-route-check-state" class="local-route-hint"></p><p>${infoIcon}<span id="local-route-evidence-message">无模型样本，不代表连接已验证。</span></p></div>
      <details class="local-route-help"><summary>使用与恢复说明<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg></summary>
        <div class="local-route-help-content">
          <section><h3>使用边界</h3><p>默认关闭。仅转发 Codex 模型 API，不接管整个应用，不修改系统代理，不关闭运行中的程序。关窗后留在托盘；退出 Serylane 会尝试恢复原配置，下次需再次接入。</p><p>切换出口无法续接已经中断的数据流；兼容模式也不保证永不断线。</p></section>
          <section><h3>连接方式与接入</h3><p>兼容模式减少对 WebSocket 的依赖，但仍需要稳定出口；部分实时能力需在新会话验证。原生模式透传 WebSocket，不声称已验证模型完成。</p><p>出站代理留空时使用 Serylane 当前代理端口。指定其他代理时，不将流异常归因给 Serylane 节点。API Key 模式仍使用 Codex 自己的登录配置。</p><p>修改连接方式前，请先恢复 Codex 接入并关闭路由。接入不会迁移正在进行的请求。恢复后若原入口是 CC Switch，仍需开启其服务。</p></section>
          <section><h3>稳定灾备与模型验证</h3><p>按最近 15 分钟样本评分，模型流权重高于基础检测。健康节点保持使用；连续失败后冷却 5 分钟，恢复需连续 3 次检查通过。只改变后续新连接，不清空正常连接、不自动重发模型请求。</p><p>每轮结束约 60 秒后检测 ChatGPT 与 OpenAI API，网络异常可触发限频复查。最多 10 个预算内候选，同时只检查 2 个节点，不执行模型请求或带宽测速。预期 401 仅表示 API 可达；非预期 HTTP 响应不计为节点失败。默认依据 ChatGPT；本地路由启用且选择 API Key 入口时依据 OpenAI API，两者证据不混用。</p><p>多个出口同时失败时先复查公共网络或服务状态，不逐个处罚节点；手动选点始终优先。模型样本来自兼容路由，需核对实际连接出口且期间托管节点未变化。无法确认出口时仅统计请求异常，不归因节点；取消请求和关闭原生隧道不等同于节点故障。应用日志只保留分类与耗时，不保存请求正文、认证信息或路由密钥。</p></section>
        </div>
      </details>
    </div>
    <div class="local-route-node-list" id="local-route-nodes" hidden></div>
  </article>
  <p class="local-route-hint local-route-footer">已中断的数据流无法续接；切换仅影响新连接，不自动重发请求。</p>
</div>`;

type Services = {
  api: {
    localRouteStatus(): Promise<RouteSnapshot>;
    saveLocalRoute(settings: RouteSettings, expectedRevision: number): Promise<RouteSnapshot>;
    setLocalRouteEnabled(enabled: boolean, expectedRevision: number, confirmed: boolean): Promise<RouteSnapshot>;
    setCodexRoute(attach: boolean, expectedConfigRevision: string, confirmed: boolean): Promise<RouteSnapshot>;
    setOpenAiStability(enabled: boolean, profileId: string, revisionId: string, confirmed: boolean): Promise<void>;
  };
  confirm(options: { title: string; message: string; confirmLabel?: string; returnFocus?: HTMLElement | null }): Promise<boolean>;
  error(error: unknown): string;
};
export function mountLocalRouting(root: HTMLElement, services: Services) {
  const $ = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#local-route-${id}`)!;
  let state: RouteSnapshot | null = null, busy = false, dirty = false, draftRevision = 0;
  function message(text: string, error = false, quiet = false) {
    $("feedback").textContent = text;
    $("feedback").dataset.error = String(error);
    $("feedback").dataset.tone = error ? (/冲突|已变化|已被其他|进行中/.test(text) ? "warning" : "error") : "info";
    $("feedback").dataset.quiet = String(quiet);
  }
  function optionalText(id: string, text: string) { $(id).textContent = text; $(id).hidden = !text; }
  function clearValidation() {
    for (const id of ["port", "mode", "upstream", "proxy"]) {
      $(id).removeAttribute("aria-invalid"); optionalText(`${id}-error`, "");
    }
  }
  function pending(text: string) { $("pending").textContent = text; }
  function fill() {
    if (!state) return;
    $<HTMLInputElement>("port").value = String(state.settings.listenPort);
    $<HTMLSelectElement>("mode").value = state.settings.mode;
    $<HTMLSelectElement>("upstream").value = state.settings.upstream;
    $<HTMLInputElement>("proxy").value = state.settings.outboundProxy;
    draftRevision = state.revision;
  }
  function render() {
    root.setAttribute("aria-busy", String(busy));
    $("pending").hidden = !busy;
    $<HTMLFieldSetElement>("fields").disabled = busy || !state || state.enabled || state.codex.hasBackup;
    $<HTMLButtonElement>("save").disabled = $<HTMLFieldSetElement>("fields").disabled;
    $<HTMLInputElement>("enabled").checked = state?.running ?? false;
    $<HTMLInputElement>("enabled").disabled = busy || !state || (dirty && !state.running);
    $<HTMLButtonElement>("reset").disabled = busy || !state || !dirty;
    $<HTMLButtonElement>("refresh").disabled = busy;
    $<HTMLInputElement>("stability").checked = state?.stability.enabled ?? false;
    $<HTMLInputElement>("stability").disabled = busy || !state?.stability.eligible;
    $<HTMLButtonElement>("attach").disabled = busy || !state?.running || state.codex.hasBackup || state.active > 0;
    $<HTMLButtonElement>("restore").disabled = busy || !state?.codex.hasBackup || state.active > 0;
    if (!state) return;
    optionalText("form-help", state.enabled || state.codex.hasBackup ? "修改连接方式前，请先恢复 Codex 接入并关闭路由。" : dirty ? "有未保存的修改。" : "");
    optionalText("start-help", dirty && !state.running ? "请先保存设置，再启动路由。" : "");
    $("status").textContent = state.running ? (state.settings.mode === "compatible" ? "运行中 · HTTP 流式" : "运行中 · WebSocket") : state.enabled ? "启动失败 · 可重试" : "已关闭";
    $("status").dataset.state = state.running ? "running" : state.enabled ? "error" : "off";
    $("endpoint").textContent = state.endpoint; $("provider").textContent = state.codex.provider;
    $("binding").textContent = state.codex.attached ? "已写入 Serylane 接入配置" : state.codex.hasBackup ? "未接入或配置已变化" : "未接入";
    $("codex-endpoint").textContent = state.codex.endpoint ?? "官方默认入口";
    $("binding-help").textContent = state.active > 0 ? "仍有进行中的请求，请等待结束后更改接入；未中断请求。" : state.codex.attached ? "接入配置已写入；尚未验证新会话是否生效。" : state.codex.hasBackup ? "存在接入备份，请先恢复原配置。" : state.running ? "路由已启动，接入 Codex 需要单独确认。" : "先启动路由，再单独确认接入。";
    optionalText("binding-warning", state.codex.warning ?? "");
    optionalText("backup", state.codex.backupPath ? `恢复备份：${state.codex.backupPath}` : "");
    $("metrics").innerHTML = routeMetricsMarkup(state);
    optionalText("last-error", [state.lastStatus ? `最近路由 HTTP 状态：${state.lastStatus}` : "", state.lastDiagnostic ? "" : state.lastError].filter(Boolean).join(" · "));
    optionalText("diagnostic", state.lastDiagnostic ? `最近异常（历史记录，不代表当前连接）\n${routeDiagnosticLabel(state.lastDiagnostic)}` : "");
    $("node").textContent = state.stability.current ?? "尚无托管节点";
    $("stability-message").textContent = state.stability.message || "先在「代理」生成灾备，再启用稳定策略。";
    const stable = state.stability;
    const basis = stable.selectionTarget === "openai_api" ? "OpenAI API" : "ChatGPT";
    $("check-state").textContent = `${stable.running ? "自动检测运行中" : stable.enabled ? "已开启 · 自动检测未运行" : "稳定策略未开启"} · 选点依据：${basis} · ${stable.lastCheck ? `最近检查 ${new Date(stable.lastCheck * 1000).toLocaleTimeString()}` : "尚无双目标检测"}${stable.commonFailure ? " · 多出口同时失败，等待复查" : ""}`;
    $("evidence-message").textContent = state.stability.nodes.some(n => n.modelCompleted > 0 || n.modelInterrupted > 0) ? "模型样本与节点状态见下方。" : "无模型样本，不代表连接已验证。";
    $("nodes").hidden = state.stability.nodes.length === 0;
    $("nodes").innerHTML = state.stability.nodes.map(n => `<section class="local-route-node"><div><strong>${escape(n.name)}</strong><p class="local-route-hint">${escape(modelHealthLabel(n))}</p><p class="local-route-hint">${n.cooldownSeconds > 0 ? `冷却 ${n.cooldownSeconds} 秒` : n.probeOk ? "可参与自动选点" : "待检测 / 待恢复"} · ${n.samples} 个近期样本 · 加权成功率 ${n.successRate == null ? "—" : `${n.successRate}%`}</p></div><div class="local-route-probes"><p>ChatGPT：${escape(probeLabel(n.chatgpt))}</p><p>OpenAI API：${escape(probeLabel(n.openaiApi))}</p></div></section>`).join("");
  }
  async function operation(work: () => Promise<void>, label = "处理中…", returnFocus?: HTMLElement) {
    if (busy) return; busy = true; pending(label); render();
    try { await work(); } catch(e) { message(services.error(e),true); }
    finally {
      busy = false; render();
      if (returnFocus) {
        const target = (returnFocus as HTMLButtonElement).disabled ? $("refresh") : returnFocus;
        target.focus({ preventScroll: true });
      }
    }
  }
  async function refresh() {
    await operation(async () => {
      state = await services.api.localRouteStatus();
      if (!dirty) fill();
      const conflict = dirty && draftRevision !== state.revision;
      message(conflict ? "设置已变化；编辑内容已保留，请撤销编辑后重新读取。" : "状态已刷新；没有修改代理或 Codex 配置。", conflict, !conflict);
    }, "正在读取状态…");
  }
  $("refresh").addEventListener("click", () => void refresh());
  for (const event of ["input","change"]) $("form").addEventListener(event, () => {
    if (busy || $<HTMLFieldSetElement>("fields").disabled) return;
    dirty = true; clearValidation(); render();
  });
  $("reset").addEventListener("click", () => {
    if (busy || !state || !dirty) return;
    dirty = false; clearValidation(); fill(); render(); message("已恢复已保存设置。");
  });
  $("form").addEventListener("submit", e => {
    e.preventDefault(); if (!state || busy || $<HTMLFieldSetElement>("fields").disabled) return;
    const settings: RouteSettings = { listenPort: Number($<HTMLInputElement>("port").value), mode: $<HTMLSelectElement>("mode").value as RouteSettings["mode"],
      upstream: $<HTMLSelectElement>("upstream").value as RouteSettings["upstream"], outboundProxy: $<HTMLInputElement>("proxy").value.trim() };
    clearValidation();
    const error = routeSettingsError(settings);
    if (error) {
      $(error.field).setAttribute("aria-invalid", "true"); optionalText(`${error.field}-error`, error.message);
      message(error.message,true); $(error.field).focus(); return;
    }
    void operation(async () => { state = await services.api.saveLocalRoute(settings,draftRevision); dirty = false; fill(); message("设置已保存；服务和 Codex 接入未自动开启。"); }, "正在保存设置…");
  });
  $("enabled").addEventListener("change", () => {
    if (!state || $<HTMLInputElement>("enabled").disabled) { render(); return; }
    const before = state, enabled = $<HTMLInputElement>("enabled").checked;
    void operation(async () => {
      if (!await services.confirm({ title: enabled ? "启动本地路由？" : "关闭路由并恢复接入？", message: enabled ? "仅监听本机端口，不会自动修改 Codex 配置。启用后随 Serylane 启动。" : "先恢复由 Serylane 管理的 Codex 配置，再停止服务。有进行中请求时将拒绝关闭。原配置如果依赖 CC Switch，需要其服务保持运行。", confirmLabel: enabled ? "启动路由" : "恢复并关闭", returnFocus: $("enabled") })) return;
      pending(enabled ? "正在启动路由…" : "正在恢复并关闭…");
      state = await services.api.setLocalRouteEnabled(enabled,before.revision,true); if (!dirty) fill();
      message(enabled ? "路由已启动。接入 Codex 需要下面单独确认。" : "路由已关闭，原接入配置已恢复。");
    }, "正在确认…", $("enabled"));
  });
  for (const [id,attach] of [["attach",true],["restore",false]] as const) $(id).addEventListener("click", () => {
    if (!state || $<HTMLButtonElement>(id).disabled) return; const before = state;
    void operation(async () => {
      if (!await services.confirm({ title: attach ? "备份并接入 Codex？" : "恢复原 Codex 接入？", message: attach ? "备份用户级 config.toml，仅更改模型提供方接入。保留官方登录和其他设置，不退出 Codex。请在新会话核对，必要时自行重启；部分实时能力需验证。" : "只还原 Serylane 管理的字段，保留其余编辑；遇到其他路由程序修改将停止恢复。原 CC Switch 接入需要其服务开启。", confirmLabel: attach ? "备份并接入" : "恢复原配置", returnFocus: $(id) })) return;
      pending(attach ? "正在备份并接入…" : "正在恢复配置…");
      state = await services.api.setCodexRoute(attach,before.codex.configRevision,true);
      message(attach ? "接入配置已写入；尚未验证新会话是否生效。" : "已恢复原接入配置，历史备份仍保留。");
    }, "正在确认…", $(id));
  });
  $("stability").addEventListener("change", () => {
    if (!state?.stability.profileId || !state.stability.revisionId || $<HTMLInputElement>("stability").disabled) { render(); return; }
    const before = state.stability, enabled = $<HTMLInputElement>("stability").checked;
    void operation(async () => {
      if (!await services.confirm({ title: enabled ? "启用稳定灾备？" : "恢复基础灾备？", message: "为当前配置生成新版本并应用核心配置，后续连接改用所选策略。不主动清空连接；请避开重要请求。稳定策略不会把基础检查当成模型验证。", confirmLabel: "确认应用", returnFocus: $("stability") })) return;
      pending("正在应用稳定策略…");
      await services.api.setOpenAiStability(enabled,before.profileId!,before.revisionId!,true);
      state = await services.api.localRouteStatus(); message("策略已应用。后台每分钟更新一次状态，请稍后刷新。");
    }, "正在确认…", $("stability"));
  });
  render();
  return { refresh };
}
