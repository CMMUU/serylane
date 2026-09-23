import type { ThemePreference } from "./theme";

export type NetworkMode = "manual" | "system_proxy" | "tun";
export type RoutingMode = "rule" | "global" | "direct";

export type UserRule = { id: string; enabled: boolean; rule: string; note: string };
export type UserRulesState = {
  revision: number;
  rules: UserRule[];
  history: Array<{ id: string; createdAt: string; count: number }>;
  targets: string[];
  warnings: string[];
  routingMode: RoutingMode | null;
};
export type UserRulesValidation = {
  valid: boolean;
  errors: string[];
  warnings: string[];
  normalizedRules: UserRule[];
  preview: string;
};

export type OpenAiNodeScore = {
  name: string;
  latencyMs: number;
  jitterMs: number;
  bandwidthMbps: number | null;
  score: number;
  checkedAt: string;
};

export type RouteSettings = { listenPort: number; mode: "native" | "compatible"; upstream: "chatgpt" | "openai_api"; outboundProxy: string };
export type RouteProbe = { state: "unknown" | "passed" | "http_unverified" | "failed"; checkedAt: number; latencyMs: number | null };
export type RouteDiagnostic = { timestamp: number; category: string; code: string; message: string; target: RouteSettings["upstream"]; stage: string; elapsedMs: number; httpStatus: number | null; attribution: "verified" | "unconfirmed" };
export type RouteSnapshot = {
  revision: number; enabled: boolean; running: boolean; settings: RouteSettings; endpoint: string;
  requests: number; active: number; completed: number; failed: number; lastStatus: number; lastError: string | null;
  lastDiagnostic?: RouteDiagnostic | null;
  codex: { configRevision: string; attached: boolean; hasBackup: boolean; provider: string; endpoint: string | null; warning: string | null; backupPath: string | null };
  stability: { enabled: boolean; running: boolean; eligible: boolean; profileId: string | null; revisionId: string | null; current: string | null; lastSwitch: number | null; message: string;
    selectionTarget?: RouteSettings["upstream"]; lastCheck?: number | null; commonFailure?: boolean;
    nodes: { name: string; probeOk: boolean; successRate: number | null; samples: number; cooldownSeconds: number; recoveryPasses: number; modelCompleted: number; modelInterrupted: number; chatgpt?: RouteProbe; openaiApi?: RouteProbe }[] };
};
export type OpenAiPolicy = {
  stabilityEnabled?: boolean;
  enabled: boolean;
  autoMaintain: boolean;
  maxNodes: number;
  selectedNodes: OpenAiNodeScore[];
  candidateCount: number;
  healthyCount: number;
  lastBenchmarkedAt: string | null;
  benchmarkVersion: number;
};

export type OpenAiTaskPhase =
  | "idle"
  | "preparing"
  | "checking"
  | "bandwidth"
  | "applying"
  | "completed"
  | "failed"
  | "cancelled";

export type OpenAiPolicyTask = {
  running: boolean;
  profileId: string | null;
  phase: OpenAiTaskPhase;
  completed: number;
  total: number;
  message: string;
  startedAt: string | null;
  finishedAt: string | null;
  error: string | null;
  result: OpenAiPolicy | null;
};

export type AppSettings = {
  schemaVersion: number;
  locale: string;
  theme: ThemePreference;
  launchAtLogin: boolean;
  silentStartup: boolean;
  restoreLastSession: boolean;
  showGlobalTraffic: boolean;
  networkMode: NetworkMode;
  mixedPort: number;
  controllerPort: number;
  updateChannel: string;
  autoCheckUpdates: boolean;
  updateSource: UpdateSource;
  autoDownloadUpdates: boolean;
  diagnosticsRetentionDays: number;
  appLogRetentionDays: number;
};

export type GlobalTrafficSnapshot = {
  enabled: boolean;
  uploadBytesPerSecond: number;
  downloadBytesPerSecond: number;
  sampledAt: string | null;
  interfaces: string[];
};

export type RuntimeStatus = {
  state: string;
  phase: string;
  binaryAvailable: boolean;
  binaryPath: string | null;
  version: string | null;
  configPath: string | null;
  message: string;
  pid: number | null;
  startedAt: string | null;
  lastError: string | null;
};

