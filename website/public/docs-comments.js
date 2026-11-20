"use strict";

// All locales use one public Issue. The website only reads text previews.
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

  const panel = document.querySelector("[data-docs-comments]");
  if (!panel) return;
  const status = panel.querySelector("[data-comments-status]");
  const list = panel.querySelector("[data-comments-list]");
  const retry = panel.querySelector("[data-comments-retry]");
  const anchor = panel.querySelector("[data-comments-thread]");
  const thread = anchor.href;
  const anchorText = anchor.textContent;
  const copy = panel.dataset;
  const date = new Intl.DateTimeFormat(document.documentElement.lang, { dateStyle: "short" });
  let loading = false;

  function render(comment) {
    const row = document.createElement("li");
    const meta = document.createElement("div");
    meta.className = "comment-meta";
    const author = document.createElement("strong");
    author.textContent = comment.author;
    author.dir = "auto";
    const permalink = document.createElement("a");
    permalink.href = thread + "#issuecomment-" + comment.id;
    permalink.target = "_blank";
    permalink.rel = "noopener noreferrer";
    const timestamp = document.createElement("time");
    timestamp.dateTime = comment.createdAt;
    timestamp.textContent = date.format(new Date(comment.createdAt));
    permalink.append(timestamp);
    meta.append(author, permalink);
    const body = document.createElement("p");
    body.dir = "auto";
    body.textContent = comment.text;
    row.append(meta, body);
    return row;
  }

  async function load() {
    if (loading) return;
    loading = true;
    retry.hidden = true;
    panel.setAttribute("aria-busy", "true");
    status.textContent = copy.commentsLoading;
    const controller = new AbortController();
    const timer = window.setTimeout(() => controller.abort(), 7500);
    try {
      const response = await fetch(panel.dataset.docsComments, { credentials: "omit", signal: controller.signal });
      if (response.status === 410) {
        list.hidden = true;
        status.textContent = copy.commentsMissing;
        anchor.href = copy.commentsIssues;
        anchor.textContent = copy.commentsBrowse;
        return;
      }
      if (!response.ok) throw new Error("GitHub unavailable");
      const { comments } = await response.json();
      if (!Array.isArray(comments) || comments.length > 6) throw new Error("Unexpected preview");
      anchor.href = thread;
      anchor.textContent = anchorText;
      list.replaceChildren(...comments.map(render));
      list.hidden = !comments.length;
      status.textContent = comments.length ? copy.commentsPreview : copy.commentsEmpty;
    } catch {
      list.hidden = true;
      status.textContent = copy.commentsUnavailable;
      retry.hidden = false;
    } finally {
      window.clearTimeout(timer);
      loading = false;
      panel.setAttribute("aria-busy", "false");
    }
  }

  retry.addEventListener("click", load);
  // The panel is after the article on narrow screens. Do not start its network
  // request until it is near the viewport; navigation does not wait for comments.
  if (typeof IntersectionObserver === "function") {
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) {
        observer.disconnect();
        void load();
      }
    }, { rootMargin: "320px" });
    observer.observe(panel);
  } else void load();
})();
