"use strict";

// The address is the source of truth. The menu is ordinary links without JS;
// enhancement only handles dismissal and carries over shared section anchors.
(() => {
  const picker = document.querySelector("[data-language-menu]");
  if (!picker) return;
  const trigger = picker.querySelector("summary");
  document.addEventListener("pointerdown", event => {
    if (picker.open && !picker.contains(event.target)) picker.open = false;
  });
  document.addEventListener("keydown", event => {
    if (event.key !== "Escape" || !picker.open) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    picker.open = false;
    trigger.focus();
  }, true);
  picker.addEventListener("click", event => {
    const link = event.target.closest("a[data-language-link]");
    if (!link) return;
    const shared = new Set(link.dataset.languageAnchors.split(" "));
    let hash = "";
    try { hash = decodeURIComponent(window.location.hash.slice(1)); } catch { /* Malformed external hash. */ }
    if (!hash) {
      // Use the chapter nearest the reading position if navigation was by scroll.
      const cutoff = Math.min(190, window.innerHeight * 0.3);
      for (const chapter of document.querySelectorAll("[data-chapter]")) {
        if (chapter.getBoundingClientRect().top <= cutoff) hash = chapter.id;
      }
    }
    link.hash = hash && shared.has(hash) ? hash : "";
    picker.open = false;
  });
})();
