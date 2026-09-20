import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

const read = (name) => readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
const { subscriptionBytes, subscriptionDate, describeSubscriptionUsage, subscriptionCardMarkup } = await import(`data:text/javascript;base64,${Buffer.from(ts.transpileModule(read("subscription-cards.ts"), { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText).toString("base64")}`);
const { subscriptionImportMarkup, describeSubscriptionImport } = await import(`data:text/javascript;base64,${Buffer.from(ts.transpileModule(read("subscription-import.ts"), { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText).toString("base64")}`);
const now = Date.parse("2026-09-07T00:00:00Z");
const GiB = 1024 ** 3;
const sample = (changes = {}) => ({ uploadBytes: 2 * GiB, downloadBytes: 28 * GiB, totalBytes: 100 * GiB, expiresAt: Date.parse("2026-10-07T00:00:00Z") / 1000, ...changes });
const subscription = () => ({ profile: { id: "fixture-only", displayName: "测试订阅", source: { type: "remote_subscription", host: "example.test" }, routingMode: "rule", openaiPolicy: { enabled: false, selectedNodes: [], autoMaintain: false } }, summary: { nodeCount: 12, proxyProviderCount: 2 }, revisionCount: 3, latestFetchedAt: "2026-09-05T00:00:00Z", latestMetadata: { bytes: 4096 }, latestValidation: { valid: true }, active: true, status: { checkedAt: "2026-09-07T00:00:00Z", lastError: null, usage: sample(), usageUpdatedAt: "2026-09-07T00:00:00Z" } });

