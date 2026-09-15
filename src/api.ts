import { invoke } from "@tauri-apps/api/core";
import type { CostSnapshot } from "./openai-costs";
import type { ThemePreference } from "./theme";
import type {
  RouteSettings,
  RouteSnapshot,
  AppInfo,
  AppSettings,
  AppUpdateStatus,
  UpdateSource,
  BinaryInfo,
  ConfigRevision,
  CurrentNodeDetails,
  GlobalTrafficSnapshot,
  NetworkMode,
  NetworkSafetyReport,
  OpenAiPolicy,
  OpenAiPolicyTask,
  ProfileDetails,
  ProfileOperationResult,
  ProfileRecord,
  ProfileSummary,
  ProgramInput,
  ProgramState,
  ProxyCompatibility,
  RoutingMode,
  RuntimeLog,
  ApplicationLogSnapshot,
  RuntimeStatus,
  SessionResumeStatus,
  StartupStatus,
  SystemProxyStatus,
  SubscriptionOverview,
  SubscriptionImportResult,
  TunHelperStatus,
  UserRule,
  UserRulesState,
  UserRulesValidation,
} from "./types";

export const api = {
  localRouteStatus: () => invoke<RouteSnapshot>("local_route_status"),
  saveLocalRoute: (settings: RouteSettings, expectedRevision: number) => invoke<RouteSnapshot>("save_local_route", { settings, expectedRevision }),
  setLocalRouteEnabled: (enabled: boolean, expectedRevision: number, confirmed: boolean) => invoke<RouteSnapshot>("set_local_route_enabled", { enabled, expectedRevision, confirmed }),
  setCodexRoute: (attach: boolean, expectedConfigRevision: string, confirmed: boolean) => invoke<RouteSnapshot>("set_codex_route", { attach, expectedConfigRevision, confirmed }),
  setOpenAiStability: (enabled: boolean, profileId: string, revisionId: string, confirmed: boolean) => invoke<void>("set_openai_stability", { enabled, profileId, revisionId, confirmed }),
  appInfo: () => invoke<AppInfo>("app_info"),
  checkAppUpdate: () => invoke<AppUpdateStatus>("check_app_update"),
  appUpdateStatus: () => invoke<AppUpdateStatus>("app_update_status"),
  downloadAppUpdate: (versionTag: string) => invoke<AppUpdateStatus>("download_app_update", { versionTag }),
  cancelAppUpdate: () => invoke<void>("cancel_app_update"),
  installAppUpdate: (versionTag: string, confirmed: boolean) => invoke<void>("install_app_update", { versionTag, confirmed }),
  saveUpdatePreferences: (source: UpdateSource, autoCheck: boolean, autoDownload: boolean) =>
    invoke<AppSettings>("save_update_preferences", { source, autoCheck, autoDownload }),
  openOfficialRelease: (source: UpdateSource, versionTag: string) =>
    invoke<void>("open_official_release", { source, versionTag }),
  settings: () => invoke<AppSettings>("get_settings"),
  updateSettings: (settings: AppSettings) =>
    invoke<AppSettings>("update_settings", { settings }),
  setTheme: (theme: ThemePreference) =>
    invoke<AppSettings>("set_app_theme", { theme }),
  globalTraffic: () =>
    invoke<GlobalTrafficSnapshot>("global_traffic_snapshot"),
  inspect: (source: string) =>
    invoke<ProfileSummary>("inspect_mihomo_yaml", { source }),
  profiles: () => invoke<ProfileRecord[]>("list_profiles"),
  subscriptions: () => invoke<SubscriptionOverview[]>("list_subscriptions"),
  profileDetails: (profileId: string) =>
    invoke<ProfileDetails>("get_profile_details", { profileId }),
  activeProfile: () => invoke<ProfileDetails | null>("get_active_profile"),
  createInlineProfile: (displayName: string, source: string) =>
    invoke<ProfileOperationResult>("create_inline_profile", { displayName, source }),
  createSubscriptionProfile: (
    displayName: string,
    url: string,
    userAgent: string,
    generateOpenAi = false,
    activateAfterImport = false,
  ) =>
    invoke<SubscriptionImportResult>("create_subscription_profile", {
      displayName,
      url,
      userAgent,
      generateOpenAi,
      activateAfterImport,
    }),
  refreshProfile: (profileId: string) =>
    invoke<ProfileOperationResult>("refresh_profile", { profileId }),
  activateProfile: (profileId: string, revisionId?: string | null) =>
    invoke<ProfileDetails>("activate_profile", {
      profileId,
      revisionId: revisionId ?? null,
    }),
  rollbackProfile: (profileId: string) =>
    invoke<ProfileDetails>("rollback_profile", { profileId }),
  deleteProfile: (profileId: string) =>
    invoke<void>("delete_profile", { profileId }),
  binary: () => invoke<BinaryInfo>("probe_mihomo"),
  runtime: () => invoke<RuntimeStatus>("runtime_status"),
  sessionResume: () => invoke<SessionResumeStatus>("get_session_resume_status"),
  startupStatus: () => invoke<StartupStatus>("get_startup_status"),
  startActive: () => invoke<RuntimeStatus>("start_active_profile"),
  stop: () => invoke<RuntimeStatus>("stop_mihomo"),
  logs: (limit = 300) => invoke<RuntimeLog[]>("runtime_logs", { limit }),
  applicationLogs: () => invoke<ApplicationLogSnapshot>("application_logs"),
  clearApplicationLogs: () => invoke<void>("clear_application_logs"),
  clearLogs: () => invoke<void>("clear_runtime_logs"),
  systemProxy: () => invoke<SystemProxyStatus>("system_proxy_status"),
  systemProxyCompatibility: () => invoke<ProxyCompatibility>("check_system_proxy_compatibility"),
  proxyPrograms: () => invoke<ProgramState>("list_proxy_programs"),
  saveProxyProgram: (input: ProgramInput, expectedRevision: number) =>
    invoke<ProgramState>("save_proxy_program", { input, expectedRevision }),
  deleteProxyProgram: (programId: string, expectedRevision: number) =>
    invoke<ProgramState>("delete_proxy_program", { programId, expectedRevision }),
  launchProxyProgram: (programId: string, expectedRevision: number) =>
    invoke<ProgramState>("launch_proxy_program", { programId, expectedRevision }),
  chooseProxyProgram: () => invoke<string | null>("choose_proxy_program"),
  tunHelperStatus: () => invoke<TunHelperStatus>("tun_helper_status"),
  installTunHelper: () => invoke<TunHelperStatus>("install_tun_helper"),
  repairTunHelper: () => invoke<TunHelperStatus>("repair_tun_helper"),
  uninstallTunHelper: () => invoke<void>("uninstall_tun_helper"),
  openTunHelperSettings: () => invoke<void>("open_tun_helper_settings"),
  prepareTun: () => invoke<void>("prepare_tun_active_profile"),
  setNetworkMode: (mode: NetworkMode) =>
    invoke<AppSettings>("set_network_mode", { mode }),
  setProfileRoutingMode: (profileId: string, mode: RoutingMode) =>
    invoke<ProfileDetails>("set_profile_routing_mode", { profileId, mode }),
  proxies: () => invoke<Record<string, unknown>>("get_proxies"),
  currentNodeDetails: (group: string) =>
    invoke<CurrentNodeDetails>("get_current_node_details", { group }),
  rules: () => invoke<Record<string, unknown>>("get_rules"),
  userRules: () => invoke<UserRulesState>("get_user_rules"),
  validateUserRules: (rules: UserRule[]) =>
    invoke<UserRulesValidation>("validate_user_rules", { rules }),
  saveUserRules: (rules: UserRule[], expectedRevision: number) =>
    invoke<UserRulesState>("save_user_rules", { rules, expectedRevision }),
  rollbackUserRules: (revisionId: string, expectedRevision: number) =>
    invoke<UserRulesState>("rollback_user_rules", { revisionId, expectedRevision }),
  parseUserRulesText: (text: string) =>
    invoke<UserRule[]>("parse_user_rules_text", { text }),
  connections: () => invoke<Record<string, unknown>>("get_connections"),
  selectProxy: (group: string, proxy: string, profileId: string, revisionId: string) =>
    invoke<void>("select_proxy", { group, proxy, profileId, revisionId }),
  openAiCosts: (profileId: string) => invoke<CostSnapshot>("get_openai_costs", { profileId }),
  saveOpenAiCosts: (input: CostSnapshot) => invoke<CostSnapshot>("save_openai_costs", { input, confirmed: true }),
  clearProxySelection: (group: string, profileId: string, revisionId: string) =>
    invoke<void>("clear_proxy_selection", { group, profileId, revisionId }),
  testProxyDelay: (proxy: string, url?: string, timeoutMs = 5_000) =>
    invoke<Record<string, unknown>>("test_proxy_delay", {
      proxy,
      url: url ?? null,
      timeoutMs,
    }),
  testProxyGroup: (
    group: string,
    url?: string,
    expectedStatus?: string,
    timeoutMs = 8_000,
  ) =>
    invoke<Record<string, number>>("test_proxy_group", {
      group,
      url: url ?? null,
      expectedStatus: expectedStatus ?? null,
      timeoutMs,
    }),
  startOpenAiPolicyGeneration: (profileId: string, autoMaintain = true) =>
    invoke<OpenAiPolicyTask>("start_openai_policy_generation", {
      profileId,
      autoMaintain,
    }),
  openAiPolicyTask: () =>
    invoke<OpenAiPolicyTask>("get_openai_policy_task"),
  cancelOpenAiPolicyGeneration: () =>
    invoke<OpenAiPolicyTask>("cancel_openai_policy_generation"),
  disableOpenAiPolicy: (profileId: string) =>
    invoke<OpenAiPolicy>("disable_openai_policy", { profileId }),
  closeConnection: (connectionId: string) =>
    invoke<void>("close_connection", { connectionId }),
  diagnostics: () =>
    invoke<Array<{ stage: string; success: boolean; latencyMs: number | null; detail: string }>>(
      "run_connectivity_diagnostics",
    ),
  networkSafety: () =>
    invoke<NetworkSafetyReport>("run_network_safety_check"),
};

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object") {
    const value = error as { message?: unknown; code?: unknown };
    const message = typeof value.message === "string" ? value.message : JSON.stringify(error);
    return typeof value.code === "string" ? `${value.code}: ${message}` : message;
  }
  return String(error);
}

export function revisionLabel(revision: ConfigRevision): string {
  return new Date(revision.fetchedAt).toLocaleString();
}