export type StartupStatus = {
  launchRequested: boolean;
  registered: boolean | null;
  systemAllows: boolean | null;
  desiredRunning: boolean;
  message: string;
};

export type SessionResumeStatus = {
  phase: "idle" | "pending" | "restoring" | "waiting_network" | "restored" | "paused";
  message: string;
};

export type RuntimeLog = {
  timestamp: string;
  level: string;
  source: string;
  message: string;
};

export type ApplicationLogSnapshot = {
  entries: Array<{ timestamp: number; level: string; source: string; message: string; count: number }>;
  retentionHours: number;
  maxBytes: number;
  storageError: string | null;
};

export type ProfileSource =
  | { type: "remote_subscription"; host: string; userAgent: string }
  | { type: "local_file"; sourcePath: string }
  | { type: "inline"; label: string };

export type ProfileRecord = {
  schemaVersion: number;
  id: string;
  displayName: string;
  source: ProfileSource;
  routingMode: RoutingMode;
  openaiPolicy: OpenAiPolicy;
  activeRevisionId: string | null;
  lastKnownGoodRevisionId: string | null;
  createdAt: string;
  updatedAt: string;
};

export type SubscriptionUsage = {
  uploadBytes: number | null;
  downloadBytes: number | null;
  totalBytes: number | null;
  /** Provider expiry as Unix seconds; zero does not establish unlimited validity. */
  expiresAt: number | null;
};

export type SubscriptionStatus = {
  requestStartedAt?: string | null;
  checkedAt: string | null;
  lastError: string | null;
  usage: SubscriptionUsage | null;
  usageUpdatedAt: string | null;
};

export type SubscriptionMetadata = {
  contentType: string | null;
  etag: string | null;
  lastModified: string | null;
  bytes: number;
  usage?: SubscriptionUsage | null;
};

export type ValidationReport = {
  valid: boolean;
  warnings: string[];
  errors: string[];
  nativeCoreValidated: boolean;
};

export type ConfigRevision = {
  schemaVersion: number;
  id: string;
  profileId: string;
  sourceSha256: string;
  effectiveSha256: string;
  fetchedAt: string;
  subscription: SubscriptionMetadata | null;
  validation: ValidationReport;
  openaiPolicy: OpenAiPolicy;
};

export type ProfileSummary = {
  format: string;
  nodeCount: number;
  proxyGroupCount: number;
  proxyProviderCount: number;
  ruleCount: number;
  ruleProviderCount: number;
  dnsConfigured: boolean;
  tunConfigured: boolean;
  nodeProtocols: string[];
  proxyGroupTypes: string[];
  unsupportedGroupTypes: string[];
  warnings: string[];
};

export type ProfileOperationResult = {
  profile: ProfileRecord;
  revision: ConfigRevision;
  summary: ProfileSummary;
  updated: boolean;
};

export type SubscriptionImportResult = ProfileOperationResult & {
  created: boolean;
  activated: boolean;
  openAiGeneration: "not_requested" | "started" | "failed";
  openAiError: string | null;
  observationError: string | null;
};

export type ProfileDetails = {
  profile: ProfileRecord;
  revisions: ConfigRevision[];
  summary: ProfileSummary | null;
};

export type SubscriptionOverview = {
  profile: ProfileRecord;
  summary: ProfileSummary | null;
  revisionCount: number;
  latestFetchedAt: string | null;
  latestMetadata: SubscriptionMetadata | null;
  latestValidation: ValidationReport | null;
  active: boolean;
  status?: SubscriptionStatus | null;
};

export type NodeDelaySample = {
  time: string | null;
  delayMs: number;
};

export type CurrentNodeDetails = {
  group: string;
  nodeName: string;
  routeChain: string[];
  nodeType: string;
  alive: boolean | null;
  udp: boolean | null;
  uot: boolean | null;
  xudp: boolean | null;
  tfo: boolean | null;
  mptcp: boolean | null;
  smux: boolean | null;
  providerName: string | null;
  maskedServer: string | null;
  port: number | null;
  network: string | null;
  tls: string | null;
  dialerProxy: string | null;
  interface: string | null;
  history: NodeDelaySample[];
  lastDelayMs: number | null;
};