test("provider upload plus download produce quota and remaining, not response bytes", () => {
  const result = describeSubscriptionUsage(sample(), now);
  assert.equal(result.used, 30 * GiB); assert.equal(result.remaining, 70 * GiB);
  assert.equal(result.progress, 30); assert.equal(result.expired, false);
  const card = subscriptionCardMarkup(subscription(), null, now);
  assert.match(card, /30 GiB/); assert.match(card, /共 100 GiB/); assert.match(card, /4 KiB · 3 个版本/);
});
test("zero is a real usage value while missing, negative and unsafe numbers stay unknown", () => {
  assert.equal(subscriptionBytes(0), "0 B"); assert.equal(subscriptionBytes(undefined), "—");
  for (const value of [null, -1, NaN, Infinity, "1024", Number.MAX_SAFE_INTEGER + 1]) assert.equal(subscriptionBytes(value), "—");
  assert.equal(describeSubscriptionUsage(sample({ uploadBytes: 0, downloadBytes: 0 }), now).progress, 0);
  assert.equal(describeSubscriptionUsage(sample({ downloadBytes: null }), now).used, null);
});
test("remaining quota is primary and periodic observations cannot apply or switch configuration", () => {
  const card = subscriptionCardMarkup(subscription(), null, now);
  assert.match(card, /剩余流量/);
  assert.match(card, /subscription-usage-value"><strong>70 GiB/);
  assert.match(card, /每 5 分钟直接检查订阅用量/);
  const poller = readFileSync(new URL('../src-tauri/src/subscription_quota.rs', import.meta.url), 'utf8');
  assert.match(poller, /POLL_SECONDS: i64 = 300/);
  assert.match(poller, /fetch_usage/);
  assert.doesNotMatch(poller, /activate_profile|refresh_profile|start_runtime|save_revision|apply_configuration/);
});
test("unknown or zero allowance never claims unlimited nor draws a made-up percentage", () => {
  for (const totalBytes of [null, 0]) {
    const record = subscription(); record.status.usage.totalBytes = totalBytes;
    const result = describeSubscriptionUsage(record.status.usage, now);
    assert.equal(result.percent, null); assert.equal(result.remaining, null);
    const card = subscriptionCardMarkup(record, null, now);
    assert.doesNotMatch(card, /role="progressbar"|不限量|无限/); assert.match(card, /额度未提供/);
  }
});
test("over-quota usage remains truthful but graphical progress and remaining are bounded", () => {
  const result = describeSubscriptionUsage(sample({ downloadBytes: 120 * GiB }), now);
  assert.equal(result.percent, 122); assert.equal(result.progress, 100); assert.equal(result.remaining, 0); assert.equal(result.exhausted, true);
});
test("expiry uses Unix seconds, zero is unknown, and exact expiration is expired", () => {
  assert.equal(describeSubscriptionUsage(sample({ expiresAt: now / 1000 }), now).expired, true);
  for (const expiresAt of [null, 0, -1, Number.MAX_SAFE_INTEGER]) assert.equal(describeSubscriptionUsage(sample({ expiresAt }), now).expires, "服务商未提供有效时间");
  assert.equal(subscriptionDate("invalid"), "尚未记录");
});
test("checked-at and usage sample time remain distinct after a metadata-less refresh", () => {
  const record = subscription(); record.status.usageUpdatedAt = "2026-09-01T00:00:00Z";
  const card = subscriptionCardMarkup(record, null, now);
  assert.match(card, /本次检查未获取新用量，保留上次采样/);
  assert.match(card, /最近检查/); assert.match(card, /用量采样/);
});
test("historical profiles without usage stay unknown rather than borrowing response bytes", () => {
  const record = subscription(); delete record.status;
  const card = subscriptionCardMarkup(record, null, now);
  assert.match(card, /流量信息未提供/); assert.doesNotMatch(card, /role="progressbar"/);
  assert.match(card, /刷新订阅以获取用量/);
});
test("current configuration nodes and provider counts are not advertised as usable nodes", () => {
  const card = subscriptionCardMarkup(subscription(), null, now);
  assert.match(card, /12 节点 · 2 提供器/); assert.doesNotMatch(card, /14 个可用|连接成功|健康|无限/);
  assert.match(card, /已选用/); assert.match(card, /配置校验通过/);
});
test("host/name/id and error content are escaped; no remote thumbnails or fake subscription URL", () => {
  const record = subscription(); record.profile.displayName = '<img src=x onerror="alert(1)">'; record.profile.id = '" onclick="bad';
  record.profile.source.host = '<script>bad</script>'; record.status.lastError = '<b>HTTP 403</b>';
  const card = subscriptionCardMarkup(record, null, now);
  assert.doesNotMatch(card, /<img|<script| onclick="|https:\/\/|token=/);
  assert.match(card, /&lt;img/); assert.match(card, /刷新失败 · &lt;b&gt;HTTP 403/);
});
test("all real subscription actions remain available and selecting an active profile is disabled", () => {
  const card = subscriptionCardMarkup(subscription(), null, now);
  for (const action of ["refresh", "activate", "versions", "delete", "openai-generate"]) assert.match(card, new RegExp(`data-subscription-action="${action}"`));
  assert.match(card, /data-subscription-action="activate"[^>]*disabled/);
  assert.match(card, /<details class="subscription-more"/);
  const main = read("main.ts"); assert.match(main, /title: "删除订阅"/); assert.match(main, /当前订阅正在使用，请先激活其他订阅后再删除/);
});
test("running disaster recovery generation keeps cancel action and blocks competing generation", () => {
  const task = { profileId: "fixture-only", running: true, completed: 1, total: 3 };
  assert.match(subscriptionCardMarkup(subscription(), task, now), /data-subscription-action="openai-cancel"/);
  assert.match(subscriptionCardMarkup(subscription(), { ...task, profileId: "other" }, now), /data-subscription-action="openai-generate"[^>]*disabled/);
});
test("compact cards use glass tokens, readable type and responsive columns without tiny labels", () => {
  const css = read("subscription-cards.css");
  assert.match(css, /repeat\(2, minmax\(0,1fr\)\)/); assert.match(css, /@container subscriptions/);
  assert.match(css, /prefers-reduced-transparency/); assert.match(css, /forced-colors/);
  assert.doesNotMatch(css, /font-size:\s*(?:[0-9]|1[012])px/);
});

test("one remote subscription form and one add entry; overview only navigates and local YAML stays", () => {
  const main = read("main.ts");
  assert.doesNotMatch(main, /quick-subscription|quick-url|id="subscription-form"|id="subscription-url"|新增远程订阅/);
  assert.equal((subscriptionImportMarkup.match(/<form\b/g) ?? []).length, 1);
  assert.equal((main.match(/id="subscriptions-add"/g) ?? []).length, 1);
  assert.match(main, /#overview-go-subscriptions.*navigate\("subscriptions"\)/);
  assert.match(main, /id="yaml-file"/); assert.match(main, /id="create-inline"/);
  assert.match(subscriptionImportMarkup, /id="managed-subscription-panel" hidden/);
  assert.match(subscriptionImportMarkup, /<details class="subscription-import-advanced"><summary>高级选项/);
  assert.match(subscriptionImportMarkup, /id="managed-subscription-activate" type="checkbox" \/>/);
  assert.match(read("api.ts"), /activateAfterImport = false/);
  assert.match(read("subscription-cards.css"), /\.subscription-import-card\[hidden\][^{]+\{ display: none; \}/);
});

const importResult = (changes = {}) => ({ profile: { id: "new-fixture" }, updated: true, created: true, activated: false, openAiGeneration: "not_requested", openAiError: null, observationError: null, ...changes });
test("import messages distinguish saving, explicit selection, duplicates and independent generation failures", () => {
  assert.match(describeSubscriptionImport(importResult()).text, /未切换当前配置/);
  assert.match(describeSubscriptionImport(importResult({ activated: true })).text, /添加并选用/);
  assert.match(describeSubscriptionImport(importResult({ created: false })).text, /未重复添加，也未更改/);
  assert.match(describeSubscriptionImport(importResult({ created: false, activated: true })).text, /未重新获取/);
  assert.match(describeSubscriptionImport(importResult({ openAiGeneration: "started" })).text, /任务已提交/);
  const failed = describeSubscriptionImport(importResult({ openAiGeneration: "failed", openAiError: "busy" }));
  assert.equal(failed.warning, true); assert.match(failed.text, /订阅已添加.*未能开始：busy/);
  assert.doesNotMatch(failed.text, /订阅添加失败|连接成功|正在筛选/);
  for (const openAiGeneration of ["not_requested", "started", "failed"]) {
    const observation = describeSubscriptionImport(importResult({ observationError: "检查记录未保存，请刷新列表后重试检查。", openAiGeneration }));
    assert.equal(observation.warning, true);
    assert.match(observation.text, /订阅已添加.*检查记录未保存/);
    assert.doesNotMatch(observation.text, /订阅添加失败/);
  }
});

function importControllerFixture(options = {}) {
  const source = ts.createSourceFile("main.ts", read("main.ts"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const declaration = source.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === "createSubscription");
  assert.ok(declaration);
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) elements.set(id, { textContent: "", value: "draft", hidden: false, disabled: false, className: "", setAttribute() {} });
    return elements.get(id);
  };
  const calls = [], notices = [];
  element("#managed-subscription-form").reset = () => { element("#managed-subscription-url").value = ""; calls.push("reset"); };
  const card = { dataset: { subscriptionId: "new-fixture" }, focus() { calls.push("focus"); }, scrollIntoView() { calls.push("scroll"); } };
  class FixtureElement {}
  const listeners = new Map();
  element("#managed-subscription-form").contains = () => false;
  const context = vm.createContext({
    $, console, Array, Error, HTMLElement: FixtureElement, viewNavigationRevision: 0,
    subscriptionImporting: false, subscriptionDraftDirty: true, subscriptionActivationTouched: true,
    highlightedSubscriptionId: null, store: { view: options.view ?? "subscriptions" },
    describeSubscriptionImport, errorMessage: error => error.message,
    api: { createSubscriptionProfile: async (...args) => { calls.push(args); if (options.wait) await options.wait; if (options.failure) throw new Error(options.failure); return options.result ?? importResult(); } },
    refreshBase: async () => { calls.push("refresh"); if (options.refreshFailure) throw new Error("refresh failed"); return options.refreshApplied !== false; },
    closeSubscriptionForm: () => { calls.push("close"); },
    toast: (text, tone) => notices.push({ text, tone }),
    document: { querySelectorAll: () => options.missingCard ? [] : [card], addEventListener: (key, listener) => listeners.set(key, listener), removeEventListener: key => listeners.delete(key) },
  });
  function $(id) { return element(id); }
  vm.runInContext(ts.transpileModule(declaration.getText(source), { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText, context);
  return { context, element, calls, notices, moveFocus: () => listeners.get("focusin")?.({ target: new FixtureElement() }), submit: (activate = false) => context.createSubscription("fixture", "https://new.example.invalid/subscription", "clash.meta", true, activate) };
}
test("actual import controller transmits explicit false/true, resets only on success and focuses its card", async () => {
  for (const activate of [false, true]) {
    const f = importControllerFixture({ result: importResult({ activated: activate }) });
    await f.submit(activate);
    assert.equal(f.calls[0][4], activate);
    assert.equal(f.element("#managed-subscription-url").value, "");
    assert.ok(f.calls.includes("close")); assert.ok(f.calls.includes("focus"));
    assert.equal(f.context.subscriptionImporting, false);
    assert.equal(f.element("#managed-subscription-fields").disabled, false);
  }
});
test("HTTP 403 keeps the draft, displays the actual error and permits retry", async () => {
  const f = importControllerFixture({ failure: "SUBSCRIPTION_ERROR: HTTP 403" });
  await f.submit();
  assert.equal(f.element("#managed-subscription-url").value, "draft");
  assert.equal(f.context.subscriptionDraftDirty, true);
  assert.match(f.element("#managed-subscription-import-status").textContent, /HTTP 403/);
  assert.equal(f.calls.includes("close"), false); assert.equal(f.context.subscriptionImporting, false);
  assert.equal(f.element("#managed-subscription-fields").disabled, false);
});
test("double submission is ignored while fields are locked; selection input cannot change midflight", async () => {
  let complete; const wait = new Promise(resolve => { complete = resolve; });
  const f = importControllerFixture({ wait });
  const pending = f.submit(false);
  assert.equal(f.element("#managed-subscription-fields").disabled, true);
  await f.submit(true);
  assert.equal(f.calls.filter(Array.isArray).length, 1);
  complete(); await pending;
  assert.equal(f.calls[0][4], false);
});
test("post-save refresh failures never call the completed import a failure or retain a resubmittable URL", async () => {
  for (const options of [{ refreshFailure: true }, { missingCard: true }, { refreshApplied: false }, { refreshApplied: false, result: importResult({ created: false }) }]) {
    const f = importControllerFixture(options); await f.submit();
    assert.equal(f.element("#managed-subscription-url").value, "");
    assert.match(f.element("#subscriptions-feedback").textContent, /(?:订阅已添加|该订阅已存在).*无需重复添加/);
    assert.equal(f.notices.some(notice => notice.tone === "error"), false);
  }
});
test("leaving then returning or focusing another control during import cancels automatic card focus", async () => {
  for (const changeFocus of [f => { f.context.viewNavigationRevision += 2; }, f => f.moveFocus()]) {
    let complete; const wait = new Promise(resolve => { complete = resolve; });
    const f = importControllerFixture({ wait }); const pending = f.submit();
    changeFocus(f); complete(); await pending;
    assert.equal(f.calls.includes("focus"), false);
  }
});
test("default selection requires an authoritative empty active profile; unknown or failed first reads are save-only", () => {
  const source = ts.createSourceFile("main.ts", read("main.ts"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const declaration = source.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === "openSubscriptionForm");
  for (const [appInfo, activeProfile, expected] of [[null, null, false], [{}, null, true], [{}, { profile: {} }, false]]) {
    const checkbox = { checked: false }, panel = { hidden: true };
    const context = vm.createContext({ Boolean, subscriptionImporting: false, subscriptionDraftDirty: false, subscriptionActivationTouched: false,
      store: { appInfo, activeProfile, view: "overview" }, renderSubscriptionActivationHint() {},
      $: id => id === "#managed-subscription-activate" ? checkbox : id === "#managed-subscription-panel" ? panel : { setAttribute() {} } });
    vm.runInContext(ts.transpileModule(declaration.getText(source), { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText, context);
    context.openSubscriptionForm(false); assert.equal(checkbox.checked, expected); assert.equal(panel.hidden, false);
  }
});
test("generation failure stays a saved-subscription warning and does not steal focus after navigation", async () => {
  const f = importControllerFixture({ view: "overview", result: importResult({ openAiGeneration: "failed", openAiError: "task busy" }) });
  await f.submit();
  assert.equal(f.calls.includes("focus"), false);
  assert.match(f.element("#subscriptions-feedback").textContent, /订阅已添加.*task busy/);
  assert.equal(f.element("#subscriptions-feedback").className, "subscription-feedback is-warning");
  assert.equal(f.notices[0].tone, "info");
});
