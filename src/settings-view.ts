import { THEME_OPTIONS } from "./theme";
import { icon, preferenceSwitch } from "./ui";

export const preferencesMarkup = `
  <article class="panel preferences-panel appearance-panel" aria-labelledby="appearance-heading">
    <h2 id="appearance-heading">外观</h2>
    <fieldset class="appearance-settings">
      <legend class="visually-hidden">选择外观</legend>
      <div class="appearance-layout">
        <div class="appearance-intro"><strong>选择外观</strong><p>立即生效并自动保存</p></div>
        <div class="appearance-grid" role="radiogroup" aria-label="外观主题" aria-describedby="appearance-status">
          ${THEME_OPTIONS.map((option) => `
            <button type="button" class="theme-option" data-theme-choice="${option.id}" role="radio" aria-label="${option.label}" aria-checked="false" tabindex="-1" title="${option.description}">
              <span class="theme-swatch" data-swatch="${option.id}" aria-hidden="true"><span class="swatch-dots"><i></i><i></i><i></i></span><span class="swatch-rows"></span></span>
              <span class="theme-copy"><strong>${option.label}</strong></span>
              <span class="theme-checkmark" aria-hidden="true">${icon("check")}</span>
            </button>`).join("")}
        </div>
      </div>
      <p class="appearance-status" id="appearance-status" role="status" aria-live="polite"></p>
    </fieldset>
  </article>
  <article class="panel preferences-panel runtime-preferences" aria-labelledby="runtime-settings-heading">
    <h2 id="runtime-settings-heading">运行设置</h2>
    <form id="settings-form">
      <div class="preference-row">
        <span id="settings-mode-label">网络模式</span>
        <select id="settings-mode" hidden aria-label="网络模式"><option value="manual">本地端口</option><option value="system_proxy">系统代理</option><option value="tun">TUN 模式</option></select>
        <div class="preference-segments" role="radiogroup" aria-labelledby="settings-mode-label">
          <label><input type="radio" name="settings-network-mode" value="manual" /><span>本地端口</span></label>
          <label><input type="radio" name="settings-network-mode" value="system_proxy" /><span>系统代理</span></label>
          <label><input type="radio" name="settings-network-mode" value="tun" /><span>TUN 模式</span></label>
        </div>
      </div>
      <div class="preference-row preference-ports">
        <label for="settings-mixed-port">Mixed Port<input id="settings-mixed-port" type="number" min="1024" max="65535" required /></label>
        <label for="settings-controller-port">Controller Port<input id="settings-controller-port" type="number" min="1024" max="65535" required /></label>
      </div>
      <label class="preference-row" for="settings-startup-mode"><span>启动模式</span>
        <select id="settings-startup-mode" aria-describedby="session-resume-help">
          <option value="manual">不随系统启动</option>
          <option value="background">登录后后台恢复上次状态（推荐）</option>
          <option value="window">登录后打开窗口并恢复</option>
          <option value="custom" disabled hidden>保留旧版自定义设置</option>
        </select>
      </label>
      <p class="session-resume-help" id="session-resume-help"></p>
      <div class="preference-row"><span>系统登录项</span><div class="runtime-actions"><button class="button button-quiet" id="settings-startup-check" type="button">核对状态</button><button class="button button-quiet is-hidden" id="settings-startup-repair" type="button">修复登录启动</button><button class="button button-quiet is-hidden" id="settings-startup-open" type="button">打开系统登录项</button></div></div>
      <p class="session-resume-help" id="startup-registration-status" role="status" aria-live="polite"></p>
      <p class="session-resume-help" id="session-resume-status" role="status" aria-live="polite"></p>
      <label class="preference-row" for="settings-global-traffic"><span>显示全局流量监控</span>${preferenceSwitch("settings-global-traffic")}</label>
      <label class="preference-row" for="settings-retention"><span>诊断报告保留天数</span><input class="preference-number" id="settings-retention" type="number" min="1" max="90" required /></label>
      <label class="preference-row" for="settings-app-log-retention"><span>应用日志保留天数</span><input class="preference-number" id="settings-app-log-retention" type="number" min="1" max="90" step="1" required aria-describedby="app-log-retention-help" /></label>
      <p class="session-resume-help" id="app-log-retention-help">默认 3 天，可设置 1–90 天；正文上限 256 KiB / 2,000 条，满额时提前淘汰旧记录。缩短期限会清理过期记录，延长不会恢复已清理内容。不保存对话或订阅凭据。</p>
      <div class="preference-actions runtime-actions"><details class="preference-help"><summary>网络模式说明</summary><p id="network-mode-help">模式与端口改动仅在保存后生效；更改前会停止核心，再恢复运行。</p></details><button class="button button-primary" type="submit">保存设置</button></div>
    </form>
  </article>
  <article class="panel preferences-panel app-update-panel" id="app-update-panel" aria-busy="false" aria-labelledby="update-heading">
    <div class="preferences-heading"><h2 id="update-heading">软件更新</h2><span class="control-state-pill is-hidden" id="app-update-state">待检查</span></div>
    <div class="preference-row update-current-row"><span>当前版本</span><div><strong id="app-update-current">—</strong><button class="button button-accent" id="app-update-check" type="button">检查更新</button></div></div>
    <form id="update-preferences-form">
      <label class="preference-row" for="settings-update-source"><span>更新渠道</span><span class="update-source-control"><select id="settings-update-source" aria-label="更新渠道"><option value="auto">自动（香港优先）</option><option value="hk">香港下载中心</option><option value="gitee">Gitee</option><option value="github">GitHub</option></select><span class="muted">自动：香港 → Gitee → GitHub</span></span></label>
      <label class="preference-row" for="settings-auto-check-updates"><span title="启动后延迟检查，运行期间每 6 小时检查一次。">自动检查更新</span>${preferenceSwitch("settings-auto-check-updates")}</label>
      <label class="preference-row" for="settings-auto-download-updates"><span title="默认关闭；开启后会占用下载带宽，仍需确认安装。">自动下载更新</span>${preferenceSwitch("settings-auto-download-updates")}</label>
      <div class="preference-actions update-preference-actions"><details class="preference-help"><summary>安装前会再次确认</summary><p>安装和重启会短暂中断代理连接。自动模式选用最高稳定版，同版本优先香港下载中心，失败时依次回退 Gitee、GitHub；香港或 Gitee 尚未同步新版时使用已核验的最新渠道。仅同版本、同摘要、同签名的包可跨渠道回退。退出软件会清除尚未安装的下载缓存。Linux 内置更新适用于 AppImage。</p></details><button class="button button-quiet" id="app-update-save" type="submit">保存更新偏好</button></div>
    </form>
    <div class="update-feedback is-hidden" id="app-update-feedback">
      <div class="app-update-summary"><div aria-live="polite"><strong id="app-update-title">尚未检查</strong><p id="app-update-message">自动模式优先香港下载中心，同版本 Gitee / GitHub 备用；安装前由你确认。</p></div></div>
      <div class="about-grid app-update-details"><span>最新稳定版</span><strong id="app-update-latest">尚未检查</strong><span>发布日期</span><strong id="app-update-date">—</strong><span>当前渠道</span><strong id="app-update-source">—</strong></div>
      <ul class="app-update-channels is-hidden" id="app-update-channels" aria-label="渠道检查结果"></ul>
      <div class="app-update-progress is-hidden" id="app-update-progress"><progress id="app-update-progress-bar" max="100" value="0" aria-label="更新包下载进度"></progress><span id="app-update-progress-text" aria-live="off"></span></div>
      <div class="app-update-controls"><span class="hint">官方签名校验 · SHA-256 校验 · 禁止降级</span><div class="toolbar">
        <button class="button button-quiet is-hidden" id="app-update-open" type="button">发布页面</button>
        <button class="button button-primary is-hidden" id="app-update-download" type="button">下载更新</button>
        <button class="button button-quiet is-hidden" id="app-update-cancel" type="button">取消下载</button>
        <button class="button button-primary is-hidden" id="app-update-install" type="button">安装并重启</button>
      </div></div>
      <details class="app-update-notes is-hidden" id="app-update-notes"><summary>查看发布说明</summary><p id="app-update-notes-content"></p></details>
    </div>
  </article>`;
