import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

const source = readFileSync(new URL("./fixtures/theme-preview.ts", import.meta.url), "utf8");
// Execute the real fixture's state and IPC handlers, but never import the app,
// browser transport or Tauri. Browser rendering is covered separately by IAB.
const prefix = source.slice(0, source.indexOf("// Supply a stable, mutable color-scheme"));
assert.ok(prefix.length > 0);
const ast = ts.createSourceFile("theme-preview.ts", prefix, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
const executable = ts.transpileModule(ast.statements.filter(statement => !ts.isImportDeclaration(statement))
  .map(statement => statement.getFullText(ast)).join("\n"), {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
}).outputText;

function fixture(scenario = "default", options = {}) {
  const elements = new Map(), listeners = new Map(), timers = [];
  let invoke;
  let browserNetworkCalls = 0;
  const element = id => {
    if (!elements.has(id)) elements.set(id, { value: "", textContent: "", dataset: {}, clicks: 0,
      setAttribute() {}, addEventListener() {}, click() { this.clicks++; } });
    return elements.get(id);
  };
  const context = {
    aiRoutePresets: JSON.parse(readFileSync(new URL('../src-tauri/ai-route-presets.json', import.meta.url), 'utf8')),
    performance: globalThis.performance, emit: async () => {},
    mockIPC: handler => { invoke = handler; }, packageInfo: { version: "0.0.0-fixture" },
    location: { search: `?${new URLSearchParams({ subscriptionScenario: scenario, ...options })}` },
    localStorage: { getItem: () => null, setItem: () => { throw new Error("unexpected fixture persistence"); } },
    window: {
      addEventListener: (name, handler) => listeners.set(name, handler),
      setTimeout: callback => { timers.push(callback); return timers.length; },
    },
    document: { documentElement: { dataset: {} }, getElementById: element,
      querySelector: selector => selector.startsWith("#") ? element(selector.slice(1)) : null },
    URL, URLSearchParams, TextEncoder, structuredClone,
    fetch: () => { browserNetworkCalls++; throw new Error("network forbidden"); },
  };
  vm.createContext(context);
  vm.runInContext(executable, context, { timeout: 5000 });
  assert.equal(typeof invoke, "function");
  const flush = async () => {
    while (timers.length) timers.shift()();
    await Promise.resolve();
  };
  return {
    element, context, flush,
    invoke: (command, args) => invoke(command, args),
    async call(command, args) { const result = invoke(command, args); await flush(); return result; },
    event(detail) { listeners.get("routedeck-fixture-subscriptions")({ detail }); },
    gate(detail) { listeners.get("serylane-fixture-subscription-import")?.({ detail }); },
    buttonGate(detail) { listeners.get("serylane-fixture-button-feedback")?.({ detail }); },
    qa: () => JSON.parse(context.document.documentElement.dataset.fixtureSubscriptionImportQa ?? "null"),
    state: () => JSON.parse(context.document.documentElement.dataset.fixtureSubscriptionImportState),
    profiles: () => JSON.parse(context.document.documentElement.dataset.fixtureSubscriptionsState),
    calls: () => JSON.parse(context.document.documentElement.dataset.fixtureSubscriptionsCalls),
    trace: () => JSON.parse(context.document.documentElement.dataset.fixtureSubscriptionImportCalls),
    networkCalls: () => browserNetworkCalls,
    runtimeCalls: () => JSON.parse(context.document.documentElement.dataset.fixtureRuntimeCalls),
  };
}

const input = overrides => ({ displayName: "新订阅 · 合成", url: "https://new.example.invalid/subscription", userAgent: "fixture-only", generateOpenAi: false, activateAfterImport: false, ...overrides });
const duplicate = overrides => input({ url: "https://duplicate.example.invalid/subscription", ...overrides });
const create = (f, args = input()) => f.call("create_subscription_profile", args);
function isolated(f) {
  assert.equal(f.networkCalls(), 0);
  assert.deepEqual(f.runtimeCalls(), []);
  assert.equal(f.element("fixture-network-mutation-count").textContent || "0", "0");
}

test("default/cards/empty fixtures remain fail-closed for subscription creation", async () => {
  for (const [scenario, count] of [["default", 2], ["cards", 4], ["empty", 0], ["import-unknown", 2]]) {
    const f = fixture(scenario);
    assert.equal(f.profiles().length, count);
    assert.equal(f.state().enabled, false);
    await assert.rejects(create(f), /FIXTURE_ONLY/);
    assert.equal(f.profiles().length, count);
    assert.equal(f.calls().create, 0);
    assert.equal(f.networkCalls(), 0);
  }
});

test("new unchecked import saves a profile without changing the current selection", async () => {
  const f = fixture("import");
  const result = await create(f);
  assert.equal(result.created, true);
  assert.equal(result.updated, true);
  assert.equal(result.activated, false);
  assert.equal(result.openAiGeneration, "not_requested");
  assert.equal(result.openAiError, null);
  assert.equal(result.profile.source.host, "new.example.invalid");
  assert.equal(result.revision.validation.nativeCoreValidated, false, "fixture does not claim native validation");
  assert.equal(f.profiles().length, 3);
  assert.equal(f.state().activeProfileId, "fixture-active");
  assert.equal(f.state().busy, false);
  isolated(f);
});

test("explicit selection chooses the newly saved synthetic profile but starts nothing", async () => {
  const f = fixture("import");
  const result = await create(f, input({ activateAfterImport: true }));
  assert.equal(result.created, true);
  assert.equal(result.activated, true);
  assert.equal(f.state().activeProfileId, result.profile.id);
  assert.deepEqual(f.trace(), [{ host: "new.example.invalid", activateAfterImport: true, generateOpenAi: false }]);
  isolated(f);
});

test("first import obeys the explicit checked value rather than forcing activation", async () => {
  for (const activateAfterImport of [true, false]) {
    const f = fixture("import-empty");
    assert.equal(f.state().activeProfileId, null);
    const result = await create(f, input({ activateAfterImport }));
    assert.equal(result.created, true);
    assert.equal(result.activated, activateAfterImport);
    assert.equal(f.state().activeProfileId, activateAfterImport ? result.profile.id : null);
    assert.equal(f.profiles().length, 1);
    isolated(f);
  }
});

test("HTTP 403 leaves profiles, selection and the form draft intact", async () => {
  const f = fixture("import-403");
  const before = f.profiles();
  f.element("managed-subscription-url").value = "https://new.example.invalid/subscription?token=fixture-only";
  f.element("managed-subscription-name").value = "保留这个草稿";
  await assert.rejects(create(f, input({ url: f.element("managed-subscription-url").value, activateAfterImport: true })), error => {
    assert.equal(error.code, "SUBSCRIPTION_ERROR");
    assert.match(error.message, /HTTP 403/);
    assert.doesNotMatch(error.message, /token|new\.example/);
    return true;
  });
  assert.deepEqual(f.profiles(), before);
  assert.equal(f.state().activeProfileId, "fixture-active");
  assert.equal(f.state().busy, false);
  assert.equal(f.element("managed-subscription-name").value, "保留这个草稿");
  assert.match(f.element("managed-subscription-url").value, /fixture-only/);
  isolated(f);
});

test("duplicate URL without selection is read-only and does not refresh or rename", async () => {
  const f = fixture("import-duplicate");
  const before = f.profiles();
  const result = await create(f, duplicate({ displayName: "不应覆盖旧名称", userAgent: "must-not-replace" }));
  assert.equal(result.profile.id, "fixture-inactive");
  assert.equal(result.created, false);
  assert.equal(result.updated, false);
  assert.equal(result.activated, false);
  assert.deepEqual(f.profiles(), before);
  assert.equal(f.state().activeProfileId, "fixture-active");
  assert.equal(f.calls().refresh, 0);
  isolated(f);
});

test("explicitly selecting a duplicate reuses the saved version without refreshing", async () => {
  const f = fixture("import-duplicate");
  const result = await create(f, duplicate({ activateAfterImport: true }));
  assert.equal(result.created, false);
  assert.equal(result.activated, true);
  assert.equal(result.profile.id, "fixture-inactive");
  assert.equal(result.revision.id, "fixture-inactive-revision");
  assert.equal(f.state().activeProfileId, "fixture-inactive");
  assert.equal(f.profiles().length, 2);
  assert.equal(f.calls().refresh, 0);
  isolated(f);
});

test("an unselected duplicate never starts OpenAI generation or changes the existing task", async () => {
  for (const existingTask of [false, true]) {
    const f = fixture("import-duplicate");
    if (existingTask) await create(f, input({ generateOpenAi: true }));
    const beforeProfiles = f.profiles();
    const beforeTask = await f.call("get_openai_policy_task");
    const result = await create(f, duplicate({ activateAfterImport: false, generateOpenAi: true }));
    assert.equal(result.created, false);
    assert.equal(result.activated, false);
    assert.equal(result.openAiGeneration, "not_requested");
    assert.equal(result.openAiError, null);
    assert.deepEqual(await f.call("get_openai_policy_task"), beforeTask);
    assert.deepEqual(f.profiles(), beforeProfiles);
    assert.equal(f.state().activeProfileId, "fixture-active");
    assert.equal(f.calls().refresh, 0);
    isolated(f);
  }
});

test("repeating a freshly saved URL creates no second profile", async () => {
  const f = fixture("import");
  const first = await create(f);
  const second = await create(f);
  assert.equal(second.created, false);
  assert.equal(second.profile.id, first.profile.id);
  assert.equal(f.profiles().length, 3);
  assert.equal(f.state().activeProfileId, "fixture-active");
  isolated(f);
});

test("OpenAI submission failure is a partial success with the subscription still saved", async () => {
  const f = fixture("import-openai-failed");
  const result = await create(f, input({ generateOpenAi: true }));
  assert.equal(result.created, true);
  assert.equal(result.openAiGeneration, "failed");
  assert.match(result.openAiError, /订阅已保存/);
  assert.equal(f.profiles().length, 3);
  assert.equal(f.state().activeProfileId, "fixture-active");
  const task = await f.call("get_openai_policy_task");
  assert.equal(task.phase, "failed");
  assert.equal(task.running, false);
  assert.equal(task.profileId, result.profile.id);
  isolated(f);
});

test("requested OpenAI task reports synthetic started state, never fake completed nodes", async () => {
  const f = fixture("import");
  const result = await create(f, input({ generateOpenAi: true }));
  assert.equal(result.openAiGeneration, "started");
  const task = await f.call("get_openai_policy_task");
  assert.equal(task.running, true);
  assert.equal(task.phase, "preparing");
  assert.equal(task.completed, 0);
  assert.equal(task.result, null);
  assert.equal(result.profile.openaiPolicy.selectedNodes.length, 0);
  isolated(f);
});

test("concurrent imports are deduplicated across the real asynchronous fixture boundary", async () => {
  const f = fixture("import");
  const first = f.invoke("create_subscription_profile", input());
  assert.equal(f.state().busy, true);
  await assert.rejects(f.invoke("create_subscription_profile", input()), error => error.code === "STATE_CONFLICT");
  await f.flush();
  assert.equal((await first).created, true);
  assert.equal(f.calls().create, 1);
  assert.equal(f.profiles().length, 3);
  isolated(f);
});

test("changing scenarios during import cannot resurrect the previous profile list", async () => {
  const f = fixture("import");
  const pending = f.invoke("create_subscription_profile", input());
  f.event({ scenario: "empty", refresh: true });
  await f.flush();
  await assert.rejects(pending, error => error.code === "STATE_CONFLICT");
  assert.equal(f.profiles().length, 0);
  assert.equal(f.state().activeProfileId, null);
  assert.equal(f.state().enabled, false);
  assert.equal(f.element("subscriptions-refresh-list").clicks, 1);
  isolated(f);
});

test("unsafe URLs and implicit activation never enter the synthetic import", async () => {
  const f = fixture("import");
  for (const args of [input({ url: "https://real-provider.example/sub?token=secret" }), input({ url: "https://user:secret@new.example.invalid/sub" }), input({ activateAfterImport: undefined })]) {
    await assert.rejects(create(f, args), error => {
      assert.equal(error.code, "INVALID_INPUT");
      assert.doesNotMatch(error.message, /secret|real-provider/);
      return true;
    });
  }
  assert.equal(f.calls().create, 0);
  assert.deepEqual(f.trace(), []);
  assert.equal(f.profiles().length, 2);
  isolated(f);
});

test("opt-in raw counts expose duplicate IPC without recording a URL or token", async () => {
  const f = fixture("import", { subscriptionImportQa: "1" });
  const inputWithSecret = input({ url: "https://new.example.invalid/private-path?token=fixture-secret", taskId: "fixture-test-task" });
  f.gate({ action: "arm", stage: "before_commit" });
  const pending = f.invoke("create_subscription_profile", inputWithSecret);
  await f.flush();
  assert.deepEqual(f.qa(), { create: 1, cancel: 0, holds: ["before_commit"] });
  assert.equal(f.profiles().length, 2);
  await assert.rejects(f.invoke("create_subscription_profile", inputWithSecret), error => error.code === "STATE_CONFLICT");
  assert.deepEqual(f.qa(), { create: 2, cancel: 0, holds: ["before_commit"] });
  assert.equal(f.calls().create, 1);
  const report = JSON.stringify(f.context.document.documentElement.dataset);
  assert.doesNotMatch(report, /private-path|token=|fixture-secret/);
  f.gate({ action: "release", stage: "before_commit" });
  assert.equal((await pending).created, true);
  assert.deepEqual(f.qa(), { create: 2, cancel: 0, holds: [] });
  assert.equal(f.profiles().length, 3);
  assert.equal(f.state().activeProfileId, "fixture-active");
  assert.doesNotMatch(JSON.stringify(f.context.document.documentElement.dataset), /private-path|token=|fixture-secret/);
  isolated(f);
});

test("a held cancel has no effect until released and cannot produce an imported profile", async () => {
  const f = fixture("import", { subscriptionImportQa: "1" });
  const taskId = "fixture-cancel-task";
  for (const stage of ["before_commit", "cancel_request"]) f.gate({ action: "arm", stage });
  const pending = f.invoke("create_subscription_profile", input({ taskId }));
  const cancelled = assert.rejects(pending, error => {
    assert.equal(error.code, "SUBSCRIPTION_ERROR");
    assert.match(error.message, /已取消订阅任务/);
    return true;
  });
  await f.flush();
  assert.equal((await f.call("subscription_task_status", { taskId })).canCancel, true);
  const cancel = f.invoke("cancel_subscription_task", { taskId });
  assert.deepEqual(f.qa(), { create: 1, cancel: 1, holds: ["before_commit", "cancel_request"] });
  assert.equal((await f.call("subscription_task_status", { taskId })).canCancel, true);
  f.gate({ action: "release", stage: "cancel_request" });
  assert.equal(await cancel, true);
  assert.equal((await f.call("subscription_task_status", { taskId })).phase, "cancelled");
  f.gate({ action: "release", stage: "before_commit" });
  await cancelled;
  assert.equal(f.profiles().length, 2);
  assert.equal(f.state().busy, false);
  assert.deepEqual(f.qa(), { create: 1, cancel: 1, holds: [] });
  isolated(f);
});

test("gated pre-commit failure preserves previous state and a committed receipt is not cancellable", async () => {
  const f = fixture("import", { subscriptionImportQa: "1" });
  const failedTask = "fixture-fail-task";
  f.gate({ action: "arm", stage: "before_commit" });
  const rejected = assert.rejects(f.invoke("create_subscription_profile", input({ taskId: failedTask })), error => {
    assert.equal(error.code, "SUBSCRIPTION_ERROR");
    assert.match(error.message, /未保存订阅/);
    return true;
  });
  await f.flush();
  f.gate({ action: "release", stage: "before_commit", outcome: "fail" });
  await rejected;
  assert.equal(f.profiles().length, 2);
  assert.equal((await f.call("subscription_task_status", { taskId: failedTask })).phase, "failed");

  const taskId = "fixture-saved-task";
  f.gate({ action: "arm", stage: "after_commit" });
  const saved = f.invoke("create_subscription_profile", input({ taskId }));
  await f.flush();
  assert.deepEqual(f.qa(), { create: 2, cancel: 0, holds: ["after_commit"] });
  assert.equal(f.profiles().length, 3);
  const task = await f.call("subscription_task_status", { taskId });
  assert.equal(task.phase, "committing");
  assert.equal(task.canCancel, false);
  assert.equal(await f.call("cancel_subscription_task", { taskId }), false);
  assert.deepEqual(f.qa(), { create: 2, cancel: 1, holds: ["after_commit"] });
  f.gate({ action: "release", stage: "after_commit", outcome: "fail" });
  assert.equal((await saved).created, true);
  assert.equal(f.state().busy, false);
  assert.deepEqual(f.qa(), { create: 2, cancel: 1, holds: [] });
  isolated(f);
});

test("subscription import gates are inert without opt-in and list hold never records arguments", async () => {
  const baseline = fixture("import");
  baseline.gate({ action: "arm", stage: "before_commit" });
  assert.equal((await create(baseline)).created, true);
  assert.equal(baseline.qa(), null);
  isolated(baseline);

  const f = fixture("import", { buttonFeedback: "1", subscriptionImportQa: "1" });
  const dataset = f.context.document.documentElement.dataset;
  f.buttonGate({ action: "arm", command: "list_subscriptions", outcome: "hold" });
  const pending = f.invoke("list_subscriptions", { url: "https://new.example.invalid/private-path?token=fixture-secret" });
  const feedback = JSON.parse(dataset.fixtureButtonFeedback);
  assert.deepEqual(feedback.holds, [1]);
  assert.deepEqual(feedback.calls[0].args, {});
  assert.doesNotMatch(JSON.stringify(dataset), /private-path|token=|fixture-secret/);
  f.buttonGate({ action: "release", command: "list_subscriptions", outcome: "success" });
  assert.equal((await pending).length, 2);
  isolated(f);
});

test("unknown native commands are still blocked in an opted-in import scenario", async () => {
  const f = fixture("import");
  await assert.rejects(f.call("start_active_profile"), /FIXTURE_ONLY/);
  await assert.rejects(f.call("create_inline_profile", { source: "" }), /FIXTURE_ONLY/);
  assert.equal(f.networkCalls(), 0);
  assert.deepEqual(f.runtimeCalls(), []);
});
