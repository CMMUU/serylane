"use strict";

// Read plain-text public comment previews from our fixed first-party endpoint.
// Visitor authentication and submissions happen on the GitHub issue itself.
(() => {
  const panel = document.querySelector("[data-docs-comments]");
  if (!panel) return;
  const status = panel.querySelector("[data-comments-status]");
  const list = panel.querySelector("[data-comments-list]");
  const retry = panel.querySelector("[data-comments-retry]");
  const thread = panel.querySelector("[data-comments-thread]").href;
  const date = new Intl.DateTimeFormat("zh-TW", { dateStyle: "short" });
  let loading = false;

  function render(comment) {
    const row = document.createElement("li");
    const meta = document.createElement("div");
    meta.className = "comment-meta";
    const author = document.createElement("strong");
    author.textContent = comment.author;
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
    body.textContent = comment.text;
    row.append(meta, body);
    return row;
  }

  async function load() {
    if (loading) return;
    loading = true;
    retry.hidden = true;
    panel.setAttribute("aria-busy", "true");
    status.textContent = "正在讀取 GitHub 留言…";
    const controller = new AbortController();
    const timer = window.setTimeout(() => controller.abort(), 7500);
    try {
      const response = await fetch(panel.dataset.docsComments, { credentials: "omit", signal: controller.signal });
      if (!response.ok) throw new Error("GitHub unavailable");
      const { comments } = await response.json();
      if (!Array.isArray(comments) || comments.length > 6) throw new Error("Unexpected preview");
      list.replaceChildren(...comments.map(render));
      list.hidden = !comments.length;
      status.textContent = comments.length ? "留言預覽。完整內容與最新留言請到 GitHub。" : "目前沒有留言。歡迎到 GitHub 留下建議。";
    } catch {
      list.hidden = true;
      status.textContent = "暫時讀不到留言。你仍可直接到 GitHub 查看或發言。";
      retry.hidden = false;
    } finally {
      window.clearTimeout(timer);
      loading = false;
      panel.setAttribute("aria-busy", "false");
    }
  }

  retry.addEventListener("click", load);
  const toc = document.querySelector(".docs-feedback-layout [data-chapter-nav]");
  if (toc) {
    const links = [...toc.querySelectorAll("a")];
    function reflectChapter() {
      const hash = window.location.hash || links[0].hash;
      for (const link of links) {
        if (link.hash === hash) link.setAttribute("aria-current", "location");
        else link.removeAttribute("aria-current");
      }
    }
    window.addEventListener("hashchange", reflectChapter);
    reflectChapter();
  }
  void load();
})();
