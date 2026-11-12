"use strict";

// Only public release metadata is fetched from this site's Worker. Never invoke desktop APIs, open local listeners,
// access a subscription, save a proxy configuration, or connect to Codex.
(() => {
  const chinese = /^zh(?:-|$)/i.test(document.documentElement.lang);
  const copy = chinese ? {
    openMenu: "打开导航", closeMenu: "关闭导航", retry: "重新查询版本",
    loading: "正在查询最新正式版…",
    latest: version => `当前最新正式版 ${version}。点击下载时再次核对版本。`,
    unavailable: "暂时无法确认最新正式版。可重试查询，或点击下载重新检查；不会静默下载旧版。",
    checking: target => `正在核对下载中心 ${target} 最新安装包…`,
    started: (version, target) => `本次下载：下载中心 · ${version} · ${target}。已交给浏览器下载；网络连接使用浏览器当前设置。`,
    failed: "下载中心暂时无法确认本次安装包，请稍后再次点击下载重试。本次没有开始下载。",
  } : {
    openMenu: "Open navigation", closeMenu: "Close navigation", retry: "Check version again",
    loading: "Checking the latest stable release…",
    latest: version => `Latest stable release: ${version}. The version is checked again when you download.`,
    unavailable: "The latest stable release could not be verified. Retry the check or click a download to check again; an older version is never substituted.",
    checking: target => `Checking the download center for the latest ${target} package…`,
    started: (version, target) => `This download: download center · ${version} · ${target}. Handed to your browser; the connection uses your browser's current network settings.`,
    failed: "The download center could not verify this package. Click the download again to retry. No download was started.",
  };

  // Navigation also runs on documentation and error pages without a download section.
  const menuButton = document.querySelector(".menu-toggle");
  const menu = document.querySelector("#main-nav");
  function setMenu(open, returnFocus = false) {
    if (!menu || !menuButton) return;
    menu.classList.toggle("is-open", open);
    menuButton.setAttribute("aria-expanded", String(open));
    menuButton.setAttribute("aria-label", open ? copy.closeMenu : copy.openMenu);
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

  const links = [...document.querySelectorAll(".download-target[data-download-target]")];
  if (!links.length) return;
  const suffixes = {
    "windows-x64": "x64-setup.exe", "windows-arm64": "arm64-setup.exe",
    "macos-x64": "x64.dmg", "macos-arm64": "aarch64.dmg",
    "linux-x64": "amd64.AppImage", "linux-arm64": "aarch64.AppImage",
  };
  const versionPattern = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
  const status = document.querySelector("#release-status");
  const retry = document.querySelector("#release-retry");
  const progress = document.querySelector("#download-progress");
  const selection = document.querySelector("#download-selection");
  let release = null;
  let loading = false;
  let refreshing = false;
  let resolvingDownload = false;
  function updateReleaseStatus() {
    const message = loading ? copy.loading : release ? copy.latest(release.version) : copy.unavailable;
    if (status) status.textContent = message;
    if (selection) selection.textContent = message;
  }

  for (const link of links) {
    const target = link.dataset.downloadTarget;
    if (!Object.hasOwn(suffixes, target)) continue;
    link.addEventListener("click", async event => {
      // Leave new-tab/window, save-link and keyboard-modifier behavior to the real anchor.
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      if (resolvingDownload) return;
      resolvingDownload = true;
      if (progress) progress.textContent = copy.checking(target);
      try {
        const reply = await fetch(`/api/download/${target}`, { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(20000) });
        if (!reply.ok) throw new Error("Unavailable");
        const data = await reply.json();
        if (data?.channel !== "hk" || typeof data.version !== "string" || !versionPattern.test(data.version)) throw new Error("Invalid release");
        const [major, minor, patch] = data.version.slice(1).split(".").map(Number);
        const brand = major === 0 && (minor < 7 || (minor === 7 && patch < 7)) ? "RouteDeck" : "Serylane";
        const expected = `https://files.cmmuu.com/releases/serylane/${data.version}/${brand}_${data.version.slice(1)}_${suffixes[target]}`;
        // Exact identity rejects alternate hosts, credentials, queries, fragments, versions and architectures.
        if (data.url !== expected) throw new Error("Invalid download");
        if (progress) progress.textContent = copy.started(data.version, target);
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        window.location.assign(expected);
      } catch {
        if (progress) progress.textContent = copy.failed;
      } finally {
        resolvingDownload = false;
      }
    });
  }

  async function refreshRelease() {
    if (refreshing) return;
    refreshing = true;
    loading = true;
    release = null;
    if (retry) { retry.hidden = false; retry.disabled = true; }
    updateReleaseStatus();
    try {
      const response = await fetch("/api/releases/latest", { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(20000) });
      if (!response.ok) throw new Error("Release unavailable");
      const data = await response.json();
      if (typeof data?.version !== "string" || !versionPattern.test(data.version) || !data.assets) throw new Error("Invalid release");
      for (const target of Object.keys(suffixes)) {
        if (data.assets[target]?.channel !== "hk") throw new Error("Incomplete release");
      }
      release = data;
    } catch {
      release = null;
    } finally {
      loading = false;
      refreshing = false;
      if (retry) { retry.disabled = false; retry.hidden = !!release; }
      updateReleaseStatus();
    }
  }
  if (retry) {
    retry.textContent = copy.retry;
    retry.addEventListener("click", refreshRelease);
  }
  void refreshRelease();
})();
