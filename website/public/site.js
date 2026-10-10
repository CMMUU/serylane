"use strict";

// Only public release metadata is fetched from this site's Worker. Never invoke desktop APIs, open local listeners,
// accesses a subscription, saves a proxy configuration, or connects to Codex.
(() => {
  const menuButton = document.querySelector(".menu-toggle");
  const menu = document.querySelector("#main-nav");
  function setMenu(open, returnFocus = false) {
    if (!menu || !menuButton) return;
    menu.classList.toggle("is-open", open);
    menuButton.setAttribute("aria-expanded", String(open));
    menuButton.setAttribute("aria-label", open ? "关闭导航" : "打开导航");
    if (returnFocus) menuButton.focus();
  }
  menuButton?.addEventListener("click", () => setMenu(menuButton.getAttribute("aria-expanded") !== "true"));
  menu?.addEventListener("click", event => { if (event.target.closest("a")) setMenu(false); });
  document.addEventListener("keydown", event => {
    if (event.key === "Escape" && menuButton?.getAttribute("aria-expanded") === "true") setMenu(false, true);
  });
  document.addEventListener("click", event => {
    if (menuButton?.getAttribute("aria-expanded") === "true" && !event.target.closest(".site-header")) setMenu(false);
  });
  window.matchMedia("(min-width: 641px)").addEventListener("change", event => { if (event.matches) setMenu(false); });

  const tabs = [...document.querySelectorAll("[data-system]")];
  const architectureButtons = [...document.querySelectorAll("[data-architecture]")];
  if (!tabs.length) return;
  const systems = {
    windows: { name: "Windows 10 / 11", short: "Windows", icon: "windows", format: "EXE", defaultArchitecture: "x64" },
    macos: { name: "macOS", short: "macOS", icon: "apple", format: "DMG", defaultArchitecture: "arm64" },
    linux: { name: "Linux · AppImage", short: "Linux", icon: "linux", format: "AppImage", defaultArchitecture: "x64" },
  };
  let release = null;
  let loading = true;
  let selectedSystem = "windows";
  let selectedArchitecture = "x64";
  function architectureLabel(architecture) {
    if (selectedSystem === "macos") return architecture === "arm64" ? "Apple 芯片（M 系列）" : "Intel 芯片";
    return architecture === "arm64" ? "ARM64" : "x64";
  }
  function updateDownloadSelection() {
    const system = systems[selectedSystem];
    const architecture = architectureLabel(selectedArchitecture);
    document.querySelector("#architecture-selector").setAttribute("data-platform", selectedSystem);
    document.querySelector("#mac-chip-help").hidden = selectedSystem !== "macos";
    document.querySelector("#platform-name").textContent = system.name;
    document.querySelector("#platform-icon use").setAttribute("href", `#i-${system.icon}`);
    document.querySelector("#download-panel").setAttribute("aria-labelledby", `tab-${selectedSystem}`);
    tabs.forEach(tab => {
      const active = tab.dataset.system === selectedSystem;
      tab.disabled = false;
      tab.setAttribute("aria-selected", String(active));
      tab.tabIndex = active ? 0 : -1;
    });
    architectureButtons.forEach(button => {
      button.disabled = false;
      button.textContent = architectureLabel(button.dataset.architecture);
      button.setAttribute("aria-pressed", String(button.dataset.architecture === selectedArchitecture));
    });
    const target = `${selectedSystem}-${selectedArchitecture}`;
    const asset = release?.assets[target];
    const domesticAvailable = asset?.hkAvailable === true;
    const github = document.querySelector("#download-github");
    const gitee = document.querySelector("#download-gitee");
    // These URLs always resolve again on click, even if this page was left open before a release.
    github.href = `/download/${target}?channel=github`;
    gitee.href = `https://downloads.cmmuu.com/download/serylane/latest/${target}`;
    github.querySelector("span").textContent = "GitHub 备用";
    gitee.querySelector("span").textContent = `下载最新版 · ${system.format}`;
    const links = document.querySelector(".release-links");
    links.append(gitee, github);
    for (const [link, channelName] of [[gitee, "下载中心优先、GitHub 备用"], [github, "GitHub"]]) {
      link.dataset.system = selectedSystem;
      link.dataset.architecture = selectedArchitecture;
      link.setAttribute("aria-label", `下载最新正式版 Serylane ${system.short} ${architecture} ${system.format} 安装包，${channelName}，点击时重新核对版本`);
    }
    const note = !release
      ? "默认使用下载中心；点击时核对最新正式版。连接异常可选择 GitHub 备用。"
      : domesticAvailable
        ? `已核对下载中心 ${system.short} ${architecture} ${system.format} 包；点击时再次确认；连接异常可选择 GitHub 备用。`
        : `暂未确认下载中心 ${system.short} ${architecture} ${system.format} 包；点击时重新检查，未就绪则转 GitHub 同版本包。`;
    document.querySelector("#release-status").textContent = loading ? "正在查询最新正式版…"
      : release ? `当前最新正式版 ${release.version}。点击时再次核对版本。`
        : "暂时无法确认最新正式版。可重试查询，或点击下载重新检查；不会静默下载旧版。";
    document.querySelector("#channel-note").textContent = note;
    document.querySelector("#download-selection").textContent = `已选择 ${system.short} ${architecture}，${system.format} 安装包。${document.querySelector("#release-status").textContent} ${note}`;
  }
  function selectSystem(system, moveFocus = false) {
    if (!Object.hasOwn(systems, system)) return;
    selectedSystem = system;
    selectedArchitecture = systems[system].defaultArchitecture;
    updateDownloadSelection();
    if (moveFocus) document.querySelector(`#tab-${system}`).focus();
  }
  tabs.forEach((tab, index) => {
    tab.addEventListener("click", () => selectSystem(tab.dataset.system));
    tab.addEventListener("keydown", event => {
      let next;
      if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
      else if (event.key === "ArrowLeft") next = (index + tabs.length - 1) % tabs.length;
      else if (event.key === "Home") next = 0;
      else if (event.key === "End") next = tabs.length - 1;
      else return;
      event.preventDefault();
      selectSystem(tabs[next].dataset.system, true);
    });
  });
  architectureButtons.forEach(button => button.addEventListener("click", () => {
    if (!["x64", "arm64"].includes(button.dataset.architecture)) return;
    selectedArchitecture = button.dataset.architecture;
    updateDownloadSelection();
  }));
  updateDownloadSelection();
  async function refreshRelease() {
    if (!loading) { loading = true; release = null; updateDownloadSelection(); }
    const retry = document.querySelector("#release-retry");
    retry.hidden = false;
    retry.disabled = true;
    try {
      const response = await fetch("/api/releases/latest", { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(20000) });
      if (!response.ok) throw new Error("Release unavailable");
      const data = await response.json();
      if (!/^v\d+\.\d+\.\d+$/.test(data.version) || !data.assets) throw new Error("Invalid release");
      for (const system of Object.keys(systems)) for (const architecture of ["x64", "arm64"]) {
        if (typeof data.assets[`${system}-${architecture}`]?.domesticAvailable !== "boolean") throw new Error("Incomplete release");
      }
      release = data;
    } catch { release = null; }
    finally { loading = false; retry.disabled = false; retry.hidden = !!release; updateDownloadSelection(); }
  }
  document.querySelector("#release-retry").addEventListener("click", refreshRelease);
  void refreshRelease();
})();