export type NetworkSafetyCheck = {
  target: string;
  url: string;
  success: boolean;
  expectedStatus: number;
  failureKind?: "timeout" | "certificate" | "tls" | "dns" | "tunnel" | "connect" | "other" | null;
  actualStatus: number | null;
  latencyMs: number;
  detail: string;
};

export type NetworkSafetyReport = {
  success: boolean;
  proxyEndpoint: string;
  checks: NetworkSafetyCheck[];
  warnings: string[];
};

export type BinaryInfo = {
  available: boolean;
  path: string | null;
  version: string | null;
  message: string;
};

export type SystemProxyStatus = {
  active: boolean;
  snapshotPath: string | null;
  platform: string;
};

export type ProxyCompatibility = {
  supported: boolean;
  systemConfigured: boolean;
  compatible: boolean;
  expectedProxy: string;
  resolvedHttp: string | null;
  resolvedHttps: string | null;
  detail: string;
};

export type ProgramProxyMode = "environment" | "chromium";
export type AppBinding =
  | { packageFamilyName: string; applicationId: string }
  | { kind: "macos"; bundleId: string; location: string; requirement: string | null }
  | { kind: "linux"; desktopId: string; location: string };
export type AppAvailability = "ready" | "not_installed" | "updating" | "maintenance" | "needs_relink" | "missing_file" | "unsupported_launch" | "read_error";
export type InstalledApplication = {
  binding: AppBinding; name: string; version: string; packageFullName: string;
  packageRoot: string; executable: string; availability: AppAvailability; detail: string;
};
export type ApplicationResolution = { availability: AppAvailability; detail: string; application: InstalledApplication | null };
export type InstalledApplications = { applications: InstalledApplication[]; warnings: string[]; refreshing?: boolean; checkedAt?: number | null };
export type ProxyProgram = {
  id: string;
  name: string;
  executable: string;
  binding?: AppBinding | null;
  workingDirectoryRelative?: string | null;
  resolution?: ApplicationResolution;
  arguments: string[];
  workingDirectory: string | null;
  mode: ProgramProxyMode;
  available: boolean;
  runningPid: number | null;
  launchPending: boolean;
};
export type ProgramInput = Omit<ProxyProgram, "id" | "available" | "runningPid" | "launchPending" | "resolution"> & { id: string | null };
export type ProgramState = {
  platform: string;
  revision: number;
  supported: boolean;
  proxyEndpoint: string;
  coreRunning: boolean;
  programs: ProxyProgram[];
};

export type TunHelperState =
  | "unsupported"
  | "not_installed"
  | "requires_approval"
  | "ready"
  | "checking"
  | "outdated"
  | "unreachable";

export type TunHelperStatus = {
  supported: boolean;
  state: TunHelperState;
  message: string;
  protocolVersion: number;
  runtimeRunning: boolean;
  runtimePid: number | null;
  runtimeVersion: string | null;
  lastError: string | null;
};

export type AppInfo = {
  productName: string;
  version: string;
  targetOs: string;
  targetArch: string;
};

export type UpdateSource = "auto" | "github" | "gitee";
export type AppUpdateInfo = {
  currentVersion: string;
  latestVersion: string;
  available: boolean;
  ahead: boolean;
  notes: string;
  publishedAt: string | null;
  releaseUrl: string;
  source: Exclude<UpdateSource, "auto">;
  channels: Array<{ source: Exclude<UpdateSource, "auto">; version: string | null; error: string | null }>;
};

export type AppUpdateStatus = {
  phase: "idle" | "checking" | "current" | "ahead" | "available" | "downloading" | "ready" | "installing" | "cancelled" | "failed";
  info: AppUpdateInfo | null;
  downloadedBytes: number;
  totalBytes: number;
  error: string | null;
};

export type AppError = {
  code?: string;
  stage?: string;
  message?: string;
  retryable?: boolean;
};

export type UserMessage = { title: string; description: string; action: string; details: string };
export type ConnectionFeedback = {
  revision: number; operation: number;
  phase: "idle" | "validating" | "starting" | "applying" | "enabled" | "failed";
  mode: NetworkMode; health: "unchecked" | "checking" | "healthy" | "partial" | "unavailable";
  retrying: boolean; elapsedMs: number; issue: UserMessage | null;
  checks: NetworkSafetyReport["checks"];
};
