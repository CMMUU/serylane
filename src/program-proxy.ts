import type { AppBinding, InstalledApplication, InstalledApplications, ProgramInput, ProgramProxyMode, ProgramState, ProxyCompatibility, ProxyProgram } from "./types";

export function parseProgramArguments(text: string): string[] {
  const args = text.split(/\r?\n/).filter((line) => line.trim().length > 0);
  if (args.some((arg) => /[\0\r]/.test(arg)) || args.length > 64 ||
      args.reduce((size, arg) => size + new TextEncoder().encode(arg).length, 0) > 8192) {
    throw new Error("启动参数最多 64 项、总长 8192 字节，不能包含空字符。");
  }
  return args;
}

export function suggestedProgramName(path: string): string {
  return (path.split(/[\\/]/).pop() ?? "").replace(/\.(exe|app|desktop)$/i, "");
}

export function launchBlockReason(program: ProxyProgram, state: ProgramState): string | null {
  if (!state.supported) return "此平台暂不支持程序代理启动";
  if (program.launchPending) return "启动结果待确认；请稍后刷新查看";
  if (!program.available || (program.resolution && program.resolution.availability !== "ready")) return program.resolution?.detail ?? "找不到程序文件，请编辑路径";
  if (program.runningPid !== null) return "已由 Serylane 启动，请先自行退出程序";
  if (!state.coreRunning) return "请先启动 Mihomo 核心";
  return null;
}

export function programRunStatus(program: ProxyProgram): string {
  if (program.launchPending) return "启动结果待确认；请稍后刷新查看";
  if (!program.available) return program.resolution?.detail ?? "文件不存在";
  if (program.runningPid !== null && program.runningPid > 0) return `已发起启动 · PID ${program.runningPid}`;
  return "尚未由本次会话启动；启动前会检查已有后台实例。";
}

export function applicationIdentity(binding: AppBinding): { source: string; identity: string; location: string } {
  if ("kind" in binding && binding.kind === "macos") return { source: "macOS 应用", identity: binding.bundleId, location: binding.location };
  if ("kind" in binding && binding.kind === "linux") return { source: "Linux 桌面应用", identity: binding.desktopId, location: binding.location };
  if ("packageFamilyName" in binding) return { source: "Windows MSIX / Store", identity: `${binding.packageFamilyName}!${binding.applicationId}`, location: "" };
  return { source: "应用", identity: "", location: "" };
}

export function isMacApplication(binding: AppBinding | null): boolean {
  return binding !== null && "kind" in binding && binding.kind === "macos";
}

export function allowsRelativeDirectory(binding: AppBinding | null): boolean {
  return binding !== null && "packageFamilyName" in binding;
}

export function requiresApplicationInspection(path: string, platform: string): boolean {
  return (platform === "macos" && /\.app\/?$/i.test(path)) || (platform === "linux" && /\.desktop$/.test(path));
}

