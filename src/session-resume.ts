import type { AppSettings, SessionResumeStatus, StartupStatus } from "./types";

export type StartupMode = "manual" | "background" | "window" | "custom";
type StartupPreferences = Pick<AppSettings, "launchAtLogin" | "silentStartup" | "restoreLastSession">;

export function startupModeFromSettings(settings: StartupPreferences): StartupMode {
  if (!settings.launchAtLogin) return "manual";
  if (!settings.restoreLastSession) return "custom";
  return settings.silentStartup ? "background" : "window";
}

export function startupModeSettings<T extends StartupPreferences>(settings: T, mode: StartupMode): T {
  if (mode === "manual") return { ...settings, launchAtLogin: false };
  if (mode === "background" || mode === "window") {
    return { ...settings, launchAtLogin: true, silentStartup: mode === "background", restoreLastSession: true };
  }
  if (mode === "custom") return { ...settings };
  throw new Error("启动模式无效");
}

export function startupModeHelp(settings: StartupPreferences, mode: StartupMode): string {
  const selected = startupModeSettings(settings, mode);
  const prefix = mode === "custom" ? "保留旧版自定义设置；选择其他模式并保存后才会替换。" : "";
  return prefix + sessionResumeHelp(selected.launchAtLogin, selected.restoreLastSession, selected.silentStartup);
}

export function startupRegistrationPresentation(status: StartupStatus | null) {
  if (!status) return { issue: false, text: "登录项状态尚未读取；核对状态不会修改系统设置。" };
  const issue = status.state === "needs_repair" || status.registered === null
    || status.registered !== status.launchRequested
    || (status.launchRequested && status.systemAllows === false);
  const remembered = status.desiredRunning ? "已开启，恢复时沿用保存的模式与配置" : "已关闭，下次保持停止";
  const label = ({ disabled: "未开启", requires_approval: "待系统允许", registered: "已登记", needs_repair: "需要修复", unknown: "待核对" } as const)[status.state];
  return { issue, text: (label ? label + "：" : "") + status.message + " 记忆的代理状态：" + remembered + "。" };
}

export function sessionResumePresentation(status: SessionResumeStatus | null) {
  const phase = status?.phase ?? "idle";
  const busy = phase === "pending" || phase === "restoring" || phase === "waiting_network";
  return {
    busy,
    visible: busy || phase === "paused",
    title: phase === "paused" ? "上次运行状态暂未恢复" : phase === "waiting_network" ? "等待网络恢复" : "正在恢复上次运行状态",
    message: status?.message ?? "尚未读取恢复状态。",
  };
}

export function canStopSession(phase: string | undefined, resume: SessionResumeStatus | null, startup: StartupStatus | null) {
  return phase === "running" || sessionResumePresentation(resume).busy
    || resume?.phase === "paused" || startup?.desiredRunning === true;
}

export function sessionResumeHelp(launchAtLogin: boolean, restoreLastSession: boolean, silentStartup = false) {
  if (!restoreLastSession) return "恢复已关闭：打开应用时不会自动启动代理核心。";
  const entry = launchAtLogin
    ? (silentStartup ? "登录后在托盘后台恢复" : "登录后自动打开应用并恢复")
    : "未开启登录时启动：重启电脑后，手动打开应用才会恢复";
  return entry + "上次运行状态。上次已停止则保持停止；运行中则使用原模式与选用配置。临时网络故障时等待重试，点击停止可取消；TUN 服务需要处理或其他系统代理占用时暂停恢复，不会自动接入 Codex 或启动其他程序。";
}
