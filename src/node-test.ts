// Job identity is separate from a node name: late replies must not follow a
// same-name node into a different subscription, mode or restarted core.
export type NodeTestScope = { kind?: "manual" | "ai"; profileId: string; revisionId: string; modeRevision: string; runtimePid: number | null; runtimeStartedAt: string | null; routingMode: string };
export type NodeTestResult = { name: string; state: 'pending' | 'testing' | 'passed' | 'timeout' | 'failed' | 'unavailable'; delayMs: number | null };
export type NodeTestSnapshot = { startedAt?: string; finishedAt?: string | null; id: string; scope: NodeTestScope; sequence: number; valid: boolean; phase: 'running' | 'cancelling' | 'completed' | 'cancelled' | 'failed'; message: string; nodes: NodeTestResult[] };
type SingleTestOutcome = { kind: 'complete'; delayMs: number | null } | { kind: 'failed'; error: unknown };
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
  let snapshot: NodeTestSnapshot | null = null, reading = false, serial = 0;
  let batch: { scope: NodeTestScope; stopping: boolean } | null = null;
  let observedScope: NodeTestScope | null = null, scopeEpoch = 0;
  const singleFlights = new Set<{ scope: NodeTestScope; group: string; name: string }>();
  const singles = new Map<string, NodeTestResult>();
  let notice = "";
  let noticeScope: NodeTestScope | null = null;
  function context() {
    const next = options.context();
    if ((next || observedScope) && !sameNodeTestScope(observedScope, next)) {
      observedScope = next;
      scopeEpoch++;
      singles.clear();
    }
    return next;
  }
  const current = (scope = context()) => snapshot?.valid && sameNodeTestScope(snapshot.scope, scope) ? snapshot : null;
  const batchBusy = () => Boolean(batch) || nodeTestRunning(current());
  function buttonState(button: HTMLButtonElement, disabled: boolean, pending: boolean, label: string) {
    if (button.disabled !== disabled) button.disabled = disabled;
    if (button.textContent !== label) button.textContent = label;
    if (button.getAttribute('aria-busy') !== String(pending)) button.setAttribute('aria-busy', String(pending));
  }
  function render() {
    const scope = context(), s = current(scope), running = nodeTestRunning(s);
    const ownBatch = batch && sameNodeTestScope(batch.scope, scope);
    const starting = ownBatch && !batch!.stopping;
    const stopping = running && (ownBatch && batch!.stopping || s?.phase === 'cancelling');
    if (!sameNodeTestScope(noticeScope, scope)) notice = "";
    buttonState(start, Boolean(batch) || !scope || running, starting || running && !stopping, starting ? '正在开始测试…' : running ? '测试进行中…' : '测试全部');
    cancel.hidden = !running;
    buttonState(cancel, Boolean(batch) || stopping, stopping, stopping ? '正在停止…' : '停止测试');
    progress.hidden = !running;
    let status = scope ? '测试当前配置全部实际节点，自动去重；仅测延迟与连通性，不主动切换出口，自动组仍按原策略运行。' : '在自选节点模式选用配置并启动核心后可测试；直连模式不测试代理节点。';
    if (s) {
      status = nodeTestSummary(s);
      progress.max = Math.max(1, s.nodes.length);
      progress.value = s.nodes.filter(n => !['pending', 'testing'].includes(n.state)).length;
    }
    status = notice || (starting ? '正在读取当前配置与节点，准备测速…' : status);
    if (message.textContent !== status) message.textContent = status;
  }
  function accept(next: NodeTestSnapshot | null) {
    if (next && snapshot?.id === next.id && next.sequence <= snapshot.sequence) return;
    if (!next && !snapshot) return;
    if (next?.id !== snapshot?.id) singles.clear();
    snapshot = next; render(); options.changed();
  }
  async function refresh() {
    render();
    const scope = context();
    if (reading || batch || !scope) return;
    const request = serial, epoch = scopeEpoch; reading = true;
    const matches = () => sameNodeTestScope(scope, context()) && epoch === scopeEpoch && request === serial && !batch;
    try { const next = await options.status(); if (matches() && (!next || sameNodeTestScope(next.scope, scope))) accept(next); }
    catch { if (matches()) message.textContent = '测试状态暂未读取成功，正在重试；不要重复启动任务。'; }
    finally { reading = false; }
  }
  async function run(stop = false) {
    const scope = context(), task = current(scope);
    if (batch || !scope || stop && !nodeTestRunning(task) || !stop && nodeTestRunning(task)) return;
    const request = ++serial, epoch = scopeEpoch;
    const matches = () => sameNodeTestScope(scope, context()) && epoch === scopeEpoch && request === serial;
    if (!stop) singles.clear();
    batch = { scope, stopping: stop }; notice = ""; render(); options.changed();
    try {
      const next = stop ? await options.cancel(task!.id) : await options.start(scope);
      if (matches() && sameNodeTestScope(scope, next.scope)) accept(next);
    } catch (e) { if (matches()) { notice = options.error(e); noticeScope = scope; } }
    finally { batch = null; render(); options.changed(); }
  }
  start.addEventListener('click', () => void run());
  cancel.addEventListener('click', () => void run(true));
  render();
  function pendingSingles(): ReadonlyMap<string, string> {
    const scope = context();
    return new Map(Array.from(singleFlights).filter(flight => sameNodeTestScope(scope, flight.scope)).map(flight => [flight.name, flight.group]));
  }
  async function runSingle(group: string, name: string, measure: () => Promise<Record<string, unknown>>): Promise<SingleTestOutcome | null> {
    const scope = context();
    if (!scope || batchBusy() || pendingSingles().has(name)) return null;
    const request = serial, epoch = scopeEpoch;
    const flight = { scope, group, name };
    singleFlights.add(flight);
    const matches = () => sameNodeTestScope(scope, context()) && epoch === scopeEpoch && request === serial && !batchBusy();
    try {
      options.changed(); // Render before starting the IPC, including on a second row for this node.
      const result = await measure();
      if (!matches()) return null;
      const delay = Number(result.delay);
      const delayMs = Number.isFinite(delay) && delay > 0 ? delay : null;
      singles.set(name, {name, state: delayMs === null ? 'unavailable' : 'passed', delayMs});
      return { kind: 'complete', delayMs };
    } catch (error) {
      if (!matches()) return null;
      singles.set(name, {name, state: 'unavailable', delayMs: null});
      return { kind: 'failed', error };
    } finally {
      // An old response may arrive after a new scope has started the same node.
      singleFlights.delete(flight);
      options.changed();
    }
  }
  function results(): NodeTestResult[] {
    const rows = new Map((current()?.nodes ?? []).map(n => [n.name, n]));
    for (const [name, result] of singles) rows.set(name, result);
    return [...rows.values()];
  }
  return { refresh, render, snapshot: current, results, pendingSingles, runSingle, busy: batchBusy };
}
