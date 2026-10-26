import type { UserRule, UserRulesState, UserRulesValidation } from "./types";

export const RULE_KINDS = [
  "DOMAIN", "DOMAIN-SUFFIX", "DOMAIN-KEYWORD", "IP-CIDR", "IP-CIDR6",
  "PROCESS-NAME", "PROCESS-PATH", "NETWORK", "DST-PORT",
] as const;
export type VisualRule = { kind: string; value: string; target: string; noResolve: boolean };
const cloneRules = (rules: UserRule[]) => rules.map(({ id, enabled, rule, note }) => ({ id, enabled, rule, note }));
export const rulesFingerprint = (rules: UserRule[]) => JSON.stringify(cloneRules(rules));

export function parseVisualRule(rule: string): VisualRule | null {
  const fields = rule.split(",").map((part) => part.trim());
  const [kind, value, target] = fields;
  const ip = kind === "IP-CIDR" || kind === "IP-CIDR6";
  if (!(RULE_KINDS as readonly string[]).includes(kind) || !value || !target) return null;
  if (fields.length !== 3 && !(ip && fields.length === 4 && fields[3] === "no-resolve")) return null;
  return { kind, value, target, noResolve: fields.length === 4 };
}

export function visualRuleText(rule: VisualRule): string {
  const ip = rule.kind === "IP-CIDR" || rule.kind === "IP-CIDR6";
  return `${rule.kind},${rule.value.trim()},${rule.target.trim()}${ip && rule.noResolve ? ",no-resolve" : ""}`;
}

export function serializeUserRules(rules: UserRule[]): string {
  // Retain the pre-RouteDeck export format for backward-compatible rule imports.
  return rules.map(({ id, enabled, rule, note }) =>
    `# mihomo-codex-rule: ${JSON.stringify({ id, enabled, note })}\n${rule}`,
  ).join("\n\n");
}

