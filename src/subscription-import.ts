import type { SubscriptionImportResult } from "./types";

export const subscriptionImportMarkup = `
  <div class="subscription-import-card" id="managed-subscription-panel" hidden>
    <div class="subscription-import-heading"><h3 id="managed-subscription-title">添加订阅</h3><p>粘贴地址，验证后保存。选用与启动是独立操作。</p></div>
    <form id="managed-subscription-form" aria-labelledby="managed-subscription-title">
      <fieldset id="managed-subscription-fields">
        <div class="subscription-import-fields">
          <label><span>订阅地址</span><input id="managed-subscription-url" required type="url" placeholder="https://example.com/subscribe?token=…" autocomplete="off" spellcheck="false" /></label>
          <label><span>订阅名称</span><input id="managed-subscription-name" required value="我的订阅" maxlength="128" autocomplete="off" /></label>
        </div>
        <label class="subscription-import-option"><input id="managed-subscription-activate" type="checkbox" /><span><strong>添加后选用</strong><small id="managed-subscription-activate-hint">未勾选时仅保存，不替换当前配置。</small></span></label>
        <label class="subscription-import-option"><input id="managed-subscription-openai" type="checkbox" /><span><strong>OpenAI 灾备</strong><small>仅 AI 代理模式可用；生成结果单独提示，不自动切换模式。</small></span></label>
        <details class="subscription-import-advanced"><summary>高级选项</summary><label><span>User-Agent</span><input id="managed-subscription-ua" value="clash.meta" autocomplete="off" spellcheck="false" /></label><p>通常保持默认即可。Serylane 核心运行时，下载复用其代理端口；未运行时不会为下载自动启动。订阅凭据仅在本机存储。</p></details>
        <div class="subscription-import-actions"><button class="button button-primary" id="managed-subscription-import-button" type="submit">验证并添加</button><button class="button button-quiet" id="managed-subscription-cancel" type="button">收起</button></div>
      </fieldset>
      <p class="import-status" id="managed-subscription-import-status" role="status" aria-live="polite"></p>
    </form>
  </div>`;

export function describeSubscriptionImport(result: SubscriptionImportResult): { text: string; warning: boolean } {
  let text = result.created
    ? result.activated ? "订阅已添加并选用。" : "订阅已添加，未切换当前配置。"
    : result.activated ? "该订阅已存在，已选用保存的配置；未重新获取订阅。" : "该订阅已存在，未重复添加，也未更改当前配置。";
  if (result.observationError) text += ` ${result.observationError}`;
  if (result.openAiGeneration === "failed") {
    return { text: `${text} OpenAI 灾备未能开始：${result.openAiError || "请在订阅详情中重试"}`, warning: true };
  }
  return { text: result.openAiGeneration === "started" ? `${text} OpenAI 灾备任务已提交，结果请查看订阅详情。` : text, warning: Boolean(result.observationError) };
}
