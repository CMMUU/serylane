export type DownloadPath = "follow_core" | "direct" | "local_proxy";
export interface DownloadOptions { path: DownloadPath; proxyPort?: number }
export interface SubscriptionTask {
  id: string; operation: string; profileId: string | null;
  phase: "preparing" | "connecting" | "downloading" | "retrying" | "checking" | "waiting" | "validating" | "committing" | "completed" | "failed" | "cancelled";
  path: "pending" | "direct" | "serylane" | "local_proxy";
  sequence: number; elapsedMs: number; attempt: number; httpStatus: number | null; retryAfterSeconds: number | null;
  bytes: number; canCancel: boolean; message: string; errorCode: string | null;
}
export function taskDescription(task: SubscriptionTask): string {
  const paths = { pending: "正在确定下载路径", direct: "直连", serylane: "经 Serylane 当前核心", local_proxy: "经指定本地代理" };
  return `${task.message} · ${paths[task.path]} · ${Math.floor(task.elapsedMs / 1000)} 秒${task.attempt > 1 ? ` · 第 ${task.attempt} 次请求` : ""}`;
}
export function downloadOptions(path: string, port: string): DownloadOptions {
  if (!["follow_core", "direct", "local_proxy"].includes(path)) throw new Error("请选择有效的订阅下载路径");
  if (path !== "local_proxy") return { path: path as DownloadPath };
  const proxyPort = Number(port);
  if (!/^\d+$/.test(port) || !Number.isInteger(proxyPort) || proxyPort < 1 || proxyPort > 65535) {
    throw new Error("请填写 1～65535 之间的本地 HTTP 代理端口");
  }
  return { path, proxyPort };
}
// Polling is deliberately serial; late responses and an older task never overwrite current UI.
// A missed progress read does not turn a successful backend commit into an import failure.
export function observeSubscriptionTask(
  id: string,
  read: (id: string) => Promise<SubscriptionTask | null>,
  onChange: (task: SubscriptionTask) => void,
  schedule: (callback: () => void) => ReturnType<typeof setTimeout> = callback => setTimeout(callback, 400),
  unschedule: (timer: ReturnType<typeof setTimeout>) => void = clearTimeout,
): () => void {
  let stopped = false, sequence = -1;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const poll = async () => {
    try {
      const task = await read(id);
      if (!stopped && task?.id === id && task.sequence >= sequence) {
        sequence = task.sequence;
        onChange(task);
      }
    } catch { /* progress is best-effort, operation result is authoritative */ }
    if (!stopped) timer = schedule(() => void poll());
  };
  void poll();
  return () => { stopped = true; if (timer !== undefined) unschedule(timer); };
}
