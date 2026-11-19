import { THEME_OPTIONS } from "./theme";
import { icon, preferenceSwitch } from "./ui";

export const preferencesMarkup = `
  <article class="panel preferences-panel" aria-labelledby="language-heading">
    <h2 id="language-heading"><!--i18n:language-heading-->界面语言</h2>
    <label class="preference-row" for="settings-language">
      <span><!--i18n:language-label-->语言</span>
      <select id="settings-language" aria-describedby="language-status" disabled></select>
    </label>
    <p class="session-resume-help" id="language-status" role="status" aria-live="polite"><!--i18n:language-description-->选择后立即切换并自动保存。</p>
  </article>
  <article class="panel preferences-panel appearance-panel" aria-labelledby="appearance-heading">
    <h2 id="appearance-heading"><!--i18n:ui-86a63f23a076b1-->外观</h2>
    <fieldset class="appearance-settings">
      <legend class="visually-hidden"><!--i18n:ui-73c8bfb65d654e-->选择外观</legend>
      <div class="appearance-layout">
        <div class="appearance-intro"><strong><!--i18n:ui-73c8bfb65d654e-->选择外观</strong><p><!--i18n:ui-09cd1d99b21d4f-->立即生效并自动保存</p></div>
        <div class="appearance-grid" role="radiogroup" aria-label="外观主题" aria-describedby="appearance-status" data-i18n-aria-label="ui-d32fff09b9abfa">
          ${THEME_OPTIONS.map((option) => `
            <button type="button" class="theme-option" data-theme-choice="${option.id}" role="radio" aria-label="${option.label}" data-i18n-aria-label="${option.labelKey}" aria-checked="false" tabindex="-1" title="${option.description}" data-i18n-title="${option.descriptionKey}">
              <span class="theme-swatch" data-swatch="${option.id}" aria-hidden="true"><span class="swatch-dots"><i></i><i></i><i></i></span><span class="swatch-rows"></span></span>
              <span class="theme-copy"><strong><!--i18n:${option.labelKey}-->${option.label}</strong></span>
              <span class="theme-checkmark" aria-hidden="true">${icon("check")}</span>
            </button>`).join("")}
        </div>
      </div>
      <p class="appearance-status" id="appearance-status" role="status" aria-live="polite"></p>
    </fieldset>
  </article>
  <article class="panel preferences-panel runtime-preferences" aria-labelledby="runtime-settings-heading">
    <h2 id="runtime-settings-heading"><!--i18n:ui-078e20eb558ebc-->运行设置</h2>
    <form id="settings-form">
      <div class="preference-row">
        <span id="settings-mode-label"><!--i18n:ui-1676dd26a7685b-->网络模式</span>
        <select id="settings-mode" hidden aria-label="网络模式" data-i18n-aria-label="ui-1676dd26a7685b"><option value="manual"><!--i18n:ui-3e719c15585310-->本地端口</option><option value="system_proxy"><!--i18n:ui-24bbc60c5fb3b1-->系统代理</option><option value="tun"><!--i18n:ui-ba32107029d76d-->TUN 模式</option></select>
        <div class="preference-segments" role="radiogroup" aria-labelledby="settings-mode-label">
          <label><input type="radio" name="settings-network-mode" value="manual" /><span><!--i18n:ui-3e719c15585310-->本地端口</span></label>
          <label><input type="radio" name="settings-network-mode" value="system_proxy" /><span><!--i18n:ui-24bbc60c5fb3b1-->系统代理</span></label>
          <label><input type="radio" name="settings-network-mode" value="tun" /><span><!--i18n:ui-ba32107029d76d-->TUN 模式</span></label>
        </div>
      </div>
      <div class="preference-row preference-ports">
        <label for="settings-mixed-port"><!--i18n:mixed-port-->混合端口<input id="settings-mixed-port" type="number" min="1024" max="65535" required /></label>
        <label for="settings-controller-port"><!--i18n:controller-port-->控制端口<input id="settings-controller-port" type="number" min="1024" max="65535" required /></label>
      </div>
      <label class="preference-row" for="settings-startup-mode"><span><!--i18n:ui-80aa931c1291e7-->启动模式</span>
        <select id="settings-startup-mode" aria-describedby="session-resume-help">
          <option value="manual"><!--i18n:ui-f9ed3ea781ddc1-->不随系统启动</option>
          <option value="background"><!--i18n:ui-a923257f7ccc7a-->登录后后台恢复上次状态（推荐）</option>
          <option value="window"><!--i18n:ui-197597849b32a6-->登录后打开窗口并恢复</option>
          <option value="custom" disabled hidden><!--i18n:ui-4a0ac4047ddc1a-->保留旧版自定义设置</option>
        </select>
      </label>
      <p class="session-resume-help" id="session-resume-help"></p>
      <div class="preference-row"><span><!--i18n:ui-f95db8ce024dda-->系统登录项</span><div class="runtime-actions"><button class="button button-quiet" id="settings-startup-check" type="button"><!--i18n:ui-aba2334fda5b43-->核对状态</button><button class="button button-quiet is-hidden" id="settings-startup-repair" type="button"><!--i18n:ui-4faaf187544306-->修复登录启动</button><button class="button button-quiet is-hidden" id="settings-startup-open" type="button"><!--i18n:ui-33f43e0b8fd14f-->打开系统登录项</button></div></div>
      <p class="session-resume-help" id="startup-registration-status" role="status" aria-live="polite"></p>
      <p class="session-resume-help" id="session-resume-status" role="status" aria-live="polite"></p>
      <label class="preference-row" for="settings-global-traffic"><span><!--i18n:ui-ad4770e6587374-->显示全局流量监控</span>${preferenceSwitch("settings-global-traffic")}</label>
      <label class="preference-row" for="settings-retention"><span><!--i18n:ui-9fcf9bd77b569c-->诊断报告保留天数</span><input class="preference-number" id="settings-retention" type="number" min="1" max="90" required /></label>
      <label class="preference-row" for="settings-app-log-retention"><span><!--i18n:ui-90f26cb220fd7b-->应用日志保留天数</span><input class="preference-number" id="settings-app-log-retention" type="number" min="1" max="90" step="1" required aria-describedby="app-log-retention-help" /></label>
      <p class="session-resume-help" id="app-log-retention-help"><!--i18n:ui-0a92e8a6de9061-->默认 3 天，可设置 1–90 天；正文上限 256 KiB / 2,000 条，满额时提前淘汰旧记录。缩短期限会清理过期记录，延长不会恢复已清理内容。不保存对话或订阅凭据。</p>
      <div class="preference-actions runtime-actions"><details class="preference-help"><summary><!--i18n:ui-5feba1a02061e6-->网络模式说明</summary><p id="network-mode-help" data-i18n-live><!--i18n:ui-f68f724b7e0a5e-->模式与端口改动仅在保存后生效；更改前会停止核心，再恢复运行。</p></details><button class="button button-primary" type="submit"><!--i18n:ui-c8550237ba701f-->保存设置</button></div>
    </form>
  </article>
  <article class="panel preferences-panel app-update-panel" id="app-update-panel" aria-busy="false" aria-labelledby="update-heading">
    <div class="preferences-heading"><h2 id="update-heading"><!--i18n:ui-651f274470153a-->软件更新</h2><span class="control-state-pill is-hidden" id="app-update-state"><!--i18n:ui-2329bc02cd048a-->待检查</span></div>
    <div class="preference-row update-current-row"><span><!--i18n:ui-837bc957672116-->当前版本</span><div><strong id="app-update-current">—</strong><button class="button button-accent" id="app-update-check" type="button"><!--i18n:ui-7f68ebad19ba6b-->检查更新</button></div></div>
    <form id="update-preferences-form">
      <label class="preference-row" for="settings-auto-check-updates"><span title="启动时在后台检查，运行期间每 6 小时复查一次。" data-i18n-title="ui-fe7919c6a90508"><!--i18n:ui-c42a3dd9c2e40b-->自动检查更新</span>${preferenceSwitch("settings-auto-check-updates")}</label>
      <label class="preference-row" for="settings-auto-download-updates"><span title="默认关闭；开启后会占用下载带宽，仍需确认安装。" data-i18n-title="ui-0d6186d1e08386"><!--i18n:ui-7392248edb803a-->自动下载更新</span>${preferenceSwitch("settings-auto-download-updates")}</label>
      <div class="preference-actions update-preference-actions"><details class="preference-help"><summary><!--i18n:ui-61417926879ee6-->安装前会再次确认</summary><p><!--i18n:update-path-help-->按顺序检查并下载正式更新。安装前会核对版本、签名和 SHA-256，并由你确认。<!--i18n:update-restart-hint-->安装和重启会短暂中断代理连接。</p></details><button class="button button-quiet" id="app-update-save" type="submit"><!--i18n:ui-b2f5b0da3e6744-->保存更新偏好</button></div>
    </form>
    <details class="preference-help" id="app-update-history" hidden><summary><!--i18n:ui-4a9c68eeedefb8-->历史安装记录（与本次下载分开）</summary><p class="hint" id="app-update-last-install"></p></details>
    <div class="update-feedback is-hidden" id="app-update-feedback">
      <div class="app-update-summary"><div aria-live="polite"><strong id="app-update-title"><!--i18n:ui-cdbc0434f568fc-->尚未检查</strong><p id="app-update-message"><!--i18n:update-path-help-->按顺序检查并下载正式更新。安装前会核对版本、签名和 SHA-256，并由你确认。</p></div></div>
      <div class="about-grid app-update-details"><span><!--i18n:ui-b556e7880bb7dc-->最新稳定版</span><strong id="app-update-latest"><!--i18n:ui-cdbc0434f568fc-->尚未检查</strong><span><!--i18n:ui-87d080e7b965a0-->发布日期</span><strong id="app-update-date">—</strong><span><!--i18n:ui-de0e6877cfcba3-->更新来源</span><strong id="app-update-source">—</strong></div>
      <p class="hint" id="app-update-network-path"></p>
      <ol class="app-update-channels" id="app-update-attempts" aria-label="本次下载路径记录" hidden data-i18n-aria-label="ui-ed75f37d9fa569"></ol>
      <ul class="app-update-channels is-hidden" id="app-update-channels" aria-label="更新路径检查结果" data-i18n-aria-label="update-check-results"></ul>
      <div class="app-update-progress is-hidden" id="app-update-progress"><progress id="app-update-progress-bar" max="100" value="0" aria-label="更新包下载进度" data-i18n-aria-label="ui-81be67254381e3"></progress><span id="app-update-progress-text" aria-live="off"></span></div>
      <div class="app-update-controls"><span class="hint"><!--i18n:ui-5b67f5a27f2d8e-->更新签名校验 · SHA-256 校验 · 禁止降级</span><div class="toolbar">
        <button class="button button-quiet is-hidden" id="app-update-open" type="button"><!--i18n:update-open-release-->打开发布页</button>
        <button class="button button-primary is-hidden" id="app-update-download" type="button"><!--i18n:ui-a78b542de2824e-->下载更新</button>
        <button class="button button-quiet is-hidden" id="app-update-cancel" type="button"><!--i18n:ui-14d1e349b14010-->取消下载</button>
        <button class="button button-primary is-hidden" id="app-update-install" type="button"><!--i18n:ui-5074aba460a604-->安装并重启</button>
      </div></div>
      <details class="app-update-notes is-hidden" id="app-update-notes"><summary><!--i18n:ui-f9148d91539456-->查看发布说明</summary><p id="app-update-notes-content"></p></details>
    </div>
  </article>`;
