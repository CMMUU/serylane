import type { AppUpdateStatus, UpdateSource } from "./types";

export const updateSourceLabel = (source: UpdateSource | undefined): string =>
  source ? ({ auto: "自动", hk: "下载中心", gitee: "Gitee", github: "GitHub" })[source] : "—";

export const updateStageLabel = (stage?: string) => ({ preflight: "安装预检", stopping: "停止旧进程", services: "准备服务", replacing: "替换应用", restarting: "等待重启", verify: "验证完整性", verified: "验证完成", download: "下载文件" } as Record<string,string>)[stage ?? ""] ?? "准备中";

export function describeAppUpdate(status: AppUpdateStatus) {
  const { phase, info } = status;
  const busy = ["checking", "downloading", "installing"].includes(phase);
  const source = updateSourceLabel(info?.source);
  const labels: Record<AppUpdateStatus["phase"], [string, string, string]> = {
    idle: ["待检查", "尚未检查", "自动模式优先下载中心，同版本 Gitee / GitHub 备用；下载和检查不会停止代理。"],
    checking: ["检查中", "正在检查更新", "正在读取所选渠道的版本和安装包信息…"],
    current: ["版本一致", "已与可用发布渠道同步", "如有渠道暂不可用，会在下方单独列出。"],
    ahead: ["本地领先", "本地版本高于可用渠道", "本地构建尚未在可用渠道发布，或镜像仍在同步。不会自动降级。"],
    available: ["有新版本", `发现新版本 ${info?.latestVersion ?? ""}`, `将从 ${source} 下载并验证更新签名，安装前由你确认。`],
    downloading: ["下载中", `正在从 ${source} 下载并验证更新`, `${status.networkPath ?? "正在确定网络路径"}。代理保持运行；验证通过后才会显示安装按钮。`],
    ready: ["已验证", "更新已就绪，等待你确认", "更新签名和 SHA-256 校验通过。安装会暂时停止代理并重新打开 Serylane，请先结束重要连接。"],
    installing: ["安装中", `正在安装更新 · ${updateStageLabel(status.stage)}`, status.stage === "preflight" ? "正在核对安装条件，尚未停止代理。" : "确认后安全停止旧进程，再替换应用。TUN 等可选服务恢复单独处理。"],
    cancelled: ["已取消", "下载已取消", "未执行安装，代理未改变；可以重新下载。"],
    failed: ["操作失败", "更新暂未完成", status.error ?? "请稍后重试，或切换官方渠道。"],
  };
  const [badge, label, detail] = labels[phase];
  return {
    state: phase, badge, label, detail, busy,
    canOpen: !busy && Boolean(info),
    canDownload: !busy && Boolean(info?.available) && phase !== "ready",
    canInstall: phase === "ready" && Boolean(info?.available),
    progress: status.totalBytes > 0 ? Math.max(0, Math.min(100, status.downloadedBytes / status.totalBytes * 100)) : 0,
  };
}
