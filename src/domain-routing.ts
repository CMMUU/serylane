import { escapeNode as escape } from './node-selection';
export type DomainSelection = { tools: string[]; customDomains: string[]; target: string };
export type DomainPolicy = DomainSelection & { domains: string[] };
export type AiPreset = {id:string; name:string; domains:string[]; note:string; sources:string[]};
export type DomainRouteState = { document: { schemaVersion: number; revision: number; enabled: boolean; draft: DomainPolicy; applied: DomainPolicy | null }; presets: AiPreset[]; targets: string[]; profileId: string | null; profileRevision: string | null; status: string; readiness?: string; verification?: string; warning: string; userRuleCount: number };
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
export const domainRoutingMarkup = `<section class="panel domain-route-panel">
  <div class="panel-heading domain-heading"><div><h2>AI 工具域名路由</h2><p class="hint">名单内流量使用指定出口，其他流量保持原规则。</p></div><div class="domain-actions"><button id="domain-toggle" class="button button-primary" type="button" aria-pressed="false">开启路由</button><button id="domain-refresh" class="button button-quiet" type="button">刷新状态</button></div></div>
  <div class="domain-state"><p id="domain-status" role="status">正在读取设置…</p><span id="domain-summary" class="hint"></span></div>
  <form id="domain-form"><fieldset id="domain-fields">
    <section class="domain-tools-section"><h3>常见 AI 工具</h3><p class="hint">勾选后自动补齐域名；取消勾选保留自定义地址。</p><div id="domain-tools" class="domain-tools"></div></section>
    <div class="domain-editor-grid"><section><label for="domain-domains">自定义地址</label><textarea id="domain-domains" rows="4" spellcheck="false" placeholder="openai.com&#10;*.openai.com" aria-describedby="domain-input-help"></textarea><p id="domain-input-help" class="hint">每行一个域名，支持 *.example.com；根域须单独添加。</p></section>
    <section><label for="domain-target">路由出口策略组</label><select id="domain-target"><option value="">请明确选择出口</option></select><p class="hint">仅处理进入代理核心的流量，在规则模式下生效。不会切换系统代理或 TUN。</p></section></div>
    <div class="domain-editor-actions"><div class="domain-actions"><button id="domain-save" class="button button-primary" type="submit">保存草稿</button><button id="domain-apply" class="button button-quiet" type="button">应用更改</button></div><div class="domain-actions"><button id="domain-reset" class="button button-quiet" type="button">撤销编辑</button><button id="domain-clear" class="button button-quiet" type="button">清空名单</button></div></div>
  </fieldset></form>
  <p id="domain-revision" class="hint"></p><p id="domain-feedback" role="status" class="domain-feedback" hidden></p>
  <details class="domain-help"><summary>使用说明与匹配规则</summary><p id="domain-warning" class="hint"></p><p class="hint">支持 https://*.openai.com/* 输入，不按 HTTPS 路径筛选、不解密内容。工具预置和自定义地址统一去重；共享第三方服务不默认纳入。切换订阅后出口缺失时，匹配流量会被拒绝，请重选出口或关闭路由。</p></details>
  <section id="domain-legacy" class="domain-help" hidden><h3>旧接入恢复</h3><p class="hint" id="domain-legacy-note"></p><button id="domain-restore" class="button button-quiet" type="button">恢复旧接入并关闭中转</button></section>
</section>`;
export function mountDomainRouting(root: HTMLElement, services: {
  api: { domainRouteStatus(): Promise<DomainRouteState>; saveDomainRoute(policy: DomainSelection, expectedRevision: number): Promise<DomainRouteState>; applyDomainRoute(enabled: boolean, state: DomainRouteState): Promise<DomainRouteState>;
    legacyRouteStatus(): Promise<import('./types').RouteSnapshot | null>; setCodexRoute(attach: boolean, revision: string, confirmed: boolean): Promise<import('./types').RouteSnapshot>; setLocalRouteEnabled(enabled: boolean, revision: number, confirmed: boolean): Promise<import('./types').RouteSnapshot> };
  confirm: (options: { title: string; message: string; confirmLabel?: string; returnFocus?: HTMLElement }) => Promise<boolean>;
  error: (error: unknown) => string;
  globalToggle?: HTMLButtonElement;
  isBlocked?: () => boolean;
  contextKey?: () => string;
  busyChanged?: (busy: boolean) => void;
  reportError?: (message: string) => void;
  openSetup?: () => void;
}) {
  const $ = <T extends HTMLElement>(name: string) => root.querySelector<T>(`#domain-${name}`)!;
  type Operation = 'save' | 'enable' | 'disable' | 'apply' | 'restore';
  let state: DomainRouteState | null = null, busy: Operation | null = null, dirty = false, draftRevision = 0;
  let epoch = 0, readAgain = false, readPromise: Promise<void> | null = null, readError = '', filled = '', baseline = '';
  let legacy: import('./types').RouteSnapshot | null = null;
  let focusAfterWork: string | null = null;
  const context = () => services.contextKey?.() ?? '';
  const blocked = () => Boolean(services.isBlocked?.());
  const message = (value: string, error = false) => { $('feedback').textContent = value; $('feedback').hidden = !value; $('feedback').dataset.tone = error ? 'error' : 'info'; };
  const errorText = (error: unknown) => {
    // Native route errors carry sanitized actionable details; do not hide them behind the generic DTO title.
    const details = error && typeof error === 'object' ? (error as {userMessage?: {details?: string}}).userMessage?.details : undefined;
    return details || services.error(error);
  };
  const selection = (): DomainSelection => ({ tools: Array.from(root.querySelectorAll<HTMLInputElement>('[data-tool]:checked')).map(c => c.dataset.tool!).sort(), customDomains: parseDomains($<HTMLTextAreaElement>('domains').value), target: $<HTMLSelectElement>('target').value.trim() });
  const signatureOf = (p: DomainSelection) => JSON.stringify({tools: [...p.tools].sort(), customDomains: p.customDomains, target: p.target.trim()});
  function edited() {
    try { dirty = signatureOf(selection()) !== baseline; } catch { dirty = true; }
    render();
  }
  function preparation(): {text: string; field: string} | null {
    if (!state) return null;
    if (!state.profileId) return {text: '请先在配置页选用订阅，再设置路由出口。', field: 'target'};
    if (dirty) return {text: '编辑尚未保存。请保存草稿，再确认开启路由。', field: 'save'};
    if (!state.document.draft.domains.length) return {text: '请勾选 AI 工具或添加自定义域名，再保存草稿。', field: 'domains'};
    if (!state.targets.includes(state.document.draft.target)) return {text: '请选择当前配置中的有效出口，并保存草稿。', field: 'target'};
    if (state.readiness === 'wrong_mode') return {text: '当前不是规则模式，请先在配置页切换为规则模式。', field: 'target'};
    return null;
  }
  function guide() {
    const next = preparation();
    if (!next) return false;
    services.openSetup?.();
    if (busy) focusAfterWork = next.field; else $(next.field).focus?.();
    message(next.text);
    return true;
  }
  function fill() {
    if (!state) return;
    const signature = JSON.stringify([state.document.revision, state.document.draft, state.presets, state.targets]);
    if (signature === filled) return; // Quiet refresh must not collapse details or steal keyboard focus.
    filled = signature;
    $('tools').innerHTML = state.presets.map(p => `<div class="domain-preset"><label><input type="checkbox" data-tool="${escape(p.id)}" ${state!.document.draft.tools.includes(p.id) ? 'checked' : ''} /><span>${escape(p.name)}</span></label><details><summary>${p.domains.length} 条域名 · 查看范围</summary><p class="hint">${escape(p.note)}</p><code>${p.domains.map(escape).join('<br>')}</code></details></div>`).join('');
    $<HTMLTextAreaElement>('domains').value = state.document.draft.customDomains.join('\n');
    $<HTMLSelectElement>('target').innerHTML = '<option value="">请明确选择出口</option>' + [...new Set([...state.targets, state.document.draft.target].filter(Boolean))].map(name => `<option value="${escape(name)}">${escape(name)}${state!.targets.includes(name) ? '' : ' · 当前配置缺失'}</option>`).join('');
    $<HTMLSelectElement>('target').value = state.document.draft.target; draftRevision = state.document.revision;
    baseline = signatureOf(state.document.draft);
  }
  function render() {
    const enabled = Boolean(state?.document.enabled), locked = Boolean(busy) || blocked();
    const unapplied = Boolean(state && JSON.stringify(state.document.draft) !== JSON.stringify(state.document.applied));
    $<HTMLFieldSetElement>('fields').disabled = locked || !state;
    const applyButton = $<HTMLButtonElement>('apply');
    applyButton.hidden = !enabled;
    applyButton.disabled = locked || !state || dirty || (!unapplied && !needsReapply());
    applyButton.textContent = busy === 'apply' ? '正在应用并核对…' : needsReapply() && !unapplied ? '重新应用' : '应用更改';
    $<HTMLButtonElement>('save').textContent = busy === 'save' ? '正在保存…' : '保存草稿';
    $<HTMLButtonElement>('refresh').disabled = locked || Boolean(readPromise);
    const pending = busy === 'enable' ? '正在开启…' : busy === 'disable' ? '正在关闭…' : busy === 'apply' ? '正在应用…' : '';
    const setup = !enabled ? preparation() : null;
    const detail = readError ? '状态待核对，请刷新重试' : setup?.text ?? state?.status ?? '正在读取路由状态…';
    for (const button of [$<HTMLButtonElement>('toggle'), services.globalToggle].filter((b): b is HTMLButtonElement => Boolean(b))) {
      const global = button === services.globalToggle;
      button.textContent = pending || (global ? '路由' : enabled ? '关闭路由' : setup ? '设置路由' : '开启路由');
      button.disabled = locked || !state;
      button.setAttribute('aria-pressed', String(enabled));
      button.setAttribute('aria-busy', String(Boolean(pending)));
      button.setAttribute('aria-label', `${enabled ? '关闭' : '开启'}域名路由`);
      button.classList.toggle('is-active', enabled && !readError && (!state?.verification || state.verification === 'verified'));
      button.dataset.verification = readError ? 'unknown' : state?.verification ?? 'unknown';
      button.title = setup ? `${setup.text} 点击前往设置。` : `${detail}；${enabled ? '点击关闭' : '点击开启'}。独立于系统代理和 TUN，不切换网络模式。`;
    }
    $('status').textContent = detail;
    $('status').dataset.tone = readError || ['mismatch', 'unavailable', 'changed'].includes(state?.verification ?? '') ? 'error' : setup ? 'info' : 'normal';
    $('summary').textContent = state?.document.applied && enabled ? `${state.document.applied.domains.length} 条域名 → ${state.document.applied.target}` : '配置后开启，仅影响名单内流量';
    $('warning').textContent = state?.warning ?? '';
    $('revision').textContent = state ? `${dirty ? '编辑尚未保存' : unapplied ? '草稿尚未应用' : '草稿与已应用设置一致'} · 保存不改变流量，应用需确认。` : '';
    $('legacy').hidden = !legacy?.codex.hasBackup && !legacy?.enabled;
    $('legacy-note').textContent = [legacy?.codex.warning, legacy?.lastError, '只恢复 Serylane 仍拥有的字段，保留其他程序的修改和原备份；不会新增 Codex 接入。'].filter(Boolean).join(' ');
    $<HTMLButtonElement>('restore').disabled = locked || Boolean(legacy?.active);
  }
  function accept(result: DomainRouteState) {
    state = result; readError = '';
    if (!dirty) fill();
  }
  async function refresh(quiet = false): Promise<void> {
    if (busy || blocked()) { readAgain = true; return; }
    if (readPromise) { readAgain = true; return readPromise; }
    readPromise = (async () => {
      // One trailing read coalesces refreshes during a profile/runtime transition.
      for (let attempt = 0; attempt < 2; attempt++) {
        readAgain = false;
        const stamp = epoch, key = context();
        try {
          const result = await services.api.domainRouteStatus();
          if (stamp !== epoch || key !== context() || busy || blocked()) { readAgain = true; continue; }
          if (!state || result.document.revision >= state.document.revision) accept(result);
          if (!quiet) {
            try { legacy = await services.api.legacyRouteStatus(); }
            catch (error) { if (stamp === epoch && key === context()) message(`路由状态已读取；旧接入信息读取失败：${errorText(error)}`, true); return; }
            if (stamp !== epoch || key !== context() || busy || blocked()) continue;
            message(dirty ? '编辑已保留；如设置已变化，请撤销编辑后重新读取。' : '状态已刷新，未修改网络设置。');
          }
        } catch (error) {
          if (stamp === epoch && key === context() && !busy && !blocked()) {
            readError = errorText(error);
            if (!quiet) message(readError, true);
          }
        }
        if (!readAgain || busy || blocked()) break;
      }
    })();
    render();
    try { await readPromise; } finally { readPromise = null; render(); }
  }
  async function work(operation: Operation, callback: () => Promise<void>) {
    if (busy || blocked()) return;
    busy = operation; epoch++; services.busyChanged?.(true); render();
    try { await callback(); }
    catch (error) { const text = errorText(error); message(text, true); services.reportError?.(text); }
    finally {
      busy = null; epoch++; services.busyChanged?.(false); render();
      if (focusAfterWork) { $(focusAfterWork).focus?.(); focusAfterWork = null; }
      // An older read cannot overwrite a successful write (or a failure rollback).
      void (readPromise ?? Promise.resolve()).then(() => refresh(true));
    }
  }
  const needsReapply = () => Boolean(state?.verification && ['mismatch', 'unavailable', 'changed'].includes(state.verification));
  async function apply(enabled: boolean, origin: HTMLElement, reapply = false) {
    if (!state || busy || blocked()) return;
    if (enabled && guide()) return;
    if (reapply && !needsReapply() && JSON.stringify(state.document.draft) === JSON.stringify(state.document.applied)) return;
    await work(enabled ? reapply ? 'apply' : 'enable' : 'disable', async () => {
      const key = context();
      const expected = await services.api.domainRouteStatus();
      if (key !== context()) throw new Error('当前配置已变化，请刷新路由状态后重试。');
      // A hidden/global entry uses a fresh snapshot, never a stale page-local toggle.
      if (!reapply && expected.document.enabled === enabled) { accept(expected); message('路由状态已更新。'); return; }
      accept(expected); render();
      if (enabled && guide()) return;
      const title = enabled ? reapply ? '应用路由更改？' : '开启域名路由？' : '关闭域名路由？';
      if (!await services.confirm({ title, message: enabled ? `工具 ${expected.document.draft.tools.length} 个 + 自定义 ${expected.document.draft.customDomains.length} 条，去重后 ${expected.document.draft.domains.length} 条 → ${expected.document.draft.target}。名单内流量优先使用该出口，其他流量保持原规则。仅处理进入核心的新流量，不修改 Codex，不切换系统代理或 TUN，不主动清空连接。` : '移除域名路由覆盖，其他本地及订阅规则保持不变。系统代理与 TUN 状态不变。', confirmLabel: enabled ? reapply ? '确认应用' : '开启路由' : '关闭路由', returnFocus: origin })) return;
      if (key !== context()) throw new Error('当前配置已变化，请刷新路由状态后重试。');
      accept(await services.api.applyDomainRoute(enabled, expected));
      if (dirty && draftRevision === expected.document.revision) draftRevision = state!.document.revision;
      message(state!.status);
    });
    const doc = root.ownerDocument;
    if (origin.isConnected && !(origin as HTMLButtonElement).disabled && doc &&
        (doc.activeElement === doc.body || doc.activeElement?.closest('#confirmation-modal'))) origin.focus();
  }
  $('form').addEventListener('input', edited);
  $('form').addEventListener('change', edited);
  $('refresh').addEventListener('click', () => void refresh());
  $('reset').addEventListener('click', () => { if (busy || blocked()) return; dirty = false; filled = ''; fill(); render(); message('已恢复已保存草稿。'); });
  $('clear').addEventListener('click', () => { if (busy || blocked()) return; $<HTMLTextAreaElement>('domains').value = ''; root.querySelectorAll<HTMLInputElement>('[data-tool]').forEach(c => c.checked = false); edited(); message('名单已清空，尚未保存。立即停止请点击关闭路由。'); });
  $('form').addEventListener('submit', event => {
    event.preventDefault(); if (busy || blocked() || !state) return;
    let policy: DomainSelection;
    try { policy = selection(); }
    catch(error) { message(errorText(error), true); return; }
    void work('save', async () => { const result = await services.api.saveDomainRoute(policy, draftRevision); dirty = false; filled = ''; accept(result); message(preparation()?.text ?? (state!.document.enabled ? '草稿已保存，当前流量未改变。点击应用更改并确认后生效。' : '草稿已保存，当前流量未改变。点击开启路由并确认后生效。')); });
  });
  const toggle = (origin: HTMLElement) => { if (state) void apply(!state.document.enabled, origin); };
  $('toggle').addEventListener('click', () => toggle($('toggle')));
  services.globalToggle?.addEventListener('click', () => toggle(services.globalToggle!));
  $('apply').addEventListener('click', () => { if (state?.document.enabled) void apply(true, $('apply'), true); });
  $('restore').addEventListener('click', () => void work('restore', async () => {
    if (!legacy || !await services.confirm({ title:'恢复旧接入？', message:'仅恢复此前 Serylane 管理的接入字段；其他程序修改时保留备份。恢复后关闭旧模型中转。', returnFocus: $('restore') })) return;
    if (legacy.codex.hasBackup) legacy = await services.api.setCodexRoute(false, legacy.codex.configRevision, true);
    if (legacy.enabled) legacy = await services.api.setLocalRouteEnabled(false, legacy.revision, true);
    message('旧接入已恢复；新域名路由未自动启用。');
  }));
  render(); return { refresh, render };
}
