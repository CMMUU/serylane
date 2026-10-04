export type ManualNode = { profileId: string; revisionId: string; profileName: string; name: string; protocol: string };
export type ManualOutboundState = { selection: { profileId: string; nodeName: string } | null; nodes: ManualNode[]; notices: string[] };
const escape = (v: string) => v.replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]!);

export const manualOutboundMarkup = `<article class="panel manual-outbound-panel">
  <div class="panel-heading"><div><h2>自选节点</h2><p>从已添加订阅中固定一个出口。</p></div><button class="button button-quiet" id="manual-refresh">刷新节点</button></div>
  <p class="warning-box">与「代理」页互斥，二选一。启用后，订阅策略组、自动灾备及用户分流规则暂不参与，进入核心的流量使用所选节点；切回代理模式后恢复订阅策略。节点失效不会自动改选其他出口。</p>
  <p id="manual-current" role="status">正在读取出口方式…</p>
  <button class="button button-quiet" id="manual-disable" disabled>切回代理模式</button>
  <div class="manual-filters"><label>订阅<select id="manual-profile" aria-label="自选节点所属订阅"><option value="">全部订阅</option></select></label><label>筛选节点<input id="manual-search" type="search" placeholder="节点名称或协议" /></label></div>
  <p id="manual-notices" class="hint"></p>
  <div id="manual-node-list" class="manual-node-list" role="radiogroup" aria-label="可自选节点"></div>
  <div class="manual-apply"><button class="button button-primary" id="manual-apply" disabled>启用所选节点</button><span class="hint">不会自动开启系统代理或 TUN。</span></div>
  <p id="manual-result" role="status"></p>
</article>`;

export function mountManualOutbound(root: HTMLElement, options: {
  read(): Promise<ManualOutboundState>;
  write(node: ManualNode | null): Promise<ManualOutboundState>;
  confirm(input: {title: string; message: string; confirmLabel: string}): Promise<boolean>;
  changed(state: ManualOutboundState): Promise<void>;
  error(error: unknown): string;
}) {
  let state: ManualOutboundState | null = null, chosen: ManualNode | null = null, busy = false, readVersion = 0;
  const el = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`)!;
  const key = (node: ManualNode) => JSON.stringify([node.profileId, node.revisionId, node.name]);
  const message = (text: string) => { el('manual-result').textContent = text; };
  function list() {
    const profile = el<HTMLSelectElement>('manual-profile').value;
    const query = el<HTMLInputElement>('manual-search').value.trim().toLocaleLowerCase();
    const rows = (state?.nodes ?? []).map((node, index) => ({node, index})).filter(({node}) => (!profile || node.profileId === profile) && `${node.name} ${node.protocol}`.toLocaleLowerCase().includes(query));
    if (chosen && !rows.some(({node}) => key(node) === key(chosen!))) chosen = null;
    el('manual-node-list').innerHTML = rows.map(({node, index}) => `<label class="manual-node"><input type="radio" name="manual-node" value="${index}" ${chosen && key(chosen) === key(node) ? 'checked' : ''} ${busy ? 'disabled' : ''}/><span><strong>${escape(node.name)}</strong><small>${escape(node.profileName)} · ${escape(node.protocol)}</small></span></label>`).join('') || '<p class="empty-state">没有匹配的独立节点。可先添加或更新订阅。</p>';
    el<HTMLButtonElement>('manual-apply').disabled = busy || !chosen;
  }
  function accept(next: ManualOutboundState) {
    state = next;
    if (chosen && !next.nodes.some(n => key(n) === key(chosen!))) chosen = null;
    el('manual-current').textContent = next.selection ? `当前：自选节点 · ${next.selection.nodeName}` : '当前：代理模式 · 使用订阅策略组与选点设置';
    el<HTMLButtonElement>('manual-disable').disabled = busy || !next.selection;
    const select = el<HTMLSelectElement>('manual-profile'), previous = select.value;
    const profiles = new Map(next.nodes.map(n => [n.profileId, n.profileName]));
    select.innerHTML = '<option value="">全部订阅</option>' + [...profiles].map(([id, name]) => `<option value="${escape(id)}">${escape(name)}</option>`).join('');
    select.value = profiles.has(previous) ? previous : '';
    el('manual-notices').textContent = next.notices.join(' ');
    list();
  }
  async function refresh() {
    if (busy) return;
    const version = ++readVersion;
    try { const next = await options.read(); if (version === readVersion && !busy) { accept(next); message(''); } }
    catch (error) { if (version === readVersion) message(options.error(error)); }
  }
  async function apply(node: ManualNode | null) {
    if (busy || !state) return;
    busy = true; ++readVersion;
    el<HTMLButtonElement>('manual-refresh').disabled = true;
    el<HTMLButtonElement>('manual-disable').disabled = true;
    list();
    try {
      if (!await options.confirm({title: node ? '启用自选节点' : '切回代理模式',
        message: node ? `选用“${node.profileName}”中的“${node.name}”并重新加载配置。代理页的策略组、自动灾备及用户分流规则暂不参与，后续进入核心的请求固定使用此节点。` : '退出固定出口，重新加载当前订阅的原有策略组、自动选点及用户分流设置。',
        confirmLabel: node ? '确认启用' : '确认切回'})) return;
      const next = await options.write(node);
      accept(next);
      message(node ? '自选节点已保存；核心已运行时立即生效，未运行时下次启动生效。' : '已恢复代理模式。');
      try { await options.changed(next); } catch { message('出口方式已保存，页面状态暂未刷新，请点击刷新。'); }
    } catch (error) { message(options.error(error)); }
    finally { busy = false; el<HTMLButtonElement>('manual-refresh').disabled = false; if (state) accept(state); }
  }
  el('manual-node-list').addEventListener('change', event => {
    const input = event.target as HTMLInputElement;
    if (!busy && input.name === 'manual-node') { chosen = state?.nodes[Number(input.value)] ?? null; el<HTMLButtonElement>('manual-apply').disabled = !chosen; }
  });
  el('manual-search').addEventListener('input', list);
  el('manual-profile').addEventListener('change', list);
  el('manual-refresh').addEventListener('click', () => void refresh());
  el('manual-apply').addEventListener('click', () => void apply(chosen));
  el('manual-disable').addEventListener('click', () => void apply(null));
  return { refresh, accept: (next: ManualOutboundState) => { if (!busy) accept(next); }, disable: () => apply(null) };
}
