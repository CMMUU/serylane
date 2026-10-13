import { escapeNode as escape } from './node-selection';
export type DomainSelection = { tools: string[]; customDomains: string[]; target: string };
export type DomainPolicy = DomainSelection & { domains: string[] };
export type AiPreset = {id:string; name:string; domains:string[]; note:string; sources:string[]};
export type DomainRouteState = { document: { schemaVersion: number; revision: number; enabled: boolean; draft: DomainPolicy; applied: DomainPolicy | null }; presets: AiPreset[]; targets: string[]; profileId: string | null; profileRevision: string | null; status: string; warning: string; userRuleCount: number };
export function parseDomains(text: string): string[] {
  const rows = text.split(/\r?\n/).map(s => s.trim()).filter(Boolean);
  if (rows.length > 100) throw new Error('域名名单最多 100 条。');
  const seen = new Set<string>();
  return rows.map(row => {
    const normalized = row.toLowerCase().replace(/^https:\/\//, '').replace(/\/(\*)?$/, '');
    const host = normalized.replace(/^\*\./, '');
    if (host.length > 253 || !host.includes('.') || /^\d+(\.\d+){3}$/.test(host) || host.split('.').some(part => !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(part))) throw new Error('请填域名或 *.子域通配符，不含端口、参数或具体路径；域名规则覆盖所有路径。');
    if (seen.has(normalized)) throw new Error('域名重复，请合并后保存。');
    seen.add(normalized); return normalized;
  });
}
export const domainRoutingMarkup = `<section class="panel local-route-panel">
  <div class="panel-heading"><div><h2>AI 工具域名路由</h2><p class="hint">名单内流量使用指定出口，其他流量保持原规则。无需接入或修改 Codex。</p></div><button id="domain-refresh" class="button button-quiet">刷新状态</button></div>
  <p id="domain-status" role="status">正在读取设置…</p><p id="domain-warning" class="hint"></p>
  <form id="domain-form"><fieldset id="domain-fields"><section><h3>常见 AI 工具</h3><p class="hint">勾选工具后，后台自动补齐相关域名。展开可查看处理范围；取消勾选不会删除自定义地址。</p><div id="domain-tools"></div></section><label>自定义地址<textarea id="domain-domains" rows="4" spellcheck="false" placeholder="openai.com&#10;*.openai.com"></textarea></label>
  <p class="hint">每行一个域名。<code>*.openai.com</code> 匹配子域，根域 <code>openai.com</code> 须单独列出。支持 <code>https://*.openai.com/*</code> 输入；不支持按 HTTPS 路径筛选，不解密内容。</p>
  <label>路由出口策略组<select id="domain-target"><option value="">请明确选择出口</option></select></label>
  <p class="hint">共享第三方服务不默认纳入。仅处理已进入代理核心的流量，规则模式下生效；切换订阅后目标缺失时，匹配流量会被拒绝，不会静默直连。</p>
  <div class="toolbar"><button class="button button-primary" type="submit">保存草稿</button><button id="domain-reset" class="button button-quiet" type="button">撤销编辑</button><button id="domain-clear" class="button button-quiet" type="button">清空名单</button></div></fieldset></form>
  <p id="domain-revision" class="hint"></p><div class="toolbar"><button id="domain-apply" class="button button-primary">应用并启用</button><button id="domain-disable" class="button button-quiet">关闭域名路由</button></div>
  <p id="domain-feedback" role="status" class="hint"></p>
  <section id="domain-legacy" hidden><h3>旧接入恢复</h3><p class="hint" id="domain-legacy-note"></p><button id="domain-restore" class="button button-quiet">恢复旧接入并关闭中转</button></section>
</section>`;
export function mountDomainRouting(root: HTMLElement, services: {
  api: { domainRouteStatus(): Promise<DomainRouteState>; saveDomainRoute(policy: DomainSelection, expectedRevision: number): Promise<DomainRouteState>; applyDomainRoute(enabled: boolean, state: DomainRouteState): Promise<DomainRouteState>;
    legacyRouteStatus(): Promise<import('./types').RouteSnapshot | null>; setCodexRoute(attach: boolean, revision: string, confirmed: boolean): Promise<import('./types').RouteSnapshot>; setLocalRouteEnabled(enabled: boolean, revision: number, confirmed: boolean): Promise<import('./types').RouteSnapshot> };
  confirm: (options: { title: string; message: string; confirmLabel?: string; returnFocus?: HTMLElement }) => Promise<boolean>;
  error: (error: unknown) => string;
}) {
  const $ = <T extends HTMLElement>(name: string) => root.querySelector<T>(`#domain-${name}`)!;
  let state: DomainRouteState | null = null, busy = false, dirty = false, edit = 0, draftRevision = 0;
  let legacy: import('./types').RouteSnapshot | null = null;
  const message = (value: string) => { $('feedback').textContent = value; };
  function fill() {
    if (!state) return;
    $('tools').innerHTML = state.presets.map(p => `<div class="domain-preset"><label><input type="checkbox" data-tool="${escape(p.id)}" ${state!.document.draft.tools.includes(p.id) ? 'checked' : ''} /><span>${escape(p.name)}</span></label><details><summary>${p.domains.length} 条域名 · 查看范围</summary><p class="hint">${escape(p.note)}</p><code>${p.domains.map(escape).join('<br>')}</code></details></div>`).join('');
    $<HTMLTextAreaElement>('domains').value = state.document.draft.customDomains.join('\n');
    $<HTMLSelectElement>('target').innerHTML = '<option value="">请明确选择出口</option>' + [...new Set([...state.targets, state.document.draft.target].filter(Boolean))].map(name => `<option value="${escape(name)}">${escape(name)}${state!.targets.includes(name) ? '' : ' · 当前配置缺失'}</option>`).join('');
    $<HTMLSelectElement>('target').value = state.document.draft.target; draftRevision = state.document.revision;
  }
  function render() {
    $<HTMLFieldSetElement>('fields').disabled = busy || !state;
    $<HTMLButtonElement>('apply').disabled = busy || !state || dirty;
    $<HTMLButtonElement>('disable').disabled = busy || !state?.document.enabled;
    $<HTMLButtonElement>('refresh').disabled = busy;
    $('status').textContent = state?.status ?? '等待读取设置';
    $('warning').textContent = state?.warning ?? '';
    $('revision').textContent = state ? `修订 ${state.document.revision} · ${dirty ? '编辑尚未保存' : JSON.stringify(state.document.draft) !== JSON.stringify(state.document.applied) ? '草稿尚未应用' : '草稿与已应用设置一致'} · 保存不改变当前流量；应用需要单独确认。` : '';
    $('legacy').hidden = !legacy?.codex.hasBackup && !legacy?.enabled;
    $('legacy-note').textContent = [legacy?.codex.warning, legacy?.lastError, '只恢复 Serylane 仍拥有的字段，保留其他程序的修改和原备份；不会新增 Codex 接入。'].filter(Boolean).join(' ');
    $<HTMLButtonElement>('restore').disabled = busy || Boolean(legacy?.active);
  }
  async function work(callback: () => Promise<void>) {
    if (busy) return; busy = true; render();
    try { await callback(); } catch (error) { message(services.error(error)); }
    finally { busy = false; render(); }
  }
  async function refresh() { await work(async () => {
    const sequence = edit;
    const result = await services.api.domainRouteStatus();
    state = result;
    if (!dirty && sequence === edit) fill();
    legacy = await services.api.legacyRouteStatus();
    message(dirty ? '编辑已保留；如修订有变化，请撤销编辑后重新读取。' : '状态已刷新；未修改代理或接入配置。');
  }); }
  $('form').addEventListener('input', () => { dirty = true; edit++; render(); });
  $('refresh').addEventListener('click', () => void refresh());
  $('reset').addEventListener('click', () => { dirty = false; edit++; fill(); render(); message('已恢复已保存草稿。'); });
  $('clear').addEventListener('click', () => { $<HTMLTextAreaElement>('domains').value = ''; root.querySelectorAll<HTMLInputElement>('[data-tool]').forEach(c => c.checked = false); dirty = true; edit++; render(); message('名单已清空，尚未保存；如需立即停止，请关闭域名路由。'); });
  $('form').addEventListener('submit', event => {
    event.preventDefault(); if (busy || !state) return;
    let policy: DomainSelection;
    try { policy = { tools: Array.from(root.querySelectorAll<HTMLInputElement>('[data-tool]:checked')).map(c => c.dataset.tool!), customDomains: parseDomains($<HTMLTextAreaElement>('domains').value), target: $<HTMLSelectElement>('target').value }; }
    catch(error) { message(services.error(error)); return; }
    void work(async () => { state = await services.api.saveDomainRoute(policy, draftRevision); dirty = false; fill(); message('草稿已保存，现有流量规则未改变。'); });
  });
  async function apply(enabled: boolean) { await work(async () => {
    if (!state || enabled && dirty) return;
    const expected = state;
    if (!await services.confirm({title: enabled ? '应用域名路由？' : '关闭域名路由？', message: enabled ? `工具 ${expected.document.draft.tools.length} 个 + 自定义 ${expected.document.draft.customDomains.length} 条，合并去重后 ${expected.document.draft.domains.length} 条 → ${expected.document.draft.target || '尚未选择出口'}。域名路由优先于现有 ${expected.userRuleCount} 条本地规则及订阅规则，重叠域名的出口会改变。仅处理进入核心的新流量，不修改 Codex，不主动清空连接。` : '移除 Serylane 域名路由覆盖，其他本地及订阅规则保持不变。', confirmLabel:'确认应用'})) return;
    state = await services.api.applyDomainRoute(enabled, expected); if (!dirty) fill(); else draftRevision = state.document.revision; message(state.status);
  }); }
  $('apply').addEventListener('click', () => void apply(true));
  $('disable').addEventListener('click', () => void apply(false));
  $('restore').addEventListener('click', () => void work(async () => {
    if (!legacy || !await services.confirm({title:'恢复旧接入？', message:'仅恢复此前 Serylane 管理的接入字段；发现其他程序修改时停止恢复并保留备份。恢复后关闭旧模型中转。'})) return;
    if (legacy.codex.hasBackup) legacy = await services.api.setCodexRoute(false, legacy.codex.configRevision, true);
    if (legacy.enabled) legacy = await services.api.setLocalRouteEnabled(false, legacy.revision, true);
    message('旧接入已恢复；新域名路由未自动启用。');
  }));
  render(); return { refresh };
}
