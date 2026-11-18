import { formatLocale, setLocalizedText } from "./i18n";
import type { ApplicationLogSnapshot, RuntimeLog } from "./types";

type Entry = { timestamp: string | number; level: string; source: string; message: string; count?: number };
export function applicationLogHelp(retentionHours: number, maxBytes: number) {
  return `最多保留 ${retentionHours / 24} 天 · 紧凑存储上限 ${Math.round(maxBytes / 1024)} KiB / 2,000 条，达到上限先淘汰旧记录 · 可在设置调整期限 · 相邻重复事件合并 · 不记录对话或订阅凭据。`;
}
export function filterLogs(entries: Entry[], level: string, query: string) {
  const needle = query.trim().toLocaleLowerCase();
  return entries.filter(entry => (level === "all" || entry.level.replace("warning", "warn") === level)
    && (!needle || `${entry.source} ${entry.message}`.toLocaleLowerCase().includes(needle)));
}

export function mountLogs(root: HTMLElement, context: {
  api: { applicationLogs(): Promise<ApplicationLogSnapshot>; logs(limit: number): Promise<RuntimeLog[]>; clearApplicationLogs(): Promise<void>; clearLogs(): Promise<void> };
  visible?: () => boolean;
  confirm(options: { title: string; message: string; confirmLabel?: string; returnFocus?: HTMLElement | null }): Promise<boolean>;
}) {
  root.innerHTML = `<article class="panel application-logs-panel">
    <div class="panel-heading"><div><div class="section-label">LOGS</div><h2><!--i18n:ui-7dbac1c20f237b-->日志</h2></div><div class="toolbar"><button class="button button-quiet" id="logs-refresh"><!--i18n:ui-aee88743413144-->刷新</button><button class="button button-danger" id="logs-clear"><!--i18n:ui-fb235b01fafbe9-->清空当前日志</button></div></div>
    <div class="logs-filters"><label><!--i18n:ui-f3bf2f426d2e39-->日志来源<select id="logs-source"><option value="app"><!--i18n:ui-a0e16ca7ea6591-->应用日志</option><option value="core"><!--i18n:ui-41b7476dee6143-->Mihomo 日志</option></select></label><label><!--i18n:ui-3d9d02e83d396e-->级别<select id="logs-level"><option value="all"><!--i18n:ui-81f9b5554532b9-->全部级别</option><option value="info"><!--i18n:ui-e7028601e7da79-->信息</option><option value="warn"><!--i18n:ui-a8b7a4480407ac-->警告</option><option value="error"><!--i18n:ui-0bc1fb72ae1be5-->错误</option></select></label><label class="logs-search"><!--i18n:ui-3003318e729a86-->查找<input id="logs-search" type="search" placeholder="按事件或关键词筛选"  data-i18n-placeholder="ui-aa464c8c5a5758"/></label></div>
    <p class="hint" id="logs-help"></p><p id="logs-status" class="hint" role="status" aria-live="polite"></p>
    <div id="log-list" class="log-list empty-state" tabindex="0" aria-label="日志内容" data-i18n-aria-label="ui-b97249cc8c2d1d"><!--i18n:ui-87e0cec6c5edd3-->正在读取日志</div>
    <button class="button button-quiet is-hidden" id="logs-more"><!--i18n:ui-331af79f34e7a5-->显示更多</button>
  </article>`;
  const get = <T extends HTMLElement>(selector: string) => root.querySelector<T>(selector)!;
  const source = get<HTMLSelectElement>("#logs-source");
  const level = get<HTMLSelectElement>("#logs-level");
  const query = get<HTMLInputElement>("#logs-search");
  const list = get<HTMLElement>("#log-list");
  const more = get<HTMLButtonElement>("#logs-more");
  const status = get<HTMLElement>("#logs-status");
  let entries: Entry[] = [];
  let limit = 200;
  let revision = 0;
  let busy = false;
  let warning = "";
  let retention: { hours: number; maxBytes: number } | null = null;
  const render = () => {
    if (context.visible && !context.visible()) return;
    const filtered = filterLogs(entries, level.value, query.value);
    const fragment = document.createDocumentFragment();
    for (const entry of filtered.slice(0, limit)) {
      const row = document.createElement("div");
      const normalized = entry.level.replace("warning", "warn");
      row.className = `log-row level-${["info", "warn", "error"].includes(normalized) ? normalized : "info"}`;
      const time = document.createElement("time");
      time.textContent = new Date(entry.timestamp).toLocaleString(formatLocale(), { hour12: false });
      const area = document.createElement("span");
      setLocalizedText(area, `${entry.source} · ${{ info: "信息", warn: "警告", error: "错误" }[normalized] ?? entry.level}`);
      const message = document.createElement("p");
      message.textContent = entry.message + ((entry.count ?? 1) > 1 ? `（合并 ${entry.count} 次）` : "");
      row.append(time, area, message);
      fragment.append(row);
    }
    const scroll = list.scrollTop;
    list.replaceChildren(fragment);
    list.classList.toggle("empty-state", !filtered.length);
    if (!filtered.length) setLocalizedText(list, entries.length ? "没有匹配的日志" : "暂无日志");
    list.scrollTop = scroll;
    more.classList.toggle("is-hidden", filtered.length <= limit);
    setLocalizedText(status, warning || `共 ${filtered.length} 条 · 已显示 ${Math.min(limit, filtered.length)} 条 · 最新在前`);
    setLocalizedText(get("#logs-help"), source.value === "app"
      ? retention ? applicationLogHelp(retention.hours, retention.maxBytes) : "正在读取应用日志保留策略…"
      : "Mihomo 当前会话日志保存在有界内存中；清空不会停止代理，也不会清除应用日志。");
  };
  const refresh = async () => {
    if (busy) return;
    const refreshButton = get<HTMLButtonElement>('#logs-refresh');
    refreshButton.disabled = true; setLocalizedText(refreshButton, '正在刷新…');
    const requested = ++revision;
    const selected = source.value;
    busy = true;
    try {
      const data = selected === "app" ? await context.api.applicationLogs() : await context.api.logs(500);
      if (requested !== revision) return;
      entries = Array.isArray(data) ? [...data].reverse() : data.entries;
      warning = Array.isArray(data) ? "" : data.storageError ?? "";
      if (!Array.isArray(data)) retention = { hours: data.retentionHours, maxBytes: data.maxBytes };
      render();
    } catch {
      if (requested === revision) { warning = "日志读取失败，请点击刷新重试；未更改代理状态。"; render(); }
    } finally { if (requested === revision) { busy = false; refreshButton.disabled = false; setLocalizedText(refreshButton, '刷新'); } }
  };
  source.addEventListener("change", () => {
    revision++; busy = false; entries = []; limit = 200; warning = ""; render(); void refresh();
  });
  level.addEventListener("change", () => { limit = 200; render(); });
  query.addEventListener("input", () => { limit = 200; render(); });
  more.addEventListener("click", () => { limit += 200; render(); });
  get("#logs-refresh").addEventListener("click", () => void refresh());
  get("#logs-clear").addEventListener("click", async () => {
    const selected = source.value;
    const confirmed = await context.confirm({ title: "清空当前日志", message: `仅清空${selected === "app" ? "应用" : "Mihomo"}日志，无法恢复；不会更改代理、订阅或其他日志。`, confirmLabel: "清空", returnFocus: get("#logs-clear") });
    if (!confirmed || source.value !== selected) return;
    revision++; busy = true;
    source.disabled = true;
    get<HTMLButtonElement>("#logs-clear").disabled = true;
    try {
      if (selected === "app") await context.api.clearApplicationLogs(); else await context.api.clearLogs();
      entries = []; warning = ""; render();
    } catch { warning = "清空失败，原日志可能仍保留；请刷新核对。"; render(); }
    finally { busy = false; source.disabled = false; get<HTMLButtonElement>("#logs-clear").disabled = false; }
    await refresh();
  });
  render();
  return { refresh };
}
