import type { NetworkMode, RuntimeStatus, SystemProxyStatus } from './types';

export type IngressState = 'stopped' | 'system_proxy' | 'tun' | 'local_only' | 'unavailable' | 'unknown';
export type ProxyActivity = 'starting' | 'stopping' | 'switching' | 'restoring' | 'working';
export type ProxyStateInput = {
  runtime: RuntimeStatus | null;
  mode?: NetworkMode;
  systemProxy: SystemProxyStatus | null;
  ingress?: { state: IngressState; message: string } | null;
  activity?: ProxyActivity | null;
  readIssue?: string;
  stopIssue?: string;
};

/** Public connection state is distinct from the remembered mode and core details. */
export function proxyPresentation(input: ProxyStateInput) {
  const result = (state: 'closed' | 'system_proxy' | 'tun' | 'local_only' | 'checking' | 'unavailable' | 'error', label: string, message = '') => ({
    state, label, message, active: state === 'system_proxy' || state === 'tun',
    systemProxyActive: state === 'system_proxy', tunActive: state === 'tun',
  });
  const progress: Record<ProxyActivity, string> = {
    starting: '正在开启代理', stopping: '正在关闭代理', switching: '正在切换网络',
    restoring: '正在恢复代理', working: '正在处理代理',
  };
  if (input.activity) return result('checking', progress[input.activity]);
  if (input.stopIssue) return result('error', '关闭未完成', input.stopIssue);
  if (input.readIssue || !input.runtime) return result('checking', '状态待确认', input.readIssue || '正在读取代理状态');
  const phase = input.runtime.phase;
  if (['validating', 'starting', 'stopping', 'recovering'].includes(phase)) {
    return result('checking', phase === 'stopping' ? '正在关闭代理' : phase === 'recovering' ? '正在恢复代理' : '正在开启代理');
  }
  if (phase === 'crashed') return result('error', '代理异常退出', input.runtime.lastError || input.runtime.message);
  if (phase !== 'running') {
    if (!input.systemProxy) return result('checking', '关闭状态待确认', '尚未读到系统代理状态');
    if (input.systemProxy.active) return result('error', '关闭未完成', '核心已停止，系统代理仍指向本应用，请重试停止或查看日志');
    return result('closed', '已关闭');
  }
  if (input.ingress?.state === 'tun') return result('tun', 'TUN 运行中', input.ingress.message);
  if (input.mode === 'system_proxy') {
    if (!input.systemProxy) return result('checking', '系统代理待确认');
    return input.systemProxy.active
      ? result('system_proxy', '系统代理运行中')
      : result('unavailable', '系统代理未接管', '核心仍在运行，系统代理设置未指向本应用');
  }
  if (input.mode === 'tun') {
    if (!input.ingress || input.ingress.state === 'unknown' || input.ingress.state === 'stopped') return result('checking', 'TUN 接管待确认', input.ingress?.message);
    return result('unavailable', 'TUN 尚未接管', input.ingress.message);
  }
  if (input.systemProxy?.active) return result('system_proxy', '系统代理运行中');
  return result('local_only', '本地代理运行中', '高级 Manual 模式，仅向明确使用本地端口的应用提供代理');
}
