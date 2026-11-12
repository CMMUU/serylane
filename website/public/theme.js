"use strict";

// Synchronous head script: only this one display preference may persist.
(() => {
  const root = document.documentElement;
  const validPreference = value => ["dark", "light", "system"].includes(value);
  const scheme = typeof window.matchMedia === "function" ? window.matchMedia("(prefers-color-scheme: dark)") : null;
  let preference = "dark";
  let buttons = [];
  try {
    const stored = window.localStorage.getItem("serylane-theme");
    if (validPreference(stored)) preference = stored;
  } catch { /* Storage access can be blocked; the default remains usable. */ }

  function applyTheme() {
    const theme = preference === "system" ? (scheme?.matches ? "dark" : "light") : preference;
    root.dataset.theme = theme;
    document.querySelector('meta[name="theme-color"]')?.setAttribute("content", theme === "dark" ? "#0a0d14" : "#f6f8fc");
    for (const button of buttons) button.setAttribute("aria-pressed", String(button.dataset.themeValue === preference));
  }
  applyTheme();
  // No initial motion runs until the document's controls and visibility are known.
  root.dataset.motion = "paused";
  scheme?.addEventListener("change", () => { if (preference === "system") applyTheme(); });

  function bindControls() {
    buttons = [...document.querySelectorAll("button[data-theme-value]")];
    for (const button of buttons) {
      button.addEventListener("click", () => {
        if (!validPreference(button.dataset.themeValue)) { applyTheme(); return; }
        preference = button.dataset.themeValue;
        applyTheme();
        try { window.localStorage.setItem("serylane-theme", preference); }
        catch { /* Keep this page interactive even if persistence is denied. */ }
      });
    }
    applyTheme();

    const diagram = document.querySelector("figure[data-routing-diagram]");
    if (!diagram) return;
    const button = document.querySelector("button[data-motion-toggle]");
    const reduced = typeof window.matchMedia === "function" ? window.matchMedia("(prefers-reduced-motion: reduce)") : null;
    const chinese = /^zh(?:-|$)/i.test(root.lang);
    const labels = chinese
      ? { pause: "暂停动效", resume: "继续动效", reduced: "已减少动态效果" }
      : { pause: "Pause animation", resume: "Resume animation", reduced: "Reduced motion" };
    let manuallyPaused = false;
    // Observe asynchronously without freezing the initially visible diagram.
    let inView = true;
    function updateMotion() {
      root.dataset.motion = manuallyPaused || reduced?.matches || document.hidden || !inView ? "paused" : "running";
      if (!button) return;
      button.disabled = !!reduced?.matches;
      // Pressed represents the user's pause choice (or enforced reduced motion),
      // not a temporary offscreen/background optimization.
      button.setAttribute("aria-pressed", String(manuallyPaused || !!reduced?.matches));
      button.textContent = reduced?.matches ? labels.reduced : manuallyPaused ? labels.resume : labels.pause;
    }
    button?.addEventListener("click", () => {
      if (reduced?.matches) return;
      manuallyPaused = !manuallyPaused;
      updateMotion();
    });
    reduced?.addEventListener("change", updateMotion);
    document.addEventListener("visibilitychange", updateMotion);
    if (typeof window.IntersectionObserver === "function") {
      const observer = new window.IntersectionObserver(entries => {
        for (const entry of entries) if (entry.target === diagram) inView = entry.isIntersecting;
        updateMotion();
      });
      observer.observe(diagram);
    }
    updateMotion();
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", bindControls, { once: true });
  else bindControls();
})();
