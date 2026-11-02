import type { BinaryInfo, NetworkMode, ProfileDetails, RuntimeStatus, SystemProxyStatus } from './types';

export type DiagnosticStage = 'local_proxy' | 'controller' | 'http_proxy';
export type ConnectivityCheck = { stage: DiagnosticStage; success: boolean; latencyMs: number | null; detail: string };
type Result = { status: 'pass' | 'warn' | 'fail' | 'skipped'; detail: string };
export type DiagnosticRow = { id: string; label: string; status: Result['status'] | 'running'; detail: string };
export type DiagnosticSnapshot = {
  phase: 'idle' | 'running' | 'completed' | 'interrupted';
  pending: boolean;
  rows: DiagnosticRow[];
  message: string;
};

/** Every published result belongs to the current run and observed proxy context. */
export function createDiagnostics(options: {
  context(): string;
  allowed(): boolean;
  networkMode(): NetworkMode | undefined;
  binary(): Promise<BinaryInfo>;
  runtime(): Promise<RuntimeStatus>;
  activeProfile(): Promise<ProfileDetails | null>;
  systemProxy(): Promise<SystemProxyStatus>;
  proxies(): Promise<unknown>;
  diagnosticStep(stage: DiagnosticStage): Promise<ConnectivityCheck>;
  error(error: unknown): string;
  changed(snapshot: DiagnosticSnapshot): void;
}) {
  let phase: DiagnosticSnapshot['phase'] = 'idle', pending = false, serial = 0;
  let rows: DiagnosticRow[] = [], notice = '';
  let flight: Promise<void> | null = null;
  function snapshot(): DiagnosticSnapshot {
    const completed = rows.filter(row => row.status !== 'running').length;
    const count = (status: DiagnosticRow['status']) => rows.filter(row => row.status === status).length;
    const checking = rows.find(row => row.status === 'running');
    const message = notice || (phase === 'running'
      ? `${checking ? `正在检测：${checking.label}` : '正在准备诊断'} · 已完成 ${completed}/8 项`
      : phase === 'completed'
        ? `诊断完成 · 通过 ${count('pass')} · 注意 ${count('warn')} · 未通过 ${count('fail')} · 跳过 ${count('skipped')}`
        : '按顺序检查核心、配置、端口与实际代理请求。');
    return { phase, pending, rows: rows.map(row => ({ ...row })), message };
  }
  const publish = () => options.changed(snapshot());
  function invalidate(message = '代理、配置或页面状态已变化，诊断已中断，请重新诊断。') {
    if (phase !== 'running') return;
    serial++;
    phase = 'interrupted'; notice = message;
    rows = rows.map(row => row.status === 'running'
      ? { ...row, status: 'skipped', detail: '本项结果未采用，请重新诊断。' } : row);
    publish();
  }
  function run(): Promise<void> {
    if (flight) return flight;
    if (!options.allowed()) return Promise.resolve();
    const request = ++serial, context = options.context(), mode = options.networkMode();
    const current = () => request === serial && context === options.context() && options.allowed();
    let running = false, runtimeKnown = false;
    phase = 'running'; pending = true; rows = []; notice = ''; publish();
    const networkSkip = () => running ? null : runtimeKnown ? '核心未运行，未执行本项检查。' : '运行状态读取失败，未执行本项检查。';
    const networkStep = (stage: DiagnosticStage) => async (): Promise<Result> => {
      const result = await options.diagnosticStep(stage);
      if (result.stage !== stage) throw new Error('诊断步骤不匹配，请重新诊断。');
      return { status: result.success ? 'pass' : 'fail', detail: result.detail + (result.latencyMs == null ? '' : ` · ${result.latencyMs} ms`) };
    };
    const steps: Array<{ id: string; label: string; check(): Promise<Result>; skip?(): string | null }> = [
      { id: 'binary', label: 'Mihomo sidecar', check: async () => {
        const binary = await options.binary();
        return { status: binary.available ? 'pass' : 'fail', detail: binary.version ?? binary.message ?? '未找到' };
      } },
      { id: 'runtime', label: '运行状态', check: async () => {
        const runtime = await options.runtime(); runtimeKnown = true; running = runtime.phase === 'running';
        return { status: running ? 'pass' : 'warn', detail: runtime.message || '未运行' };
      } },
      { id: 'profile', label: '活动配置', check: async () => {
        const profile = await options.activeProfile();
        return { status: profile ? 'pass' : 'fail', detail: profile?.profile.displayName ?? '未激活配置' };
      } },
      { id: 'system_proxy', label: '系统代理', check: async () => {
        const proxy = await options.systemProxy();
        return proxy.active ? { status: 'pass', detail: '已保存快照并接管' }
          : mode && mode !== 'system_proxy' ? { status: 'skipped', detail: '当前模式不要求接管系统代理。' }
            : { status: 'warn', detail: '未接管系统代理' };
      } },
      { id: 'api', label: 'Mihomo API', skip: networkSkip, check: async () => {
        const proxies = await options.proxies();
        return { status: proxies ? 'pass' : 'fail', detail: proxies ? '控制接口可访问' : '控制接口请求失败' };
      } },
      { id: 'local_proxy', label: '本地代理端口', skip: networkSkip, check: networkStep('local_proxy') },
      { id: 'controller', label: '控制接口端口', skip: networkSkip, check: networkStep('controller') },
      { id: 'http_proxy', label: '实际代理请求', skip: networkSkip, check: networkStep('http_proxy') },
    ];
    flight = Promise.resolve().then(async () => {
      for (const step of steps) {
        if (!current()) { invalidate(); return; }
        const reason = step.skip?.();
        if (reason) { rows.push({ id: step.id, label: step.label, status: 'skipped', detail: reason }); publish(); continue; }
        const row: DiagnosticRow = { id: step.id, label: step.label, status: 'running', detail: '检测中…' };
        rows.push(row); publish();
        let result: Result;
        try { result = await step.check(); }
        catch (error) { result = { status: 'fail', detail: options.error(error) }; }
        if (!current()) { invalidate(); return; }
        Object.assign(row, result); publish();
      }
      phase = 'completed'; publish();
    }).finally(() => { pending = false; flight = null; publish(); });
    return flight;
  }
  return { run, invalidate, snapshot };
}

const escape = (value: string) => value.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]!));
export function diagnosticRowsMarkup(snapshot: DiagnosticSnapshot): string {
  const labels = { running: '检测中', pass: '通过', warn: '注意', fail: '未通过', skipped: '已跳过' };
  const icons = { running: '', pass: '✓', warn: '!', fail: '×', skipped: '—' };
  return snapshot.rows.map(row => `<div class="diagnostic-item status-${row.status}" role="listitem" data-render-key="${escape(row.id)}" data-diagnostic-id="${escape(row.id)}"><span class="diagnostic-icon" aria-hidden="true">${icons[row.status]}</span><div><div class="diagnostic-item-heading"><strong>${escape(row.label)}</strong><span class="diagnostic-stage-status">${labels[row.status]}</span></div><p>${escape(row.detail)}</p></div></div>`).join('');
}