export async function copyRuleText(text: string, writeText: (text: string) => Promise<void>): Promise<boolean> {
  try {
    await writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** Owns a local draft independently of refreshes, subscription changes, and network settings. */
export class RulesDraft {
  base: UserRulesState | null = null;
  latest: UserRulesState | null = null;
  rules: UserRule[] = [];
  text = "";
  textPending = false;
  epoch = 0;
  private validated = "";

  get dirty(): boolean {
    return this.textPending || (this.base !== null && rulesFingerprint(this.rules) !== rulesFingerprint(this.base.rules));
  }
  get conflict(): boolean { return !!this.base && !!this.latest && this.base.revision !== this.latest.revision; }
  get canSave(): boolean {
    return !!this.base && this.dirty && !this.conflict && !this.textPending && this.validated === rulesFingerprint(this.rules);
  }
  sync(state: UserRulesState): boolean {
    if (state.revision < (this.latest?.revision ?? -1)) return false;
    this.latest = state;
    if (!this.dirty) this.accept(state);
    return true;
  }
  accept(state: UserRulesState): void {
    this.base = { ...state, rules: cloneRules(state.rules) };
    this.latest = state;
    this.rules = cloneRules(state.rules);
    this.text = serializeUserRules(this.rules);
    this.textPending = false;
    this.validated = "";
    this.epoch++;
  }
  replace(rules: UserRule[]): void {
    this.rules = cloneRules(rules);
    this.text = serializeUserRules(this.rules);
    this.textPending = false;
    this.validated = "";
    this.epoch++;
  }
  editText(text: string): void {
    this.text = text;
    this.textPending = text !== serializeUserRules(this.rules);
    this.validated = "";
    this.epoch++;
  }
  move(id: string, offset: -1 | 1): void {
    const rules = cloneRules(this.rules);
    const index = rules.findIndex((rule) => rule.id === id);
    const destination = index + offset;
    if (index < 0 || destination < 0 || destination >= rules.length) return;
    [rules[index], rules[destination]] = [rules[destination], rules[index]];
    this.replace(rules);
  }
  markValidated(result: UserRulesValidation, expectedEpoch: number): boolean {
    if (expectedEpoch !== this.epoch) return false;
    if (result.valid) {
      this.replace(result.normalizedRules);
      this.validated = rulesFingerprint(this.rules);
    } else this.validated = "";
    return true;
  }
  undo(): void { if (this.latest) this.accept(this.latest); }
}

export const ruleManagerMarkup = `
<article class="panel rule-manager-panel">
  <div class="panel-heading"><div><div class="section-label">RULES</div><h2>规则管理</h2><p class="rule-manager-intro">独立保存的本地规则，不随订阅更新被覆盖。</p></div><button type="button" class="button button-quiet" id="user-rules-refresh">刷新状态</button></div>
  <div class="rule-priority-note"><strong>我的规则 → AI 托管规则 → 订阅规则</strong><span>按列表从上到下匹配，首个命中生效。DIRECT 是经本机核心直连，不等于绕过系统代理或 TUN。</span></div>
  <div class="rule-tabs" role="tablist" aria-label="规则视图">
    <button type="button" role="tab" id="user-rules-tab" aria-controls="user-rules-panel" aria-selected="true" data-rule-tab="visual">我的规则 <span id="user-rules-count">0</span></button>
    <button type="button" role="tab" id="advanced-rules-tab" aria-controls="advanced-rules-panel" aria-selected="false" tabindex="-1" data-rule-tab="advanced">高级文本</button>
    <button type="button" role="tab" id="effective-rules-tab" aria-controls="effective-rules-panel" aria-selected="false" tabindex="-1" data-rule-tab="effective">生效规则</button>
  </div>
  <p id="user-rules-status" class="rule-manager-status" role="status" aria-live="polite">正在读取本地规则…</p>
  <div id="user-rules-notices" class="rule-manager-notices"></div>
  <section id="user-rules-panel" class="rules-tab-panel" role="tabpanel" aria-labelledby="user-rules-tab">
    <form id="user-rule-form" class="user-rule-form">
      <div class="user-rule-editor-heading"><strong id="user-rule-editor-title">添加规则</strong><span>先加入草稿，再校验并应用</span></div>
      <label><span>匹配类型</span><select id="user-rule-kind">${RULE_KINDS.map((kind) => `<option value="${kind}">${kind}</option>`).join("")}</select></label>
      <label class="user-rule-value-field"><span>匹配内容</span><input id="user-rule-value" required autocomplete="off" placeholder="例如 example.com" /></label>
      <label><span>执行策略</span><input id="user-rule-target" list="user-rule-targets" required autocomplete="off" value="DIRECT" aria-describedby="user-rule-target-hint" /><datalist id="user-rule-targets"></datalist></label>
      <label class="user-rule-note-field"><span>备注（可选）</span><input id="user-rule-note" maxlength="500" placeholder="例如：开发服务直连" /></label>
      <div class="user-rule-options"><label class="checkbox-row"><input id="user-rule-enabled" type="checkbox" checked />启用</label><label class="checkbox-row is-hidden" id="user-rule-no-resolve-field"><input id="user-rule-no-resolve" type="checkbox" />不主动解析域名（no-resolve）</label><span id="user-rule-target-hint">DIRECT / REJECT / 当前配置的策略组</span></div>
      <div class="toolbar user-rule-editor-actions"><button type="submit" class="button button-primary" id="user-rule-submit">加入草稿</button><button type="button" class="button button-quiet is-hidden" id="user-rule-cancel">取消编辑</button></div>
    </form>
    <div id="user-rules-list" class="user-rules-list" aria-label="本地规则顺序"></div>
  </section>
  <section id="advanced-rules-panel" class="rules-tab-panel is-hidden" role="tabpanel" aria-labelledby="advanced-rules-tab" hidden>
    <label class="rule-text-label" for="user-rules-text">规则文本</label>
    <p class="hint">支持逐行规则、YAML 规则列表或仅含 rules 的 YAML。元数据注释保留 ID、启停状态及备注；高级规则不会被可视化编辑器改写。</p>
    <textarea id="user-rules-text" class="user-rules-text" spellcheck="false" autocapitalize="off" aria-describedby="user-rules-text-hint" placeholder="DOMAIN-SUFFIX,example.com,DIRECT"></textarea>
    <p class="hint" id="user-rules-text-hint">修改文本后，切回可视化或点击校验时解析到同一份草稿。未保存的更改不会影响当前网络。</p>
    <div class="toolbar rule-text-actions"><label class="button file-button">导入规则文本<input id="user-rules-import" type="file" accept=".txt,.yaml,.yml,text/plain" hidden /></label><button type="button" class="button" id="user-rules-export" aria-controls="user-rules-export-panel" aria-expanded="false">导出文本</button></div>
    <section id="user-rules-export-panel" class="rule-export-panel is-hidden" aria-labelledby="user-rules-export-title" hidden>
      <div class="user-rule-editor-heading"><strong id="user-rules-export-title">当前草稿的导出文本</strong><button type="button" class="button button-quiet" id="user-rules-export-close">关闭</button></div>
      <p class="hint">下面是完整规则文本，可复制到文本编辑器后自行保存为 .txt 或 .yaml。此面板不会直接写入文件。</p>
      <textarea id="user-rules-export-text" class="user-rules-text rule-export-text" readonly spellcheck="false" aria-label="只读导出规则文本" aria-describedby="user-rules-export-feedback"></textarea>
      <div class="toolbar rule-text-actions"><button type="button" class="button button-primary" id="user-rules-copy">复制文本</button><button type="button" class="button" id="user-rules-select-all">全选文本</button></div>
      <p id="user-rules-export-feedback" class="hint" role="status" aria-live="polite">文本已准备，尚未写入文件。</p>
    </section>
  </section>
  <section id="effective-rules-panel" class="rules-tab-panel is-hidden" role="tabpanel" aria-labelledby="effective-rules-tab" hidden>
    <div class="rule-effective-toolbar"><p class="hint">这里展示运行核心当前加载的规则；本地草稿需保存后才会进入该列表。</p><div class="toolbar"><input id="rule-search" aria-label="搜索生效规则" placeholder="搜索规则" /><button type="button" class="button button-quiet" id="rules-refresh">刷新生效规则</button></div></div>
    <div class="table-wrap"><table><thead><tr><th>类型</th><th>内容</th><th>策略</th></tr></thead><tbody id="rules-body"><tr><td colspan="3">启动后加载规则</td></tr></tbody></table></div><button id="rules-more" type="button" class="button button-quiet" hidden>显示更多</button>
  </section>
  <div id="user-rules-validation" class="rule-validation is-hidden" role="status" aria-live="polite"></div>
  <div class="user-rules-actions"><div class="toolbar"><button type="button" class="button" id="user-rules-validate">校验草稿</button><button type="button" class="button button-primary" id="user-rules-save" disabled>保存并应用</button><button type="button" class="button button-quiet" id="user-rules-undo" disabled>撤销草稿</button></div><span id="user-rules-save-hint">校验通过后可保存</span></div>
  <details class="user-rules-history" id="user-rules-history-section"><summary>历史版本与回滚</summary><div class="toolbar"><label for="user-rules-history">选择已保存版本</label><select id="user-rules-history" aria-label="历史规则版本"><option value="">暂无历史版本</option></select><button type="button" class="button" id="user-rules-rollback" disabled>回滚到此版本</button></div><p class="hint">回滚会校验并应用历史规则，同时保留新的版本记录。</p></details>
</article>`;

type RuleApi = {
  userRules(): Promise<UserRulesState>;
  validateUserRules(rules: UserRule[]): Promise<UserRulesValidation>;
  saveUserRules(rules: UserRule[], expectedRevision: number): Promise<UserRulesState>;
  rollbackUserRules(revisionId: string, expectedRevision: number): Promise<UserRulesState>;
  parseUserRulesText(text: string): Promise<UserRule[]>;
};
type RuleManagerOptions = {
  api: RuleApi;
  confirm(options: { title: string; message: string; confirmLabel?: string; returnFocus?: HTMLElement }): Promise<boolean>;
  error(error: unknown): string;
  onApplied(): Promise<void>;
};
const escape = (value: unknown) => String(value ?? "").replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]!);

