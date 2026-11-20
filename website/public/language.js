"use strict";

// The address is the source of truth. The menu is ordinary links without JS;
// enhancement only handles dismissal and carries over shared section anchors.
(() => {
  const toc = document.querySelector("[data-chapter-nav]");
  if (toc) {
    const links = [...toc.querySelectorAll("a")];
    function reflectChapter() {
      const hash = window.location.hash || links[0]?.hash;
      for (const link of links) {
        if (link.hash === hash) link.setAttribute("aria-current", "location");
        else link.removeAttribute("aria-current");
      }
    }
    window.addEventListener("hashchange", reflectChapter);
    reflectChapter();
  }

  const picker = document.querySelector("[data-language-menu]");
  if (!picker) return;
  const menus = [picker, ...document.querySelectorAll("[data-mobile-nav]")];
  document.addEventListener("pointerdown", event => {
    for (const menu of menus) if (menu.open && !menu.contains(event.target)) menu.open = false;
  });
  document.addEventListener("keydown", event => {
    if (event.key !== "Escape") return;
    const menu = menus.find(item => item.open && item.contains(event.target)) ?? menus.find(item => item.open);
    if (!menu) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    menu.open = false;
    menu.querySelector("summary").focus();
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
