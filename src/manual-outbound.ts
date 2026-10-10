import type { ProxyMap } from './node-selection';
export type ProxyMode = 'manual' | 'ai';
export type ManualOutboundState = { mode: ProxyMode; revision: string; activeProfileId: string | null; legacySelection: boolean };
const escape = (v: string) => v.replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'})[c]!);
export const manualOutboundMarkup = `<article class="panel manual-outbound-panel">
  <div class="panel-heading"><div><h2>自选节点</h2><p>按订阅策略组选择出口，保留原有分流规则。</p></div><button class="button button-quiet" id="manual-refresh">刷新</button></div>
  <div class="mode-banner"><p id="manual-current" role="status">正在读取选点方式…</p><button class="button button-primary" id="manual-apply" disabled>使用自选节点</button></div>
  <p class="hint">与 AI 代理互斥，二者仅一个生效；Codex 路由接入独立，不受切换影响。不会自动开启系统代理或 TUN。</p>
  <p id="manual-result" role="status"></p>
  <fieldset id="manual-controls"><legend class="sr-only">订阅策略组选点</legend>
    <div class="manual-filters"><label>当前配置<select id="manual-profile" aria-label="选用订阅或本地配置"></select></label><label>筛选节点<input id="manual-search" type="search" placeholder="搜索节点名称或协议" /></label><label>排序<select id="manual-sort"><option value="source">订阅顺序</option><option value="delay">延迟优先</option><option value="name">名称</option></select></label></div>
    <div class="routing-segments" id="manual-routing-mode" role="group" aria-label="自选节点路由模式"><button type="button" data-routing-mode="rule">规则</button><button type="button" data-routing-mode="global">全局</button><button type="button" data-routing-mode="direct">直连</button></div>
    <p class="hint">规则：按分流规则使用各组；全局：使用 GLOBAL；直连：不使用代理节点。自动组手动优先，失效时仍可由核心回退。</p>
    <div id="proxy-groups" class="card-list empty-state">添加并选用配置、启动核心后查看节点。</div>
  </fieldset>
</article>`;

export function nodeGridMarkup(group: string, map: ProxyMap, query: string, sort: string, busy: boolean): string {
  const value = map[group] ?? {};
  const selectable = ['Selector', 'Fallback', 'URLTest'].includes(value.type ?? '');
  const delay = (name: string) => { const history = map[name]?.history; const sample = history?.[history.length - 1]?.delay; return typeof sample === 'number' && sample > 0 ? sample : null; };
  const nodes = (value.all ?? []).filter(name => `${name} ${map[name]?.type ?? ''}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  if (sort === 'name') nodes.sort((a,b) => a.localeCompare(b));
  if (sort === 'delay') nodes.sort((a,b) => (delay(a) ?? Infinity) - (delay(b) ?? Infinity));
  return `<div class="manual-node-list" role="group" aria-label="${escape(group)} 节点">${nodes.map(name => `<div class="manual-node${value.now === name ? ' is-selected' : ''}"><button type="button" data-node-choice="${escape(name)}" data-node-group="${escape(group)}" aria-pressed="${value.now === name}" ${busy || !selectable ? 'disabled' : ''}><strong>${escape(name)}</strong><small>${escape(map[name]?.type ?? '节点')} · ${value.now === name ? '当前选中' : '候选'}</small></button><button type="button" class="proxy-delay node-latency" data-proxy="${escape(name)}" aria-label="测试 ${escape(name)} 延迟" ${busy ? 'disabled' : ''}>${delay(name) === null ? '测速' : `${delay(name)} ms`}</button></div>`).join('') || '<p class="empty-state">没有匹配节点。</p>'}</div>`;
}

export function mountManualOutbound(root: HTMLElement, options: {
  read(): Promise<ManualOutboundState>;
  write(mode: ProxyMode, expectedRevision: string): Promise<ManualOutboundState>;
  confirm(input: {title: string; message: string; confirmLabel: string}): Promise<boolean>;
  changed(state: ManualOutboundState): Promise<void>;
  error(error: unknown): string;
}) {
  let state: ManualOutboundState | null = null, busy = false, readVersion = 0;
  const el = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`)!;
  const message = (text: string) => { el('manual-result').textContent = text; };
  function accept(next: ManualOutboundState) {
    state = next;
    el('manual-current').textContent = next.legacySelection ? '旧版固定出口待迁移：请确认使用订阅策略组选点。' : !next.activeProfileId ? `未启用 · 请先添加并选用配置（已选${next.mode === 'manual' ? '自选节点' : 'AI 代理'}）` : `当前选点方式：${next.mode === 'manual' ? '自选节点' : 'AI 代理'}`;
    el<HTMLButtonElement>('manual-apply').disabled = busy || next.mode === 'manual' && !next.legacySelection;
    el<HTMLFieldSetElement>('manual-controls').disabled = busy || next.mode !== 'manual' || next.legacySelection;
  }
  async function refresh() {
    if (busy) return;
    const version = ++readVersion;
    try { const next = await options.read(); if (version === readVersion && !busy) { accept(next); message(''); } }
    catch (error) { if (version === readVersion) message(options.error(error)); }
  }
  async function apply(mode: ProxyMode) {
    if (busy || !state) return;
    busy = true; ++readVersion;
    el<HTMLButtonElement>('manual-refresh').disabled = true;
    accept(state);
    try {
      const name = mode === 'manual' ? '自选节点' : 'AI 代理';
      if (!await options.confirm({title: `切换为${name}？`, message: mode === 'manual' ? '暂停 AI 自动筛选与灾备，恢复订阅原有策略组和分流规则。既有 AI 偏好会保留，Codex 路由接入不变。核心运行中会重新加载配置。' : '暂停自选节点操作，启用当前配置中已保存的 AI 策略；尚未生成策略时需在 AI 代理页生成。Codex 路由接入不变。核心运行中会重新加载配置。', confirmLabel: `使用${name}`})) return;
      const next = await options.write(mode, state.revision);
      accept(next); message(`已切换为${name}；核心未运行时，下次启动生效。`);
      try { await options.changed(next); } catch { message('选点方式已保存，请刷新页面状态。'); }
    } catch (error) { message(options.error(error)); }
    finally { busy = false; el<HTMLButtonElement>('manual-refresh').disabled = false; if (state) accept(state); }
  }
  el('manual-refresh').addEventListener('click', () => void refresh());
  el('manual-apply').addEventListener('click', () => void apply('manual'));
  return { refresh, accept: (next: ManualOutboundState) => { if (!busy) accept(next); }, enableAi: () => apply('ai') };
}