export function mountRuleManager(root: HTMLElement, options: RuleManagerOptions) {
  const model = new RulesDraft();
  const find = <T extends HTMLElement = HTMLElement>(selector: string) => root.querySelector<T>(selector)!;
  let tab: "visual" | "advanced" | "effective" = "visual";
  let editingId: string | null = null;
  let editorDirty = false;
  let busy = "";
  let feedback = "";
  let validation: UserRulesValidation | null = null;
  let request = 0;
  const input = (id: string) => find<HTMLInputElement>(id);
  const editingForm = find<HTMLFormElement>("#user-rule-form");

  function updateControls() {
    const loaded = model.base !== null;
    root.setAttribute("aria-busy", String(!!busy));
    root.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>("button, input, select, textarea").forEach((element) => {
      if (element.id === "rules-refresh" || element.id === "rule-search" || element.id === "user-rules-export-text") return;
      element.disabled = !!busy || (!loaded && element.id !== "user-rules-refresh");
    });
    find<HTMLButtonElement>("#user-rules-save").disabled = !!busy || editorDirty || !model.canSave;
    find<HTMLButtonElement>("#user-rules-undo").disabled = !!busy || (!model.dirty && !editorDirty);
    find<HTMLButtonElement>("#user-rules-rollback").disabled = !!busy || !loaded || !input("#user-rules-history").value;
    root.querySelectorAll<HTMLButtonElement>("[data-boundary='true']").forEach((button) => { button.disabled = true; });
    find("#user-rules-status").textContent = busy
      ? ({ refresh: "正在读取规则状态…", validate: "正在校验草稿…", save: "正在保存并应用…", rollback: "正在回滚规则…", parse: "正在解析规则文本…", import: "正在导入规则文本…", undo: "等待确认撤销…", delete: "等待确认删除…", copy: "正在复制规则文本…" }[busy] ?? "正在处理…")
      : feedback || (!loaded ? "尚未加载规则，可点击刷新重试。" : `版本 ${model.base!.revision} · ${model.rules.length} 条规则 · ${model.dirty ? "有未保存更改" : "已与本地保存同步"}`);
    find("#user-rules-save-hint").textContent = editorDirty ? "先将表单更改加入草稿，或取消编辑" : model.conflict ? "版本已变化，请保留草稿或撤销后重新编辑" : model.canSave ? "校验已通过，等待应用" : model.dirty ? "更改后需重新校验" : "当前没有待保存更改";
    find("#user-rules-count").textContent = String(model.rules.length);
    const exportText = find<HTMLTextAreaElement>("#user-rules-export-text");
    if (!find("#user-rules-export-panel").hidden && exportText.value !== model.text) {
      exportText.value = model.text;
      find("#user-rules-export-feedback").textContent = "导出文本已同步为当前草稿，尚未写入文件。";
    }
  }

  function render() {
    const latest = model.latest;
    const notices = [...(latest?.warnings ?? [])];
    if (latest?.routingMode && latest.routingMode !== "rule") notices.unshift(`当前为 ${latest.routingMode === "global" ? "全局" : "直连"} 模式，规则只在 Rule 规则模式参与匹配。`);
    if (model.conflict) notices.unshift("本地保存版本已由其他操作更新。当前草稿完整保留；请先导出文本，或撤销草稿载入最新版本后再编辑。");
    find("#user-rules-notices").innerHTML = notices.map((text) => `<p>${escape(text)}</p>`).join("");
    find("#user-rule-targets").innerHTML = [...new Set(["DIRECT", "REJECT", ...(latest?.targets ?? [])])].map((target) => `<option value="${escape(target)}"></option>`).join("");
    find("#user-rules-list").innerHTML = model.rules.map((rule, index) => {
      const parsed = parseVisualRule(rule.rule);
      return `<article class="user-rule-row ${rule.enabled ? "" : "is-disabled"}" data-rule-id="${escape(rule.id)}">
        <div class="user-rule-order"><span>${index + 1}</span><input type="checkbox" data-rule-action="toggle" ${rule.enabled ? "checked" : ""} aria-label="${rule.enabled ? "停用" : "启用"}规则 ${index + 1}" /></div>
        <div class="user-rule-row-copy"><div><span class="rule-kind-badge">${escape(parsed?.kind ?? "高级规则")}</span><strong title="${escape(rule.rule)}">${escape(parsed?.value ?? rule.rule)}</strong></div><p>${escape(rule.note || "无备注")}</p></div>
        <span class="rule-target-badge">${escape(parsed?.target ?? "原样保留")}</span>
        <div class="toolbar user-rule-row-actions"><button type="button" class="button button-quiet" data-rule-action="up" aria-label="上移规则 ${index + 1}" ${index === 0 ? 'data-boundary="true"' : ""}>↑</button><button type="button" class="button button-quiet" data-rule-action="down" aria-label="下移规则 ${index + 1}" ${index === model.rules.length - 1 ? 'data-boundary="true"' : ""}>↓</button><button type="button" class="button" data-rule-action="edit">${parsed ? "编辑" : "高级编辑"}</button><button type="button" class="button button-danger" data-rule-action="delete">删除</button></div>
      </article>`;
    }).join("") || '<div class="empty-state rule-empty-state"><strong>还没有本地规则</strong><span>添加一个域名直连、拦截或指定策略组规则开始使用。</span></div>';
    const textarea = find<HTMLTextAreaElement>("#user-rules-text");
    if (textarea.value !== model.text) textarea.value = model.text;
    const history = find<HTMLSelectElement>("#user-rules-history");
    const selected = history.value;
    history.innerHTML = '<option value="">选择历史版本</option>' + (latest?.history ?? []).map((item) => `<option value="${escape(item.id)}">${escape(new Date(item.createdAt).toLocaleString())} · ${item.count} 条规则</option>`).join("");
    if ([...history.options].some((item) => item.value === selected)) history.value = selected;
    const result = find("#user-rules-validation");
    result.classList.toggle("is-hidden", !validation || tab === "effective");
    result.classList.toggle("is-invalid", !!validation && !validation.valid);
    result.innerHTML = validation ? `<strong>${validation.valid ? "✓ 校验通过" : "校验未通过"}</strong>${[...validation.errors, ...validation.warnings].map((message) => `<p>${escape(message)}</p>`).join("")}${validation.valid && validation.preview ? `<details><summary>查看合并预览</summary><pre>${escape(validation.preview)}</pre></details>` : ""}` : "";
    updateControls();
  }

  async function run(name: string, operation: () => Promise<void>) {
    if (busy) return;
    busy = name;
    feedback = "";
    updateControls();
    try { await operation(); }
    catch (error) { feedback = `${options.error(error)}；当前草稿已保留。`; }
    finally { busy = ""; render(); }
  }
  async function refresh() {
    if (busy) return;
    await run("refresh", async () => {
      const current = ++request;
      const epoch = model.epoch;
      const state = await options.api.userRules();
      if (current !== request) return;
      if (epoch !== model.epoch && !model.dirty) return;
      if (editorDirty && model.base) {
        if (state.revision >= (model.latest?.revision ?? -1)) model.latest = state;
      } else model.sync(state);
    });
  }
  function clearEditor() {
    editingId = null;
    editorDirty = false;
    editingForm.reset();
    input("#user-rule-target").value = "DIRECT";
    find("#user-rule-editor-title").textContent = "添加规则";
    find("#user-rule-submit").textContent = "加入草稿";
    find("#user-rule-cancel").classList.add("is-hidden");
    find("#user-rule-no-resolve-field").classList.add("is-hidden");
  }
  function changed() { validation = null; feedback = "草稿已更新，校验并保存后生效。"; render(); }
  function restoreRowFocus(id: string, action: string) {
    const row = [...root.querySelectorAll<HTMLElement>("[data-rule-id]")].find((item) => item.dataset.ruleId === id);
    const control = row?.querySelector<HTMLElement>(`[data-rule-action="${action}"]:not(:disabled)`)
      ?? row?.querySelector<HTMLElement>("[data-rule-action='edit']");
    control?.focus();
  }
  async function parseText() {
    if (!model.textPending) return;
    const parsed = await options.api.parseUserRulesText(model.text);
    model.replace(parsed);
    clearEditor();
    validation = null;
  }
  function showTab(next: typeof tab) {
    tab = next;
    for (const name of ["visual", "advanced", "effective"] as const) {
      const button = find<HTMLButtonElement>(`[data-rule-tab="${name}"]`);
      const active = name === tab;
      button.setAttribute("aria-selected", String(active));
      button.tabIndex = active ? 0 : -1;
      const panel = find(`#${button.getAttribute("aria-controls")}`);
      panel.hidden = !active;
      panel.classList.toggle("is-hidden", !active);
    }
    find(".user-rules-actions").classList.toggle("is-hidden", next === "effective");
    find("#user-rules-validation").classList.toggle("is-hidden", !validation || next === "effective");
  }
  async function selectTab(next: typeof tab) {
    if (busy) return;
    if (editorDirty && next !== "visual") {
      feedback = "表单还有未加入草稿的更改，请先加入草稿或取消编辑。";
      updateControls();
      return;
    }
    if (next === "visual" && model.textPending) {
      await run("parse", async () => { await parseText(); showTab(next); });
    } else { showTab(next); if (next === "effective") await options.onApplied(); }
  }

  editingForm.addEventListener("submit", (event) => {
    event.preventDefault();
    if (busy || !model.base) return;
    const kind = input("#user-rule-kind").value;
    const value = input("#user-rule-value").value.trim();
    const target = input("#user-rule-target").value.trim();
    if (!value || !target || value.includes(",") || target.includes(",") || /[\r\n]/.test(value + target)) { feedback = "匹配内容和策略需填写为单项；复杂规则请使用高级文本。"; updateControls(); return; }
    const rule: UserRule = {
      id: editingId ?? crypto.randomUUID(), enabled: input("#user-rule-enabled").checked,
      rule: visualRuleText({ kind, value, target, noResolve: input("#user-rule-no-resolve").checked }),
      note: input("#user-rule-note").value.trim(),
    };
    const rules = cloneRules(model.rules);
    const index = rules.findIndex((item) => item.id === editingId);
    if (editingId && index < 0) { feedback = "编辑中的规则已变化，请重新选择。"; updateControls(); return; }
    if (index >= 0) rules[index] = rule; else rules.push(rule);
    model.replace(rules); clearEditor(); changed();
  });
  editingForm.addEventListener("input", () => {
    editorDirty = true;
    find("#user-rule-cancel").classList.remove("is-hidden");
    feedback = "表单更改尚未加入草稿。";
    updateControls();
  });
  find("#user-rule-kind").addEventListener("change", () => {
    const kind = input("#user-rule-kind").value;
    find("#user-rule-no-resolve-field").classList.toggle("is-hidden", kind !== "IP-CIDR" && kind !== "IP-CIDR6");
  });
  find("#user-rule-cancel").addEventListener("click", () => { clearEditor(); feedback = "已取消表单编辑。"; updateControls(); });
  find("#user-rules-list").addEventListener("click", (event) => {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-rule-action]");
    const row = target?.closest<HTMLElement>("[data-rule-id]");
    if (!target || !row || busy) return;
    const rule = model.rules.find((item) => item.id === row.dataset.ruleId);
    if (!rule) return;
    const action = target.dataset.ruleAction;
    if (action === "toggle") { model.replace(model.rules.map((item) => item.id === rule.id ? { ...item, enabled: (target as HTMLInputElement).checked } : item)); changed(); restoreRowFocus(rule.id, action); }
    if (action === "up" || action === "down") { model.move(rule.id, action === "up" ? -1 : 1); changed(); restoreRowFocus(rule.id, action); }
    if (action === "delete") void run("delete", async () => {
      if (!await options.confirm({ title: "删除草稿规则", message: `删除后仍需校验并保存才会生效。\n${rule.rule}`, confirmLabel: "从草稿删除", returnFocus: target })) return;
      model.replace(model.rules.filter((item) => item.id !== rule.id));
      if (editingId === rule.id) clearEditor();
      validation = null;
    });
    if (action === "edit") {
      if (editorDirty) { feedback = "请先将当前表单加入草稿，或取消编辑。"; updateControls(); return; }
      const parsed = parseVisualRule(rule.rule);
      if (!parsed) { showTab("advanced"); find<HTMLTextAreaElement>("#user-rules-text").focus(); return; }
      editingId = rule.id;
      input("#user-rule-kind").value = parsed.kind;
      input("#user-rule-value").value = parsed.value;
      input("#user-rule-target").value = parsed.target;
      input("#user-rule-note").value = rule.note;
      input("#user-rule-enabled").checked = rule.enabled;
      input("#user-rule-no-resolve").checked = parsed.noResolve;
      find("#user-rule-no-resolve-field").classList.toggle("is-hidden", !["IP-CIDR", "IP-CIDR6"].includes(parsed.kind));
      find("#user-rule-editor-title").textContent = "编辑规则";
      find("#user-rule-submit").textContent = "更新草稿";
      find("#user-rule-cancel").classList.remove("is-hidden");
      input("#user-rule-value").focus();
    }
  });
  root.querySelectorAll<HTMLButtonElement>("[data-rule-tab]").forEach((button) => {
    button.addEventListener("click", () => void selectTab(button.dataset.ruleTab as typeof tab));
    button.addEventListener("keydown", (event) => {
      const names = ["visual", "advanced", "effective"] as const;
      const index = names.indexOf(tab);
      let next: typeof tab;
      if (event.key === "ArrowRight") next = names[(index + 1) % names.length];
      else if (event.key === "ArrowLeft") next = names[(index + names.length - 1) % names.length];
      else if (event.key === "Home") next = "visual";
      else if (event.key === "End") next = "effective";
      else return;
      event.preventDefault();
      void selectTab(next).then(() => find<HTMLButtonElement>(`[data-rule-tab="${tab}"]`).focus());
    });
  });
  find("#user-rules-text").addEventListener("input", () => { model.editText(find<HTMLTextAreaElement>("#user-rules-text").value); validation = null; feedback = "文本草稿尚未解析或保存。"; find("#user-rules-validation").classList.add("is-hidden"); updateControls(); });
  find("#user-rules-refresh").addEventListener("click", () => void refresh());
  find("#user-rules-validate").addEventListener("click", () => void run("validate", async () => {
    if (editorDirty) throw new Error("请先将表单更改加入草稿，或取消编辑");
    await parseText();
    const epoch = model.epoch;
    const result = await options.api.validateUserRules(cloneRules(model.rules));
    if (model.markValidated(result, epoch)) validation = result;
    feedback = result.valid ? "校验通过，可保存并应用。" : "请修正校验列出的规则后重试。";
  }));
  find("#user-rules-save").addEventListener("click", () => {
    if (!model.canSave || editorDirty) return;
    void run("save", async () => {
      const epoch = model.epoch;
      try {
        const saved = await options.api.saveUserRules(cloneRules(model.rules), model.base!.revision);
        request++;
        if (epoch === model.epoch) { model.accept(saved); validation = null; clearEditor(); }
        feedback = "规则已保存并应用；现有连接可能继续使用原路径，新连接按最新规则匹配。";
        await options.onApplied();
      } catch (error) {
        try { model.sync(await options.api.userRules()); } catch { /* Keep the original save error and draft. */ }
        throw error;
      }
    });
  });
  find("#user-rules-undo").addEventListener("click", () => void run("undo", async () => {
    if ((!model.dirty && !editorDirty) || !await options.confirm({ title: "撤销未保存更改", message: "当前规则草稿、表单和文本更改将被最新保存版本替换。可先导出文本保留副本。", confirmLabel: "撤销草稿", returnFocus: find("#user-rules-undo") })) return;
    model.undo(); clearEditor(); validation = null; feedback = "已还原到最新读取的保存版本。";
  }));
  find("#user-rules-history").addEventListener("change", updateControls);
  find("#user-rules-rollback").addEventListener("click", () => void run("rollback", async () => {
    const revisionId = input("#user-rules-history").value;
    if (!revisionId || !model.base) return;
    if (!await options.confirm({ title: "回滚规则版本", message: `${model.dirty || editorDirty ? "当前未保存草稿及表单更改将被替换。" : ""}历史规则将重新校验并应用；配置中的其他设置保持不变。`, confirmLabel: "确认回滚", returnFocus: find("#user-rules-rollback") })) return;
    try {
      const restored = await options.api.rollbackUserRules(revisionId, model.base.revision);
      request++; model.accept(restored); clearEditor(); validation = null; feedback = "已回滚并应用规则。"; await options.onApplied();
    } catch (error) { try { model.sync(await options.api.userRules()); } catch { /* Preserve original error. */ } throw error; }
  }));
  find("#user-rules-import").addEventListener("change", (event) => {
    const fileInput = event.target as HTMLInputElement;
    const file = fileInput.files?.[0];
    if (!file) return;
    void run("import", async () => {
      if (file.size > 1024 * 1024) throw new Error("规则文件需小于 1 MB");
      const text = await file.text();
      const parsed = await options.api.parseUserRulesText(text);
      if ((model.dirty || editorDirty) && !await options.confirm({ title: "替换规则草稿", message: "导入内容将替换当前未保存草稿及表单更改，是否继续？", confirmLabel: "替换草稿", returnFocus: fileInput })) return;
      model.replace(parsed); validation = null; clearEditor(); feedback = `已导入 ${parsed.length} 条规则到草稿，尚未保存。`;
    }).finally(() => { fileInput.value = ""; });
  });
  find("#user-rules-export").addEventListener("click", () => {
    if (busy) return;
    find("#user-rules-export-panel").hidden = false;
    find("#user-rules-export-panel").classList.remove("is-hidden");
    find("#user-rules-export").setAttribute("aria-expanded", "true");
    feedback = "规则文本已准备，尚未写入文件。";
    updateControls();
    find<HTMLTextAreaElement>("#user-rules-export-text").focus();
  });
  const selectExportText = () => {
    const text = find<HTMLTextAreaElement>("#user-rules-export-text");
    text.focus();
    text.select();
  };
  const copyShortcut = /mac/i.test(navigator.platform) ? "⌘C" : "Ctrl+C";
  find("#user-rules-select-all").addEventListener("click", () => {
    if (busy) return;
    selectExportText();
    find("#user-rules-export-feedback").textContent = `已全选文本，按 ${copyShortcut} 复制；尚未写入文件。`;
  });
  find("#user-rules-copy").addEventListener("click", () => void run("copy", async () => {
    selectExportText();
    const text = find<HTMLTextAreaElement>("#user-rules-export-text").value;
    const copied = await copyRuleText(text, (value) => navigator.clipboard.writeText(value));
    if (!copied) selectExportText();
    const message = copied
      ? "规则文本已复制到剪贴板；尚未写入文件。"
      : `自动复制未完成。文本已全选，请按 ${copyShortcut} 复制；尚未写入文件。`;
    find("#user-rules-export-feedback").textContent = message;
    feedback = message;
  }));
  find("#user-rules-export-close").addEventListener("click", () => {
    if (busy) return;
    find("#user-rules-export-panel").hidden = true;
    find("#user-rules-export-panel").classList.add("is-hidden");
    find("#user-rules-export").setAttribute("aria-expanded", "false");
    find("#user-rules-export").focus();
  });
  render();
  return { refresh };
}
