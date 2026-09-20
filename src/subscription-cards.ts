import type { OpenAiPolicyTask, SubscriptionOverview, SubscriptionUsage, SubscriptionStatus } from "./types";

// A delayed full-page read must not replace a newer background quota event.
export function newestSubscriptionStatus(previous: SubscriptionStatus | null | undefined, incoming: SubscriptionStatus | null | undefined) {
  const time = (status: SubscriptionStatus | null | undefined) => {
    const value = Date.parse(status?.checkedAt ?? "");
    return Number.isFinite(value) ? value : -Infinity;
  };
  return time(previous) > time(incoming) ? previous : incoming;
}

const escape = (value: unknown) => String(value ?? "").replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
const byteValue = (value: unknown): number | null => typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;

export function subscriptionBytes(value: unknown): string {
  const bytes = byteValue(value);
  if (bytes === null) return "—";
  if (bytes === 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
  const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${new Intl.NumberFormat("zh-CN", { maximumFractionDigits: unit > 0 ? 1 : 0 }).format(bytes / 1024 ** unit)} ${units[unit]}`;
}

export function subscriptionDate(value: string | null | undefined): string {
  if (!value) return "尚未记录";
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? new Intl.DateTimeFormat("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }).format(date) : "尚未记录";
}

export function describeSubscriptionUsage(usage: SubscriptionUsage | null | undefined, now = Date.now()) {
  const upload = byteValue(usage?.uploadBytes), download = byteValue(usage?.downloadBytes);
  const total = byteValue(usage?.totalBytes);
  const used = upload !== null && download !== null ? byteValue(upload + download) : null;
  const quota = total !== null && total > 0 ? total : null;
  const percent = used !== null && quota !== null ? used / quota * 100 : null;
  const expiry = byteValue(usage?.expiresAt);
  const expiryDate = expiry !== null && expiry > 0 ? new Date(expiry * 1000) : null;
  const validExpiry = expiryDate && Number.isFinite(expiryDate.getTime()) ? expiryDate : null;
  const expired = Boolean(validExpiry && validExpiry.getTime() <= now);
  const exhausted = used !== null && quota !== null && used >= quota;
  const remaining = used !== null && quota !== null ? Math.max(0, quota - used) : null;
  return {
    upload, download, total, used, quota, percent, remaining, expired, exhausted,
    progress: percent === null ? null : Math.max(0, Math.min(100, percent)),
    expires: validExpiry ? new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }).format(validExpiry) : "服务商未提供有效时间",
    state: exhausted ? "额度已用尽" : percent !== null && percent >= 90 ? "额度即将用尽" : used === null || quota === null ? "用量信息不完整" : "剩余流量",
  };
}

export function subscriptionCardMarkup(subscription: SubscriptionOverview, task: OpenAiPolicyTask | null, now = Date.now(), refreshing = false): string {
  const { profile, summary, latestMetadata, latestValidation, status } = subscription;
  const usageData = status?.usage ?? latestMetadata?.usage;
  const usage = describeSubscriptionUsage(usageData, now);
  const sampleAt = status?.usageUpdatedAt ?? (latestMetadata?.usage ? subscription.latestFetchedAt : null);
  const noCurrentSample = Boolean(status?.checkedAt && sampleAt && status.checkedAt !== sampleAt);
  const host = profile.source.type === "remote_subscription" ? profile.source.host : "远程订阅";
  const taskForProfile = task?.profileId === profile.id;
  const generating = Boolean(taskForProfile && task?.running);
  const failed = Boolean(taskForProfile && !task?.running && task?.phase === "failed");
  const taskProgress = generating ? ` ${task!.completed}/${task!.total || "—"}` : "";
  const policyText = generating ? `正在生成灾备${taskProgress}` : failed ? "上次灾备生成失败" : profile.openaiPolicy.enabled ? `OpenAI 灾备 · ${profile.openaiPolicy.selectedNodes.length} 个节点` : "未启用 OpenAI 灾备";
  const configuration = !latestValidation ? "尚未验证配置" : latestValidation.valid ? "配置校验通过" : "配置校验未通过";
  const mode = profile.routingMode === "global" ? "全局" : profile.routingMode === "direct" ? "直连" : "规则";
  const warning = usage.exhausted || usage.expired;
  const usageNote = !usageData ? "刷新订阅以获取用量；服务商未提供时显示 —。" : noCurrentSample ? "本次检查未获取新用量，保留上次采样。" : "每 5 分钟直接检查订阅用量；余额变化即更新，不切换订阅或重载配置。";
  const quotaLabel = usage.quota === null ? "额度未提供" : `共 ${subscriptionBytes(usage.quota)}`;
  const progress = usage.progress === null
    ? '<div class="subscription-usage-track is-unknown" aria-hidden="true"></div>'
    : `<div class="subscription-usage-track" role="progressbar" aria-label="${escape(profile.displayName)} 套餐流量" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${usage.progress.toFixed(2)}" aria-valuetext="已用 ${subscriptionBytes(usage.used)}，总额度 ${subscriptionBytes(usage.quota)}${usage.exhausted ? "，额度已用尽" : ""}"><span style="width:${usage.progress.toFixed(2)}%"></span></div>`;
  const id = escape(profile.id);
  const button = (action: string, label: string, extra = "") => `<button type="button" class="button button-quiet${action === "delete" ? " button-danger" : ""}" data-subscription-action="${action}" data-profile-id="${id}" ${refreshing ? "disabled" : extra}>${escape(refreshing && action === "refresh" ? "刷新中…" : label)}</button>`;
  return `<article class="subscription-entry subscription-tile${subscription.active ? " is-active" : ""}${warning ? " has-warning" : ""}" data-subscription-id="${id}">
    <div class="subscription-entry-head">
      <div class="subscription-identity"><span class="subscription-mark" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="3"/><path d="M7 8h3m4 0h3M3 13h6l2-3h6v11"/></svg></span><div><h3 title="${escape(profile.displayName)}">${escape(profile.displayName)}</h3><p title="${escape(host)}">${escape(host)} · 凭据已隐藏</p></div></div>
      <span class="subscription-state${subscription.active ? " is-active" : ""}">${subscription.active ? "已选用" : "未选用"}</span>
    </div>
    <section class="subscription-usage" aria-label="套餐流量">
      <div class="subscription-usage-label"><span>${usageData ? usage.state : "流量信息未提供"}</span>${usage.percent === null ? "" : `<span>${new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 1 }).format(usage.percent)}% 已用</span>`}</div>
      <div class="subscription-usage-value"><strong>${subscriptionBytes(usage.remaining)}</strong><span>${quotaLabel}</span></div>
      ${progress}
      <div class="subscription-transfer"><span>上传 ${subscriptionBytes(usage.upload)}</span><span>下载 ${subscriptionBytes(usage.download)}</span><span>已用 ${subscriptionBytes(usage.used)}</span></div>
    </section>
    <dl class="subscription-facts"><div><dt>到期时间${usage.expired ? " · 已到期" : ""}</dt><dd class="${usage.expired ? "is-expired" : ""}">${usage.expires}</dd></div><div><dt>当前配置</dt><dd>${summary ? `${summary.nodeCount} 节点 · ${summary.proxyProviderCount} 提供器` : "尚未读取"}</dd></div></dl>
    <div class="subscription-observation"><p class="${status?.lastError ? "is-error" : ""}">${status?.lastError ? `刷新失败 · ${escape(status.lastError)}` : status?.checkedAt ? `最近检查 ${subscriptionDate(status.checkedAt)}` : "尚未检查用量"}</p><p>${usageNote}</p></div>
    <div class="subscription-entry-footer"><span class="subscription-validation">${configuration} · ${mode}模式</span><div class="toolbar">${button("refresh", "刷新")}${button("activate", subscription.active ? "已选用" : "选用", subscription.active ? "disabled" : "")}</div></div>
    <details class="subscription-more"><summary>订阅详情与更多操作<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg></summary><div class="subscription-more-body"><dl><div><dt>用量采样</dt><dd>${sampleAt ? subscriptionDate(sampleAt) : "尚无采样"}</dd></div><div><dt>配置更新</dt><dd>${subscriptionDate(subscription.latestFetchedAt)}</dd></div><div><dt>配置文件</dt><dd>${subscriptionBytes(latestMetadata?.bytes)} · ${subscription.revisionCount} 个版本</dd></div></dl><p>${policyText}${failed ? `：${escape(task?.error ?? "请重试")}` : ""}</p><div class="toolbar">${button(generating ? "openai-cancel" : "openai-generate", generating ? `停止生成${taskProgress}` : profile.openaiPolicy.enabled ? "重新生成灾备" : "OpenAI 灾备", task?.running && !taskForProfile ? "disabled" : "")}${button("versions", "版本")}${button("delete", "删除")}</div></div></details>
  </article>`;
}