const escape = (value: string) => value.replace(/[&<>"']/g, (char) =>
  ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);

export const programManagerMarkup = `
  <article class="panel program-intro">
    <div class="panel-heading">
      <div><div class="section-label">PROGRAM PROXY</div><h2>程序代理</h2></div>
      <span class="control-state-pill">按需启动</span>
    </div>
    <p class="program-lead">让指定程序单独使用 Serylane 的本地代理。</p>
    <p class="hint">仅影响从这里启动、且支持所选代理方式的程序及其子进程。不强制接管已运行的程序，不修改全局环境变量，也不会自动切换系统代理或 TUN。</p>
    <div class="program-connection"><span class="program-core-dot" id="program-core-dot" aria-hidden="true"></span><strong id="program-core-status">正在读取核心状态</strong><code id="program-endpoint">—</code></div>
  </article>
  <div class="program-layout">
    <article class="panel program-editor">
      <div class="panel-heading"><div><div class="section-label">PROGRAM DETAILS</div><h2 id="program-editor-title">添加程序</h2></div></div>
      <form id="program-form">
        <fieldset id="program-fields" disabled>
          <div class="toolbar program-add-sources"><button id="program-installed" type="button" class="button button-primary">从已安装应用选择</button><button id="program-browse" type="button" class="button button-quiet">手动选择文件 / 应用</button></div>
          <p class="hint" id="program-source-hint">按当前平台读取已安装应用，也可手动选择。关联身份后在启动前重新定位；不会下载或安装更新。</p>
          <section id="program-app-picker" class="program-app-picker is-hidden" aria-label="选择已安装应用">
            <label for="program-app-search">搜索已安装应用</label><input id="program-app-search" type="search" placeholder="应用名称、身份或安装位置" autocomplete="off" />
            <div class="toolbar program-picker-actions"><button id="program-app-refresh" type="button" class="button button-quiet">重新扫描</button><button id="program-app-close" type="button" class="button button-quiet">收起列表</button></div>
            <p id="program-app-feedback" class="hint" role="status" aria-live="polite"></p><div id="program-app-results" class="program-app-results"></div>
          </section>
          <p id="program-binding-info" class="hint" role="status">普通文件关联</p>
          <label for="program-name">程序名称</label>
          <input id="program-name" required maxlength="128" placeholder="例如：开发工具" autocomplete="off" />
          <label for="program-executable">程序文件</label>
          <div class="program-path-picker"><input id="program-executable" required placeholder="C:\\…\\app.exe" autocomplete="off" spellcheck="false" /></div>
          <p class="hint" id="program-path-hint">应用关联时显示当前入口，启动前会重新定位；普通文件按保存的路径启动。</p>
          <label for="program-mode">代理方式</label>
          <select id="program-mode" aria-describedby="program-mode-hint"><option value="environment">环境变量 · 支持代理的应用</option><option value="chromium">Chromium / Electron · 显式代理参数</option></select>
          <p class="hint" id="program-mode-hint">仅给新进程设置 HTTP(S)_PROXY 等变量；不读取这些变量的应用不会因此走代理。</p>
          <details class="program-advanced">
            <summary>启动参数与工作目录</summary>
            <label for="program-arguments">启动参数 <span class="muted">（可选）</span></label>
            <textarea id="program-arguments" rows="3" placeholder="每行一个参数，无需额外包引号" spellcheck="false" aria-describedby="program-args-hint"></textarea>
            <p class="hint" id="program-args-hint">空行忽略，含空格的整行视为一个参数，不作为 Shell 命令执行。参数明文保存，请勿填写密码或令牌。</p>
            <div id="program-directory-fields"><label for="program-directory">工作目录 <span class="muted">（可选）</span></label>
            <select id="program-directory-kind" aria-label="工作目录方式"><option value="application">跟随应用入口目录（默认）</option><option value="custom">自定义绝对路径</option><option value="relative">包内相对目录（随更新）</option></select>
            <input id="program-directory" placeholder="留空时使用程序所在目录" autocomplete="off" spellcheck="false" /></div>
            <p class="hint" id="program-directory-note"></p><button id="program-directory-clear" type="button" class="button button-quiet is-hidden">使用默认工作目录</button>
          </details>
          <div class="toolbar program-form-actions"><button class="button button-primary" type="submit" id="program-save">添加到清单</button><button class="button button-quiet" type="button" id="program-reset">清空</button></div>
        </fieldset>
      </form>
      <p class="hint">保存不会启动程序。删除条目不会卸载软件或关闭进程。</p>
    </article>
    <article class="panel program-library">
      <div class="panel-heading"><div><div class="section-label">YOUR PROGRAMS</div><h2>我的程序 <span class="program-count" id="program-count">0</span></h2></div><button id="program-refresh" type="button" class="button button-quiet">刷新清单</button></div>
      <p class="program-feedback" id="program-feedback" role="status" aria-live="polite">正在读取程序清单…</p>
      <div class="program-list" id="program-list"></div>
      <p class="hint program-library-note">使用期间请保持核心运行；修改本地端口后需重新代理启动。运行状态不代表联网验证成功。已有后台实例时，请先自行退出；macOS 关闭窗口可能仍在后台，请使用 ⌘Q 退出应用。</p>
    </article>
  </div>`;

type ProgramServices = {
  api: {
    proxyPrograms(refresh?: boolean): Promise<ProgramState>;
    installedProxyApplications(refresh?: boolean): Promise<InstalledApplications>;
    inspectProxyApplication(path: string): Promise<InstalledApplication>;
    saveProxyProgram(input: ProgramInput, expectedRevision: number): Promise<ProgramState>;
    deleteProxyProgram(programId: string, expectedRevision: number): Promise<ProgramState>;
    launchProxyProgram(programId: string, expectedRevision: number): Promise<ProgramState>;
    chooseProxyProgram(): Promise<string | null>;
  };
  confirm(options: { title: string; message: string; confirmLabel?: string; returnFocus?: HTMLElement | null }): Promise<boolean>;
  error(error: unknown): string;
};

export function mountProgramManager(root: HTMLElement, services: ProgramServices) {
  const $ = <T extends HTMLElement>(selector: string) => root.querySelector<T>(selector)!;
  const field = (name: string) => $<HTMLInputElement | HTMLTextAreaElement>(`#program-${name}`);
  const form = $<HTMLFormElement>("#program-form");
  let state: ProgramState | null = null;
  let editing: string | null = null;
  let draftRevision: number | null = null;
  let dirty = false;
  let busy = false;
  let binding: AppBinding | null = null;
  let selectedApplication: InstalledApplication | null = null;
  let applications: InstalledApplication[] = [];
  let catalogGeneration = 0;
  let catalogTimer: ReturnType<typeof setTimeout> | null = null;
  let catalogRefreshing = false;
  let disposed = false;

  function stopCatalog() {
    catalogGeneration++;
    if (catalogTimer !== null) clearTimeout(catalogTimer);
    catalogTimer = null;
    catalogRefreshing = false;
    $<HTMLButtonElement>("#program-app-refresh").disabled = false;
  }

  function pickerVisible() {
    return !disposed && root.isConnected && !root.classList.contains("is-hidden") && !$("#program-app-picker").classList.contains("is-hidden");
  }

  function closeApplications() {
    stopCatalog();
    $("#program-app-picker").classList.add("is-hidden");
  }

  function feedback(message: string, error = false) {
    $("#program-feedback").textContent = message;
    $("#program-feedback").dataset.error = String(error);
  }

  function render() {
    root.setAttribute("aria-busy", String(busy));
    $<HTMLFieldSetElement>("#program-fields").disabled = busy || !state?.supported;
    $<HTMLButtonElement>("#program-refresh").disabled = busy;
    $("#program-count").textContent = String(state?.programs.length ?? 0);
    $("#program-endpoint").textContent = state?.proxyEndpoint ?? "—";
    $("#program-core-status").textContent = !state ? "尚未读取状态" : !state.supported ? "此平台暂不支持程序代理启动" : state.coreRunning ? "本地核心已运行" : "请先启动本地核心";
    $("#program-core-dot").classList.toggle("is-running", state?.coreRunning === true);
    $("#program-editor-title").textContent = editing ? "编辑程序" : "添加程序";
    $("#program-save").textContent = editing ? "保存修改" : "添加到清单";
    $("#program-reset").textContent = editing ? "取消编辑" : "清空";
    $<HTMLInputElement>("#program-executable").readOnly = binding !== null;
    $<HTMLInputElement>("#program-executable").required = binding === null;
    const platform = state?.platform ?? "windows";
    const mac = isMacApplication(binding);
    $("#program-source-hint").textContent = platform === "macos"
      ? "选择已安装应用或整个 .app。按应用身份与选定副本关联，更新后重新定位；不会下载或安装更新。"
      : platform === "linux" ? "选择桌面应用入口或可执行文件。.desktop 入口保留原有启动参数；特殊激活方式会单独说明。"
      : "已安装应用支持 MSIX / Store 身份关联，更新后自动跟随。普通或便携软件可手动选择 .exe。";
    field("executable").placeholder = platform === "macos" ? "/Applications/Example.app" : platform === "linux" ? "/usr/bin/example 或 /…/example.desktop" : "C:\\…\\app.exe";
    $("#program-path-hint").textContent = mac ? "通过 macOS 原生接口启动选定的 .app，不需要进入应用内部选择可执行文件。" : "应用关联时显示当前入口，启动前会重新定位；普通文件按保存的路径启动。";
    $("#program-binding-info").textContent = binding ? `${applicationIdentity(binding).source} · ${selectedApplication?.name ?? "已关联应用"} · ${selectedApplication?.version ? `当前版本 ${selectedApplication.version} · ` : ""}身份关联 · 启动前重新定位${selectedApplication && selectedApplication.availability !== "ready" ? ` · ${selectedApplication.detail}` : ""}` : "普通文件关联 · 保留手动选择路径";
    const directoryKind = $<HTMLSelectElement>("#program-directory-kind");
    const incompatibleDirectory = (mac && directoryKind.value !== "application") || (directoryKind.value === "relative" && !allowsRelativeDirectory(binding));
    directoryKind.querySelector<HTMLOptionElement>('option[value="relative"]')!.disabled = !allowsRelativeDirectory(binding);
    directoryKind.disabled = busy || mac;
    $("#program-directory-fields").classList.toggle("is-hidden", mac);
    field("directory").disabled = busy || mac || directoryKind.value === "application";
    field("directory").placeholder = directoryKind.value === "relative" ? "例如 app\\work；. 表示应用包目录" : platform === "windows" ? "例如 D:\\Projects" : "例如 /home/user/project";
    $("#program-directory-note").textContent = incompatibleDirectory
      ? "原工作目录设置已保留，但与所选应用不兼容。请明确切换为默认工作目录后保存，或选择原应用。"
      : mac ? "macOS .app 使用系统原生工作目录，不设置自定义目录。关闭窗口后仍在后台时，请先用 ⌘Q 退出再启动。"
      : binding && "kind" in binding && binding.kind === "linux" ? "默认保留桌面入口的参数与工作目录；上方附加参数独立保存，不覆盖入口自身的参数。" : "留空遵循程序入口的默认目录；包内相对目录仅适用于 Windows 应用包。";
    $("#program-directory-clear").classList.toggle("is-hidden", !incompatibleDirectory);
    $("#program-list").innerHTML = !state ? "" : state.programs.length === 0
      ? `<div class="program-empty"><span aria-hidden="true">＋</span><strong>还没有添加程序</strong><p>填写程序信息，保存后即可按需启动。</p></div>`
      : state.programs.map((program) => {
        const blocked = launchBlockReason(program, state!);
        const status = programRunStatus(program);
        const installed = program.resolution?.application;
        const association = program.binding ? `${applicationIdentity(program.binding).source} · ${installed?.version ? `当前版本 ${escape(installed.version)} · ` : ""}身份关联 · 启动前重新定位` : "普通文件关联";
        return `<section class="program-card" data-program-id="${escape(program.id)}" aria-label="${escape(program.name)}">
          <div class="program-card-heading"><span class="program-icon" aria-hidden="true">${escape(program.name.slice(0, 1).toUpperCase())}</span><div><h3>${escape(program.name)}</h3><span class="program-mode-badge">${program.mode === "chromium" ? "Chromium / Electron" : "环境变量"}</span></div></div>
          <p class="program-association">${association}</p>
          <details class="program-path-details"><summary>查看关联与路径详情</summary><p class="program-exe">${escape(program.executable || "当前入口待解析")}</p>${program.binding ? `<p class="program-exe">${escape(applicationIdentity(program.binding).identity)}${applicationIdentity(program.binding).location ? `<br />${escape(applicationIdentity(program.binding).location)}` : ""}</p>` : ""}</details>
          <p class="program-run-status" data-missing="${!program.available}">${escape(status)}</p>
          <div class="program-card-actions"><button type="button" class="button button-primary" data-program-action="launch" ${busy || blocked ? "disabled" : ""} title="${escape(blocked ?? "确认后为新进程配置代理并启动")}">代理启动</button><button type="button" class="button button-quiet" data-program-action="edit" ${busy ? "disabled" : ""}>编辑</button><button type="button" class="button button-quiet" data-program-action="associate" ${busy ? "disabled" : ""}>重新关联</button><button type="button" class="button button-danger" data-program-action="delete" ${busy ? "disabled" : ""}>删除</button></div>
        </section>`;
      }).join("");
  }

  function updateModeHint() {
    $("#program-mode-hint").textContent = $<HTMLSelectElement>("#program-mode").value === "chromium"
      ? "为 Chromium / Electron 增加显式 HTTP 代理参数并禁用 QUIC，同时设置子进程代理变量。其他网络库仍需支持代理；不做强制拦截。"
      : "仅给新进程设置 HTTP(S)_PROXY 等变量；不读取这些变量的应用不会因此走代理。";
  }

  function reset() {
    form.reset();
    binding = null; selectedApplication = null;
    closeApplications();
    $<HTMLDetailsElement>(".program-advanced").open = false;
    editing = null;
    dirty = false;
    draftRevision = state?.revision ?? null;
    updateModeHint();
    render();
  }

  async function operation(work: () => Promise<void>) {
    if (busy) return;
    busy = true;
    render();
    try { await work(); } catch (error) { feedback(services.error(error), true); }
    finally { busy = false; render(); }
  }

  async function refresh(explicit = false) {
    await operation(async () => {
      const next = await services.api.proxyPrograms(explicit);
      state = next;
      const hasDraft = dirty || editing !== null;
      if (!hasDraft) draftRevision = next.revision;
      if (hasDraft && draftRevision !== next.revision) {
        if (explicit && await services.confirm({ title: "保留草稿并更新清单版本？", message: "清单在编辑期间发生了变化。继续会保留表单草稿，并允许你基于最新清单再次保存；保存时会覆盖此条目的已保存字段。", confirmLabel: "保留草稿继续" })) draftRevision = next.revision;
        else { feedback("清单已变化，编辑内容仍保留。点击刷新清单确认后重试，或取消编辑重新选择条目。", true); return; }
      }
      if (next.supported && pickerVisible() && !catalogRefreshing) showApplications(false);
      feedback(next.supported ? "清单已同步。启动前会重新定位应用，并检查核心和已有实例；自动跟随不下载或安装更新。" : "此平台暂不支持程序代理启动。", !next.supported);
    });
  }

  function renderApplications() {
    const query = $<HTMLInputElement>("#program-app-search").value.trim().toLowerCase();
    const filtered = applications.filter((app) => {
      const identity = applicationIdentity(app.binding);
      return `${app.name} ${identity.identity} ${identity.location}`.toLowerCase().includes(query);
    });
    const markup = filtered.length ? filtered.map((app) => {
      const index = applications.indexOf(app);
      const identity = applicationIdentity(app.binding);
      return `<button type="button" class="program-app-choice" data-app-index="${index}" data-app-key="${escape(`${identity.identity}|${identity.location}`)}"><strong>${escape(app.name)}</strong><span>${escape(identity.source)}${app.version ? ` · 当前版本 ${escape(app.version)}` : ""} · ${app.availability === "ready" ? "可代理启动" : "当前入口暂不可启动"}</span><small>${escape(identity.identity)}</small>${identity.location ? `<small>${escape(identity.location)}</small>` : ""}${app.availability !== "ready" ? `<small>${escape(app.detail)} 可保留关联，恢复可用后再启动。</small>` : ""}</button>`;
    }).join("") : `<p class="hint">${catalogRefreshing ? "正在查找应用；已有表单仍可编辑。" : "没有匹配的应用。可调整搜索词、重新扫描，或使用手动选择。"}</p>`;
    const results = $("#program-app-results");
    // Keep focus and scroll position when polling returns an unchanged snapshot.
    if (results.innerHTML !== markup) {
      const focused = document.activeElement?.closest<HTMLElement>("[data-app-index]");
      const focusedKey = focused && results.contains(focused) ? focused.dataset.appKey : null;
      const scrollTop = results.scrollTop;
      results.innerHTML = markup;
      results.scrollTop = scrollTop;
      if (focusedKey) [...results.querySelectorAll<HTMLElement>("[data-app-key]")].find((item) => item.dataset.appKey === focusedKey)?.focus({ preventScroll: true });
    }
  }

  async function readApplications(generation: number, force: boolean, deadline: number) {
    let timeout: ReturnType<typeof setTimeout> | undefined;
    try {
      const catalog = await Promise.race([
        services.api.installedProxyApplications(force),
        new Promise<never>((_, reject) => { timeout = setTimeout(() => reject(new Error("读取时间较长，已保留现有列表。请稍后重新扫描。")), Math.max(1, Math.min(15000, deadline - Date.now()))); }),
      ]);
      if (generation !== catalogGeneration || !pickerVisible()) return;
      applications = catalog.applications;
      catalogRefreshing = catalog.refreshing === true;
      $<HTMLButtonElement>("#program-app-refresh").disabled = catalogRefreshing;
      const checked = catalog.checkedAt ? ` · 最近读取 ${new Date(catalog.checkedAt).toLocaleTimeString()}` : "";
      $("#program-app-feedback").textContent = catalogRefreshing
        ? `正在后台刷新；当前 ${applications.length} 个入口可先选择，表单内容不会丢失。`
        : catalog.warnings.length ? "部分应用信息暂未读到，现有列表和草稿已保留。可重新扫描或手动选择。"
        : `找到 ${applications.length} 个应用入口${checked}。选择后仍需点击保存。`;
      renderApplications();
      if (catalogRefreshing && Date.now() < deadline) {
        catalogTimer = setTimeout(() => { catalogTimer = null; void readApplications(generation, false, deadline); }, 500);
      } else if (catalogRefreshing) {
        stopCatalog();
        $("#program-app-feedback").textContent = "扫描时间较长，已保留现有列表和草稿。请稍后重新扫描。";
      }
    } catch (error) {
      if (generation !== catalogGeneration || !pickerVisible()) return;
      stopCatalog();
      $("#program-app-feedback").textContent = `应用信息暂未更新：${services.error(error)} 现有列表和草稿已保留。`;
      renderApplications();
    } finally { if (timeout !== undefined) clearTimeout(timeout); }
  }

  function showApplications(force = false) {
    stopCatalog();
    $("#program-app-picker").classList.remove("is-hidden");
    catalogRefreshing = true;
    $<HTMLButtonElement>("#program-app-refresh").disabled = true;
    $("#program-app-feedback").textContent = applications.length ? "现有列表可先选择，正在后台检查更新…" : "正在读取已安装应用；你可以继续填写表单…";
    renderApplications();
    void readApplications(catalogGeneration, force, Date.now() + 30000);
  }

  function selectApplication(selected: InstalledApplication) {
    binding = selected.binding; selectedApplication = selected;
    field("executable").value = selected.executable;
    if (!field("name").value.trim()) field("name").value = selected.name;
    dirty = true;
    closeApplications();
    feedback(selected.availability === "ready"
      ? "已选择应用，尚未保存。原代理方式与参数已保留；请检查工作目录后保存关联。"
      : `已选择应用，尚未保存。${selected.detail} 保存关联不会启动该入口。`);
    render();
  }
  function editProgram(program: ProxyProgram) {
    editing = program.id; draftRevision = state!.revision;
    binding = program.binding ?? null;
    selectedApplication = program.resolution?.application ?? null;
    field("name").value = program.name;
    field("executable").value = program.executable;
    field("arguments").value = program.arguments.join("\n");
    field("directory").value = program.workingDirectoryRelative ?? program.workingDirectory ?? "";
    $<HTMLSelectElement>("#program-directory-kind").value = program.workingDirectoryRelative ? "relative" : program.workingDirectory ? "custom" : "application";
    $<HTMLSelectElement>("#program-mode").value = program.mode;
    dirty = false; updateModeHint();
  }
  $("#program-installed").addEventListener("click", () => showApplications(false));
  $("#program-app-refresh").addEventListener("click", () => showApplications(true));
  $("#program-app-close").addEventListener("click", closeApplications);
  $("#program-directory-clear").addEventListener("click", () => {
    $<HTMLSelectElement>("#program-directory-kind").value = "application"; field("directory").value = ""; dirty = true; render();
  });
  $("#program-app-search").addEventListener("input", renderApplications);
  $("#program-directory-kind").addEventListener("change", () => { dirty = true; render(); });
  $("#program-app-results").addEventListener("click", (event) => {
    const choice = (event.target as HTMLElement).closest<HTMLElement>("[data-app-index]");
    if (!choice || busy) return;
    const selected = applications[Number(choice.dataset.appIndex)];
    if (!selected) return;
    selectApplication(selected);
  });

  form.addEventListener("input", (event) => { if ((event.target as HTMLElement).id !== "program-app-search") dirty = true; });
  $("#program-mode").addEventListener("change", () => { dirty = true; updateModeHint(); });
  $("#program-refresh").addEventListener("click", () => void refresh(true));
  $("#program-reset").addEventListener("click", () => void operation(async () => {
    if (dirty && !await services.confirm({ title: "放弃未保存的修改？", message: "仅清空当前表单，已保存的程序清单不变。", confirmLabel: "放弃修改" })) return;
    reset();
  }));
  $("#program-browse").addEventListener("click", () => void operation(async () => {
    const path = await services.api.chooseProxyProgram();
    if (path === null) return;
    if (requiresApplicationInspection(path, state?.platform ?? "windows")) {
      selectApplication(await services.api.inspectProxyApplication(path));
      return;
    }
    closeApplications();
    binding = null; selectedApplication = null;
    if ($<HTMLSelectElement>("#program-directory-kind").value === "relative") {
      $<HTMLSelectElement>("#program-directory-kind").value = "application"; field("directory").value = "";
    }
    field("executable").value = path;
    if (!field("name").value.trim()) field("name").value = suggestedProgramName(path);
    dirty = true;
  }));
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    if (!state?.supported || draftRevision === null || busy) return;
    // Capture before disabling the form. Parameters are literal strings, not a shell command.
    void operation(async () => {
      const input: ProgramInput = {
        id: editing, name: field("name").value.trim(), executable: field("executable").value.trim(),
        arguments: parseProgramArguments(field("arguments").value),
        binding,
        workingDirectory: $<HTMLSelectElement>("#program-directory-kind").value === "custom" ? field("directory").value.trim() || null : null,
        workingDirectoryRelative: $<HTMLSelectElement>("#program-directory-kind").value === "relative" ? field("directory").value.trim() || "." : null,
        mode: $<HTMLSelectElement>("#program-mode").value as ProgramProxyMode,
      };
      state = await services.api.saveProxyProgram(input, draftRevision!);
      reset();
      feedback("程序已保存到清单，尚未启动。点击对应条目的“代理启动”开始使用。");
    });
  });
  $("#program-list").addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-program-action]");
    const id = button?.closest<HTMLElement>("[data-program-id]")?.dataset.programId;
    const program = state?.programs.find((entry) => entry.id === id);
    if (!button || button.disabled || !program || !state || busy) return;
    const action = button.dataset.programAction;
    void operation(async () => {
      if (action === "edit" || action === "associate") {
        if (dirty && !await services.confirm({ title: "切换编辑的程序？", message: "当前未保存的表单修改将被放弃，已保存的程序不会受到影响。", confirmLabel: "切换编辑" })) return;
        editProgram(program);
        if (action === "associate") showApplications(false);
        // The operation releases its busy state before focusing the editor.
        window.requestAnimationFrame(() => field("name").focus());
      } else if (action === "delete") {
        const revision = state!.revision;
        if (!await services.confirm({ title: `从清单移除“${program.name}”？`, message: "只删除 Serylane 中的这条配置，不删除程序文件、不卸载软件，也不关闭已运行的进程。", confirmLabel: "移除条目", returnFocus: $("#program-refresh") })) return;
        state = await services.api.deleteProxyProgram(program.id, revision);
        if (editing === program.id) reset();
        else if (draftRevision === revision) draftRevision = state.revision;
        feedback("条目已移除；程序文件和正在运行的进程未改动。");
      } else if (action === "launch") {
        const revision = state!.revision;
        if (!await services.confirm({ title: `代理启动“${program.name}”？`, message: `将${program.binding ? "重新定位当前安装版本并启动" : "启动"} ${program.name}，仅为新进程配置 ${state!.proxyEndpoint}。程序自身窗口可能出现；不会切换系统代理或关闭已有实例。启动成功不代表所有流量均走代理。`, confirmLabel: "代理启动", returnFocus: $("#program-refresh") })) return;
        state = await services.api.launchProxyProgram(program.id, revision);
        feedback(state.programs.find((entry) => entry.id === program.id)?.launchPending
          ? "启动结果待确认；请稍后刷新查看。暂不重复发起启动，已有配置保持不变。"
          : "已发起代理启动。请在目标程序中验证联网；程序退出、后台已有实例或不支持代理时，仍需检查。");
      }
    });
  });
  const visibilityObserver = new MutationObserver(() => {
    if (!pickerVisible()) stopCatalog();
  });
  visibilityObserver.observe(root, { attributes: true, attributeFilter: ["class"] });
  const pageHidden = () => stopCatalog();
  window.addEventListener("pagehide", pageHidden);
  render();
  return {
    refresh,
    dispose() { disposed = true; stopCatalog(); visibilityObserver.disconnect(); window.removeEventListener("pagehide", pageHidden); },
  };
}

