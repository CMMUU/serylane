import { sameNodeTestScope, nodeTestRunning, nodeTestSummary, type NodeTestScope, type NodeTestSnapshot } from './node-test';

// One UI operation at a time; a stale poll or changed subscription cannot adopt a result.
export function createAiHealth(options: {
  context(): NodeTestScope | null;
  start(scope: NodeTestScope): Promise<NodeTestSnapshot>;
  status(): Promise<NodeTestSnapshot | null>;
  cancel(id: string): Promise<NodeTestSnapshot>;
  changed(): void;
  error(e: unknown): string;
}) {
  let scope: NodeTestScope | null = null, snapshot: NodeTestSnapshot | null = null;
  let serial = 0, reading = false, notice = '';
  let pendingAction: 'start' | 'cancel' | null = null;
  function sync() {
    const next = options.context();
    if (scope === null && next === null || sameNodeTestScope(scope, next)) return;
    scope = next; serial++; snapshot = null; pendingAction = null; notice = '';
  }
  function accept(next: NodeTestSnapshot | null) {
    if (!next) { snapshot = null; return; }
    if (!next.valid) {
      if (snapshot?.id === next.id) { snapshot = null; notice = next.message; }
      return;
    }
    if (!sameNodeTestScope(next.scope, scope)) return;
    if (snapshot?.id === next.id && next.sequence <= snapshot.sequence) return;
    snapshot = next;
  }
  function view() {
    sync();
    return { snapshot, pending: pendingAction !== null, starting: pendingAction === 'start',
      cancelling: pendingAction === 'cancel' || snapshot?.phase === 'cancelling',
      running: nodeTestRunning(snapshot), enabled: Boolean(scope), notice };
  }
  async function run(cancel = false) {
    sync();
    const running = nodeTestRunning(snapshot);
    if (!scope || pendingAction || (cancel ? !running || snapshot?.phase === 'cancelling' : running)) return;
    const request = ++serial, context = scope, id = snapshot?.id;
    pendingAction = cancel ? 'cancel' : 'start'; notice = cancel ? '正在取消健康检查…' : '正在开始健康检查…'; options.changed();
    try {
      const result = cancel ? await options.cancel(id!) : await options.start(context);
      sync();
      if (request === serial && sameNodeTestScope(context, scope)) { accept(result); notice = ''; }
    } catch (error) {
      sync();
      if (request === serial) notice = options.error(error);
    } finally {
      if (request === serial) pendingAction = null;
      options.changed();
    }
  }
  async function refresh() {
    sync();
    if (!scope || pendingAction || reading) return;
    const request = serial, context = scope; reading = true;
    try {
      const result = await options.status(); sync();
      if (request === serial && sameNodeTestScope(context, scope) && !pendingAction) {
        accept(result);
        if (notice.startsWith('健康检查状态暂未读取')) notice = '';
      }
    } catch {
      sync();
      if (request === serial) notice = '健康检查状态暂未读取成功，正在重试；无需重复启动。';
    } finally { reading = false; options.changed(); }
  }
  return { view, run, refresh };
}

const escape = (value: unknown) => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]!));
export function aiHealthMarkup(view: ReturnType<ReturnType<typeof createAiHealth>['view']>, resultsOpen = false): string {
  const { snapshot: s, notice, pending, running, cancelling } = view;
  if (!s && !notice) return '';
  const completed = s?.nodes.filter(n => !['pending', 'testing'].includes(n.state)).length ?? 0;
  const state = {pending: '未测试', testing: '检查中', passed: '可达', timeout: '超时', failed: '检测未通过', unavailable: '暂未测得'};
  return `<section class="ai-health-results" data-render-key="ai-health-results" aria-live="polite" aria-busy="${pending || running}">
    <p role="status">${escape(notice || (s ? nodeTestSummary(s) : ''))}</p>
    ${s ? `<p class="hint">${s.finishedAt ? '上次检查完成' : '本次开始'}：${escape(new Date(s.finishedAt ?? s.startedAt ?? '').toLocaleTimeString('zh-CN', {hour12: false}))} · 基础连通性检测，不代表账号登录或模型请求已验证。</p>` : ''}
    ${running ? `<progress data-render-key="ai-health-progress" max="${s?.nodes.length || 1}" value="${completed}" aria-label="健康检查进度"></progress><button type="button" class="button button-quiet" data-render-key="ai-health-cancel" data-openai-action="health-cancel" data-node-action-loading aria-busy="${cancelling}" ${cancelling ? 'disabled' : ''}>${cancelling ? '正在取消…' : '取消检查'}</button>` : ''}
    ${s ? `<details data-render-key="ai-health-details" ${resultsOpen ? 'open' : ''}><summary>节点检查结果 · ${completed}/${s.nodes.length}</summary><ul>${s.nodes.map(n => `<li><span>${escape(n.name)}</span><strong>${state[n.state]}${n.delayMs !== null ? ` · ${n.delayMs} ms` : ''}</strong></li>`).join('')}</ul></details>` : ''}
  </section>`;
}
