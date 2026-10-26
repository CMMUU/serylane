// Job identity is separate from a node name: late replies must not follow a
// same-name node into a different subscription, mode or restarted core.
export type NodeTestScope = { kind?: "manual" | "ai"; profileId: string; revisionId: string; modeRevision: string; runtimePid: number | null; runtimeStartedAt: string | null; routingMode: string };
export type NodeTestResult = { name: string; state: 'pending' | 'testing' | 'passed' | 'timeout' | 'failed' | 'unavailable'; delayMs: number | null };
export type NodeTestSnapshot = { startedAt?: string; finishedAt?: string | null; id: string; scope: NodeTestScope; sequence: number; valid: boolean; phase: 'running' | 'cancelling' | 'completed' | 'cancelled' | 'failed'; message: string; nodes: NodeTestResult[] };
export const nodeTestRunning = (s: NodeTestSnapshot | null) => Boolean(s && ['running', 'cancelling'].includes(s.phase));
export function sameNodeTestScope(a: NodeTestScope | null, b: NodeTestScope | null): boolean {
  const timestamp = (v: string | null) => v === null ? null : Date.parse(v);
  return Boolean(a && b && (a.kind ?? "manual") === (b.kind ?? "manual") && a.profileId === b.profileId && a.revisionId === b.revisionId && a.modeRevision === b.modeRevision && a.runtimePid === b.runtimePid && timestamp(a.runtimeStartedAt) === timestamp(b.runtimeStartedAt) && a.routingMode === b.routingMode);
}
export function nodeTestSummary(s: NodeTestSnapshot): string {
  const count = (state: NodeTestResult['state']) => s.nodes.filter(n => n.state === state).length;
  const completed = s.nodes.length - count('pending') - count('testing');
  return `${s.message} · 已完成 ${completed}/${s.nodes.length} · 可用 ${count('passed')} · 超时 ${count('timeout')} · 失败 ${count('failed')} · 暂未测得 ${count('unavailable')}${nodeTestRunning(s) ? '' : ` · 未测试 ${s.nodes.length - completed}`}`;
}
export function mountNodeTest(root: HTMLElement, options: {
  context(): NodeTestScope | null;
  start(scope: NodeTestScope): Promise<NodeTestSnapshot>;
  status(): Promise<NodeTestSnapshot | null>;
  cancel(id: string): Promise<NodeTestSnapshot>;
  changed(): void;
  error(e: unknown): string;
}) {
  const start = root.querySelector<HTMLButtonElement>('#manual-test-all')!;
  const cancel = root.querySelector<HTMLButtonElement>('#manual-test-cancel')!;
  const message = root.querySelector<HTMLElement>('#manual-test-status')!;
  const progress = root.querySelector<HTMLProgressElement>('#manual-test-progress')!;
  let snapshot: NodeTestSnapshot | null = null, busy = false, reading = false, serial = 0;
  let singleScope: NodeTestScope | null = null;
  const singles = new Map<string, NodeTestResult>();
  let notice = "";
  let noticeScope: NodeTestScope | null = null;
  const current = () => snapshot?.valid && sameNodeTestScope(snapshot.scope, options.context()) ? snapshot : null;
  function render() {
    const s = current(), running = nodeTestRunning(s);
    if (!sameNodeTestScope(noticeScope, options.context())) notice = "";
    start.disabled = busy || !options.context() || running;
    start.textContent = running ? '测试进行中…' : '测试全部';
    cancel.hidden = !running; cancel.disabled = busy || s?.phase === 'cancelling';
    progress.hidden = !running;
    if (s) {
      message.textContent = nodeTestSummary(s);
      progress.max = Math.max(1, s.nodes.length);
      progress.value = s.nodes.filter(n => !['pending', 'testing'].includes(n.state)).length;
    } else if (!busy) {
      message.textContent = options.context() ? '测试当前配置全部实际节点，自动去重；仅测延迟与连通性，不主动切换出口，自动组仍按原策略运行。' : '在自选节点模式选用配置并启动核心后可测试；直连模式不测试代理节点。';
    }
    if (notice) message.textContent = notice;
  }
  function accept(next: NodeTestSnapshot | null) {
    if (next && snapshot?.id === next.id && next.sequence <= snapshot.sequence) return;
    if (!next && !snapshot) return;
    if (next?.id !== snapshot?.id) singles.clear();
    snapshot = next; render(); options.changed();
  }
  async function refresh() {
    render();
    if (reading || busy || !options.context()) return;
    const request = serial, scope = options.context(); reading = true;
    try { const next = await options.status(); if (request === serial && sameNodeTestScope(scope, options.context()) && !busy) accept(next); }
    catch { if (request === serial && !busy) message.textContent = '测试状态暂未读取成功，正在重试；不要重复启动任务。'; }
    finally { reading = false; }
  }
  async function run(stop = false) {
    const scope = options.context(), task = current();
    if (busy || !scope || stop && !nodeTestRunning(task) || !stop && nodeTestRunning(task)) return;
    const request = ++serial; if (!stop) singles.clear(); busy = true; notice = ""; render(); options.changed();
    try {
      const next = stop ? await options.cancel(task!.id) : await options.start(scope);
      if (request === serial && sameNodeTestScope(scope, options.context())) accept(next);
    } catch (e) { if (request === serial && sameNodeTestScope(scope, options.context())) { notice = options.error(e); noticeScope = scope; message.textContent = notice; } }
    finally { busy = false; render(); options.changed(); }
  }
  start.addEventListener('click', () => void run());
  cancel.addEventListener('click', () => void run(true));
  render();
  function recordSingle(scope: NodeTestScope | null, revision: number, result: NodeTestResult) {
    if (revision !== serial || nodeTestRunning(current()) || !sameNodeTestScope(scope, options.context())) return;
    if (!sameNodeTestScope(singleScope, scope)) singles.clear();
    singleScope = scope; singles.set(result.name, result); options.changed();
  }
  function results(): NodeTestResult[] {
    const rows = new Map((current()?.nodes ?? []).map(n => [n.name, n]));
    if (sameNodeTestScope(singleScope, options.context())) for (const [name, result] of singles) rows.set(name, result);
    return [...rows.values()];
  }
  return { refresh, render, snapshot: current, results, recordSingle, revision: () => serial, busy: () => busy || nodeTestRunning(current()) };
}