export const proxyCompatibilityMarkup = `
  <article class="panel proxy-compatibility-panel" id="proxy-compatibility-panel">
    <div class="panel-heading"><div><div class="section-label">PROXY COMPATIBILITY</div><h2>系统代理兼容性</h2></div><button class="button button-quiet" id="proxy-compatibility-check" type="button">只读检查</button></div>
    <p class="hint">Windows 系统代理使用单一地址，兼容 HTTP 与 HTTPS CONNECT。只读检查会将当前系统代理地址交给 HTTP 解析器校验，不切换代理、不重启程序，也不发出外部网络请求。</p>
    <p class="program-feedback" id="proxy-compatibility-status" role="status" aria-live="polite">尚未检查。格式通过不代表所有应用实际使用此代理；环境变量、应用自身设置及长连接仍需单独验证。</p>
    <dl class="proxy-compatibility-results is-hidden" id="proxy-compatibility-results"><div><dt>期望代理</dt><dd id="proxy-compatibility-expected">—</dd></div><div><dt>HTTP 解析</dt><dd id="proxy-compatibility-http">—</dd></div><div><dt>HTTPS 解析</dt><dd id="proxy-compatibility-https">—</dd></div></dl>
  </article>`;

export function mountProxyCompatibility(root: HTMLElement, check: () => Promise<ProxyCompatibility>, errorMessage: (error: unknown) => string) {
  const button = root.querySelector<HTMLButtonElement>("#proxy-compatibility-check")!;
  const status = root.querySelector<HTMLElement>("#proxy-compatibility-status")!;
  button.addEventListener("click", async () => {
    if (button.disabled) return;
    button.disabled = true;
    button.textContent = "检查中…";
    root.setAttribute("aria-busy", "true");
    status.dataset.error = "false";
    root.querySelector("#proxy-compatibility-results")!.classList.add("is-hidden");
    try {
      const result = await check();
      status.textContent = result.detail;
      status.dataset.error = String(result.supported && !result.compatible);
      for (const [id, value] of [["expected", result.expectedProxy], ["http", result.resolvedHttp], ["https", result.resolvedHttps]]) {
        root.querySelector(`#proxy-compatibility-${id}`)!.textContent = value ?? "DIRECT / 未解析到代理";
      }
      root.querySelector("#proxy-compatibility-results")!.classList.toggle("is-hidden", !result.supported);
    } catch (error) { status.textContent = errorMessage(error); status.dataset.error = "true"; }
    finally { button.disabled = false; button.textContent = "只读检查"; root.removeAttribute("aria-busy"); }
  });
}
