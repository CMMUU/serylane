import { localizeText, setLocalizedText, setMessage } from "./i18n";
import type { NodeTestResult } from "./node-test";
import type { ProxyMap } from './node-selection';
export type ProxyMode = 'manual' | 'ai';
export type ManualOutboundState = { mode: ProxyMode; revision: string; activeProfileId: string | null; legacySelection: boolean };
const escape = (v: string) => v.replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'})[c]!);
export const manualOutboundMarkup = `<article class="panel manual-outbound-panel">
  <div class="panel-heading"><div><h2><!--i18n:ui-84f1348d1c5c12-->自选节点</h2><p><!--i18n:ui-fdcddc076df612-->按订阅策略组选择出口，保留原有分流规则。</p></div><button class="button button-quiet" id="manual-refresh"><!--i18n:ui-aee88743413144-->刷新</button></div>
  <div class="mode-banner"><p id="manual-current" role="status"><!--i18n:ui-780be93f7a1d28-->正在读取选点方式…</p><button class="button button-primary" id="manual-apply" disabled><!--i18n:ui-7abf9476a22ed1-->使用自选节点</button></div>
  <p class="hint"><!--i18n:ui-1bc9836596759c-->与 AI 代理互斥，二者仅一个生效；域名路由独立，不受切换影响。不会自动开启系统代理或 TUN。</p>
  <p id="manual-result" role="status"></p>
  <div class="manual-test-toolbar"><button type="button" class="button button-primary" id="manual-test-all" data-node-action-loading disabled><!--i18n:ui-9196835e388d25-->测试全部</button><button type="button" class="button button-quiet" id="manual-test-cancel" data-node-action-loading hidden><!--i18n:ui-d3a18faf1859f2-->停止测试</button><progress id="manual-test-progress" max="1" value="0" aria-label="节点测试进度" hidden data-i18n-aria-label="ui-59321687df0691"></progress></div>
  <p id="manual-test-status" class="hint" role="status" aria-live="polite"><!--i18n:ui-e75a82acb7776a-->正在读取测试状态…</p>
  <fieldset id="manual-controls"><legend class="sr-only"><!--i18n:ui-4679f6fb9d15ed-->订阅策略组选点</legend>
    <div class="manual-filters"><label><!--i18n:ui-283a7f514f66bd-->当前配置<select id="manual-profile" aria-label="选用订阅或本地配置" data-i18n-aria-label="ui-1eb01b1b9287b3"></select></label><label><!--i18n:ui-42d21914f68b23-->筛选节点<input id="manual-search" type="search" placeholder="搜索节点名称或协议"  data-i18n-placeholder="ui-5778a01730f249"/></label><label><!--i18n:ui-a96c9a85419087-->排序<select id="manual-sort"><option value="source"><!--i18n:ui-0df05b62b68371-->订阅顺序</option><option value="delay"><!--i18n:ui-338e09b164e295-->延迟优先</option><option value="name"><!--i18n:ui-d44e9b3d3b31d3-->名称</option></select></label></div>
    <div class="routing-segments" id="manual-routing-mode" role="group" aria-label="自选节点路由模式" data-i18n-aria-label="ui-f273ed4aee3ec9"><button type="button" data-routing-mode="rule"><!--i18n:ui-1123245d1bed08-->规则</button><button type="button" data-routing-mode="global"><!--i18n:ui-63d6b47116de36-->全局</button><button type="button" data-routing-mode="direct"><!--i18n:ui-b06325c5660f0c-->直连</button></div>
    <p class="hint"><!--i18n:ui-ed8e965008249b-->规则：按分流规则使用各组；全局：使用 GLOBAL；直连：不使用代理节点。自动组手动优先，失效时仍可由核心回退。</p>
    <div id="proxy-groups" class="card-list empty-state"><!--i18n:ui-dcb9ed46383fb3-->添加并选用配置、启动核心后查看节点。</div>
  </fieldset>
</article>`;

export function nodeGridMarkup(
  group: string, map: ProxyMap, query: string, sort: string, busy: boolean, results: NodeTestResult[] = [], testing = false, limit = 200,
  pendingSingles: ReadonlyMap<string, string> = new Map(), selection: { group: string; node: string | null; phase?: "write" | "verify" } | null = null,
): string {
  const value = map[group] ?? {};
  const selectable = ['Selector', 'Fallback', 'URLTest'].includes(value.type ?? '');
  const measured = new Map(results.map(n => [n.name, n]));
  const delay = (name: string) => { const result = measured.get(name); if (result) return result.state === 'passed' ? result.delayMs : null; const history = map[name]?.history; const sample = history?.[history.length - 1]?.delay; return typeof sample === 'number' && sample > 0 ? sample : null; };
  const nodes = (value.all ?? []).filter(name => `${name} ${map[name]?.type ?? ''}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  if (sort === 'name') nodes.sort((a,b) => a.localeCompare(b));
  if (sort === 'delay') nodes.sort((a,b) => (delay(a) ?? Infinity) - (delay(b) ?? Infinity));
  const label = (name: string) => { const n = measured.get(name); return n ? ({pending: '未测试', testing: '测试中…', passed: `${n.delayMs} ms`, timeout: '超时', failed: '测试失败', unavailable: '暂未测得'})[n.state] : delay(name) === null ? '测速' : `${delay(name)} ms`; };
  const escapedGroup = escape(group);
  const rows = nodes.slice(0, limit).map(name => {
    const escapedName = escape(name);
    const speedPending = pendingSingles.get(name) === group;
    const selectionPending = selection?.group === group && selection.node === name;
    return `<div data-render-key="${escapedName}" class="manual-node${value.now === name ? ' is-selected' : ''}">
      <button type="button" data-node-choice="${escapedName}" data-node-group="${escapedGroup}" data-node-action-loading ${selectionPending ? 'aria-busy="true" ' : ''}aria-pressed="${value.now === name}" ${busy || selection || !selectable ? 'disabled' : ''}><strong>${escapedName}</strong><small>${escape(map[name]?.type ?? '节点')} · ${selectionPending ? selection.phase === 'verify' ? '正在核实出口…' : '正在应用…' : value.now === name ? '当前选中' : '候选'}</small></button>
      <button type="button" class="proxy-delay node-latency" data-proxy="${escapedName}" data-proxy-group="${escapedGroup}" data-node-action-loading data-i18n-live aria-label="${speedPending ? '正在测试' : '测试'} ${escapedName} 延迟" data-i18n-aria-label="ui-8491065fd965ec" ${speedPending ? 'aria-busy="true" ' : ''}${busy || testing || pendingSingles.has(name) ? 'disabled' : ''}>${speedPending ? '测速中…' : escape(label(name))}</button>
    </div>`;
  });
  return `<div class="manual-node-list" role="group" aria-label="${escapedGroup} 节点" data-i18n-aria-label="ui-afb1ab4f2bb856">${rows.join('') || "<p class=\"empty-state\"><!--i18n:ui-46a5facb65008b-->没有匹配节点。</p>"}</div>${nodes.length > limit ? `<button type="button" class="button button-quiet" data-node-more="${escapedGroup}"><!--i18n:ui-aae033dc8b8edc-->显示更多（${limit}/${nodes.length}）</button>` : ''}`;
}

export function mountManualOutbound(root: HTMLElement, options: {
  read(): Promise<ManualOutboundState>;
  refreshAll?: () => Promise<void>;
  write(mode: ProxyMode, expectedRevision: string): Promise<ManualOutboundState>;
  confirm(input: {title: string; message: string; confirmLabel: string}): Promise<boolean>;
  changed(state: ManualOutboundState): Promise<void>;
  error(error: unknown): string;
}) {
  let state: ManualOutboundState | null = null, busy = false, reading = false, readVersion = 0;
  const el = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`)!;
  const message = (text: string) => { setLocalizedText(el('manual-result'), text); };
  function accept(next: ManualOutboundState) {
    state = next;
    const current = el('manual-current');
    const mode = next.mode === 'manual' ? '自选节点' : 'AI 代理';
    const terms = () => ({ p1: localizeText(mode) });
    if (next.legacySelection) {
      setLocalizedText(current, '旧版固定出口待迁移：请确认使用订阅策略组选点。');
    } else if (next.activeProfileId) {
      setMessage(current, 'ui-9fce1c210321c4', `当前选点方式：${mode}`, terms);
    } else {
      setMessage(current, 'ui-77391f87190659', `未启用 · 请先添加并选用配置（已选${mode}）`, terms);
    }
    el<HTMLButtonElement>('manual-apply').disabled = busy || next.mode === 'manual' && !next.legacySelection;
    el<HTMLFieldSetElement>('manual-controls').disabled = busy || next.mode !== 'manual' || next.legacySelection;
  }
  async function refresh(all = false) {
    if (busy || reading) return;
    reading = true;
    const refreshButton = el<HTMLButtonElement>('manual-refresh');
    refreshButton.disabled = true; setLocalizedText(refreshButton, '正在刷新…');
    const version = ++readVersion;
    try { const next = await options.read(); if (version === readVersion && !busy) { accept(next); message(''); } if (all) await options.refreshAll?.(); }
    catch (error) { if (version === readVersion) message(options.error(error)); }
    finally { reading = false; refreshButton.disabled = busy; setLocalizedText(refreshButton, '刷新'); }
  }
  async function apply(mode: ProxyMode) {
    if (busy || !state) return;
    busy = true; ++readVersion;
    el<HTMLButtonElement>('manual-refresh').disabled = true;
    accept(state);
    try {
      const name = mode === 'manual' ? '自选节点' : 'AI 代理';
      if (!await options.confirm({title: `切换为${name}？`, message: mode === 'manual' ? '暂停 AI 自动筛选与灾备，恢复订阅原有策略组和分流规则。既有 AI 偏好会保留，域名路由设置不变。核心运行中会重新加载配置。' : '暂停自选节点操作，启用当前配置中已保存的 AI 策略；尚未生成策略时需在 AI 代理页生成。域名路由设置不变。核心运行中会重新加载配置。', confirmLabel: `使用${name}`})) return;
      const next = await options.write(mode, state.revision);
      accept(next); message(`已切换为${name}；核心未运行时，下次启动生效。`);
      try { await options.changed(next); } catch { message('选点方式已保存，请刷新页面状态。'); }
    } catch (error) { message(options.error(error)); }
    finally { busy = false; el<HTMLButtonElement>('manual-refresh').disabled = false; if (state) accept(state); }
  }
  el('manual-refresh').addEventListener('click', () => void refresh(true));
  el('manual-apply').addEventListener('click', () => void apply('manual'));
  return { refresh, accept: (next: ManualOutboundState) => { if (!busy) accept(next); }, enableAi: () => apply('ai') };
}
