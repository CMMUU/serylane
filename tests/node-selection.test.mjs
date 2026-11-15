import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import ts from "typescript";
import vm from "node:vm";
const source = readFileSync(new URL("../src/node-selection.ts", import.meta.url), "utf8");
const js = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText;
const { generalGroups, resolvedNode, nodeSelectionMarkup, currentNodeMarkup, canRestoreAuto, openAiRecommendations, openAiRecommendationMarkup, aiNodeBlockReason, isConfirmedAiChoice, OPENAI_GROUP } = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);
const map = { PROXY: { type: "Selector", now: "AUTO", all: ["AUTO", "B"] }, AUTO: { type: "URLTest", now: "B", all: ["B"] }, B: { type: "Trojan", trafficMultiplier: 1 }, GLOBAL: { type: "Selector", now: "PROXY", all: ["PROXY"] }, [OPENAI_GROUP]: { type: "Selector", now: "B", all: ["B"] } };
test("ordinary selection is independent of OpenAI, direct and global are explicit", () => {
  assert.deepEqual(generalGroups(map, "rule"), ["PROXY", "AUTO"]);
  const ordinary = structuredClone(map); delete ordinary[OPENAI_GROUP];
  assert.deepEqual(generalGroups(ordinary, "rule"), ["PROXY", "AUTO"]);
  assert.deepEqual(generalGroups(map, "direct"), []);
  assert.deepEqual(generalGroups(map, "global"), ["GLOBAL"]);
});
test("resolve nested groups and fail closed for cycles, missing and distributed routes", () => {
  assert.deepEqual(resolvedNode(map, "PROXY"), { name: "B", chain: ["PROXY", "AUTO", "B"] });
  for (const fixture of [{ A: { all: ["A"], now: "A" } }, { A: { all: ["missing"], now: "missing" } }, { A: { all: ["B"] } }, { A: { type: "LoadBalance", all: ["B"], now: "B" }, B: {} }]) assert.equal(resolvedNode(fixture, "A").name, null);
});
test("manual choice has explicit apply, fixed and recover-auto states", () => {
  const group = { type: "Selector", all: ["B"], now: "B", manualNode: "B" };
  assert.match(nodeSelectionMarkup(OPENAI_GROUP, group, false), /自动切换已暂停/);
  assert.match(nodeSelectionMarkup(OPENAI_GROUP, group, false), /恢复自动/);
  assert.match(nodeSelectionMarkup(OPENAI_GROUP, group, true), /disabled/);
  assert.equal(canRestoreAuto("PROXY", group), false);
  assert.match(nodeSelectionMarkup("AUTO", { ...group, type: "Fallback", manualNode: null, fixed: "B" }, false), /失效时由核心回退/);
  assert.match(nodeSelectionMarkup(OPENAI_GROUP, { ...group, manualNode: "removed" }, false), /原手动节点已不在候选中/);
  assert.match(nodeSelectionMarkup("LB", { type: "LoadBalance", all: ["B"] }, false), /不支持单节点手动选择/);
});
test("no eligible current choice disables only Apply and a valid draft enables it", () => {
  const group = { type: "Selector", all: ["blocked", "eligible"], now: "blocked", manualNode: "blocked" };
  const prices = { blocked: { metadata: { eligible: false, regionReason: "地区待确认" } }, eligible: { metadata: { eligible: true } } };
  const markup = draft => nodeSelectionMarkup(OPENAI_GROUP, group, false, group.all, prices, undefined, { draft });
  const apply = html => html.match(/<button type="submit"[^>]*>/)?.[0];
  const restore = html => html.match(/<button type="button"[^>]*data-node-auto=[^>]*>/)?.[0];
  assert.match(apply(markup(undefined)), /disabled/);
  assert.doesNotMatch(restore(markup(undefined)), /disabled/);
  assert.match(markup(undefined), /请先选择一个可用节点/);
  assert.match(markup(undefined), /value="blocked" selected disabled/);
  assert.match(markup("old candidate"), /value="" selected disabled/);
  assert.match(apply(markup("old candidate")), /disabled/);
  assert.match(markup("eligible"), /value="eligible" selected/);
  assert.doesNotMatch(apply(markup("eligible")), /disabled/);
  assert.doesNotMatch(markup("eligible"), /请先选择一个可用节点/);
  const none = nodeSelectionMarkup(OPENAI_GROUP, group, false, ["blocked", "stale"], { ...prices, stale: prices.eligible });
  assert.match(none, /暂无可手动选用的节点/);
  assert.match(none, /value="stale"\s+disabled/);
});
test("only the read-back exit and matching AI manual intent produce a disabled Applied state", () => {
  const base = { type: "Selector", now: "B", all: ["A", "B"], manualNode: "B" };
  const prices = { A: { metadata: { eligible: true } }, B: { metadata: { eligible: true } } };
  const button = (group, data, draft = "B", pending) =>
    nodeSelectionMarkup(group, data, false, data.all, prices, undefined, { draft, pending })
      .match(/<button type="submit"[^>]*>[^<]*<\/button>/)?.[0] ?? "";
  for (const data of [base, { ...base, type: "Fallback", manualNode: null, fixed: "B" }, { ...base, type: "URLTest", manualNode: null, fixed: "B" }]) {
    const applied = button(OPENAI_GROUP, data);
    assert.match(applied, /data-ai-choice-applied[^>]*aria-busy="false"[^>]*disabled>已应用/);
    assert.doesNotMatch(applied, /button-primary/);
  }
  for (const data of [
    { ...base, manualNode: null }, // current automatic choice can still be pinned
    { ...base, now: "A" }, // stored intent is not the running exit
    { ...base, now: undefined }, // no controller readback
    { ...base, aiChoiceUnverified: true },
    { ...base, type: "Fallback", manualNode: null, fixed: "B", now: "A" },
    { ...base, type: "URLTest", manualNode: null, fixed: true },
  ]) {
    assert.match(button(OPENAI_GROUP, data), /button-primary[^>]*>应用节点/);
    assert.doesNotMatch(button(OPENAI_GROUP, data), /disabled|data-ai-choice-applied/);
  }
  assert.doesNotMatch(button(OPENAI_GROUP, base, "A"), /disabled|data-ai-choice-applied/);
  assert.doesNotMatch(button("PROXY", base), /disabled|data-ai-choice-applied/);
  assert.match(button(OPENAI_GROUP, base, "B", { node: "B", phase: "write" }), /aria-busy="true"[^>]*disabled>正在应用节点/);
  const restore = nodeSelectionMarkup(OPENAI_GROUP, base, false, base.all, prices).match(/<button[^>]*data-node-auto[^>]*>/)?.[0];
  assert.ok(restore);
  assert.doesNotMatch(restore, /disabled/);
});

test("only the invoked picker control shows busy while write and read-back are pending", () => {
  const group = { type: "Selector", all: ["A", "B"], now: "A", manualNode: "A" };
  const prices = { A: { metadata: { eligible: true } }, B: { metadata: { eligible: true } } };
  const markup = (node, phase, source = group) => nodeSelectionMarkup(OPENAI_GROUP, source, false, group.all, prices, undefined, { pending: { node, phase } });
  const write = markup("B", "write");
  assert.match(write, /data-render-key="node-choice-apply"[^>]*aria-busy="true"[^>]*disabled>正在应用节点…/);
  assert.match(write, /data-render-key="node-choice-auto"[^>]*aria-busy="false"[^>]*disabled>恢复自动/);
  assert.match(write, /value="B" selected/);
  assert.match(markup("B", "verify"), /data-render-key="node-choice-apply"[^>]*aria-busy="true"[^>]*disabled>正在核对出口…/);
  const restore = markup(null, "verify", { ...group, manualNode: null });
  assert.match(restore, /data-render-key="node-choice-apply"[^>]*aria-busy="false"[^>]*disabled>应用节点/);
  assert.match(restore, /data-render-key="node-choice-auto"[^>]*aria-busy="true"[^>]*disabled>正在核对出口…/);
});
test("overview never invents online state or shows stale details for another node", () => {
  const html = currentNodeMarkup("普通代理", "PROXY", map, { nodeName: "OLD", maskedServer: "should-not-appear", alive: true, routeChain: ["PROXY", "OLD"] });
  assert.match(html, /尚无有效检测/); assert.doesNotMatch(html, /should-not-appear|在线/);
  assert.match(html, /PROXY → AUTO → B/);
});
test("names are escaped in labels, attributes, options and details", () => {
  const name = 'x"><img src=x onerror=alert(1)>';
  const html = nodeSelectionMarkup(name, { type: "Selector", all: [name], now: name }, false);
  assert.doesNotMatch(html, /<img/); assert.match(html, /&quot;&gt;&lt;img/);
});
test("recommendations follow only the matching live revision and distinguish current use from rank", () => {
  const profile = {
    id: "subscription-1", activeRevisionId: "r1",
    openaiPolicy: { enabled: true, benchmarkVersion: 2, selectedNodes: [{ name: "A<&", score: 80, connectionSamples: 4 }, { name: "B", score: 78 }] },
  };
  const snapshot = { profileId: profile.id, revisionId: "r1", running: true, eligible: true, lastCheck: 1, nodes: [
    { name: "A<&", rank: 2, recommendationScore: 68, probeOk: true, streamSamples: 0, connectionSamples: 4 },
    { name: "B", rank: 1, recommendationScore: 90, probeOk: false, cooldownSeconds: 20, streamSamples: 2 },
  ] };
  const live = openAiRecommendations(profile, snapshot, ["A<&", "B"]);
  assert.deepEqual(live.map(row => row.name), ["B", "A<&"]);
  const markup = openAiRecommendationMarkup(live, "A<&");
  assert.match(markup, /1\.[\s\S]+>B<[\s\S]+冷却中 · 暂停自动选点/);
  assert.match(markup, /2\.[\s\S]+A&lt;&amp;<[\s\S]+当前使用/);
  assert.match(markup, /真实模型流待验证（2 条样本）/);
  const picker = nodeSelectionMarkup(OPENAI_GROUP, { type: "Selector", all: ["A<&", "B"], now: "A<&" }, false, live.map(row => row.name), undefined, live);
  assert.match(picker, /value="B"[^>]*>1\. B · 冷却中/);
  assert.match(picker, /value="A&lt;&amp;" selected[^>]*>2\. A&lt;&amp;/);
  for (const stale of [{ ...snapshot, revisionId: "old" }, { ...snapshot, profileId: "other" }, { ...snapshot, lastCheck: null }]) {
    assert.deepEqual(openAiRecommendations(profile, stale).map(row => [row.name, row.rank, row.streamSamples]), [["A<&", 1, 0], ["B", 2, 0]]);
  }
  assert.deepEqual(openAiRecommendations({ ...profile, openaiPolicy: { ...profile.openaiPolicy, benchmarkVersion: 1 } }, snapshot), []);
});
test("manual intent is checked again after probes and Selector auto bypasses DELETE", () => {
  const stability = readFileSync(new URL("../src-tauri/src/openai_stability.rs", import.meta.url), "utf8");
  const selection = readFileSync(new URL("../src-tauri/src/node_selection.rs", import.meta.url), "utf8");
  assert.ok(stability.indexOf("openai_manual_node", stability.indexOf(".collect::<Vec<_>>()")) > 0);
  assert.match(selection, /if node\.is_none\(\) && managed\s*\{[\s\S]*?Ok\(ProxyWriteState::Accepted\)/);
  assert.doesNotMatch(selection, /close_connection|reload_config|stop_runtime/);
  assert.match(selection, /state\.active_profile_id == Some\(id\)\s*&& state\.active_revision_id == Some\(revision\)\s*&& running_identity\(app\) == Some\(runtime\)/);
  assert.ok((selection.match(/scope_current\(app, &storage, id, revision, runtime\)/g) ?? []).length >= 3);
});

const deferred = () => {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { resolve, promise };
};
const flush = () => new Promise(done => setImmediate(done));

function controllerFixture(names) {
  const calls = [];
  const profile = { id: "one", activeRevisionId: "r1", routingMode: "rule", openaiPolicy: { enabled: true, stabilityEnabled: true } };
  const fixtureMap = structuredClone(map);
  fixtureMap.A = { type: "Trojan", trafficMultiplier: 1, metadata: { eligible: true } };
  fixtureMap.B.metadata = { eligible: true };
  Object.assign(fixtureMap[OPENAI_GROUP], { now: "A", manualNode: "A", all: ["A", "B"] });
  const snapshot = { profileId: "one", revisionId: "r1", modeRevision: "m1", proxies: fixtureMap };
  const readCoordinator = readFileSync(new URL("../src/read-coordinator.ts", import.meta.url), "utf8");
  const coordinatorAst = ts.createSourceFile("read-coordinator.ts", readCoordinator, ts.ScriptTarget.Latest, true);
  const pageReads = coordinatorAst.statements.find(statement => ts.isClassDeclaration(statement) && statement.name?.text === "PageReads").getText(coordinatorAst).replace(/^export /, "");
  const context = {
    store: { view: "proxies", activeProfile: { profile }, runtime: { phase: "running", pid: 1 }, manualOutbound: {mode:"ai",revision:"m1"}, proxies: snapshot, proxySnapshotRuntime: JSON.stringify([1, null]), openAiTask: null },
    manualModeSwitching: false, nodeSelectionBusy: false, nodeSelectionPending: null, aiNodeDraft: null,
    runtimeActionInFlight: false, networkModeSwitching: false, settingsSaving: false,
    proxySelectionReadRevision: 0, proxyReadSequence: 0, runtimeMutationRevision: 0, viewNavigationRevision: 0,
    overviewNodeDetails: {}, proxyReadError: "", OPENAI_GROUP_NAME: OPENAI_GROUP, aiNodeBlockReason, isConfirmedAiChoice,
    confirmAction: async ({ title }) => { calls.push(["confirm", title]); return true; },
    openNodeDetails: async group => { calls.push(["details", group]); },
    api: {
      selectProxy: async (...args) => { calls.push(["select", ...args]); return receipt({ group: args[0], now: args[1], manualNode: args[1] }); },
      clearProxySelection: async (...args) => { calls.push(["auto", ...args]); return receipt({ group: args[0], manualNode: null }); },
      proxies: async () => snapshot,
    },
    toast: (...args) => calls.push(["toast", ...args]), errorMessage: e => e.message,
    refreshProxies: async quiet => { calls.push(["refresh", quiet]); },
    renderProxies() { calls.push(["render", context.nodeSelectionPending?.phase ?? null]); },
    renderOverviewNodes() {},
  };
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const ast = ts.createSourceFile("main.ts", main, ts.ScriptTarget.Latest, true);
  const selected = ["pageContext", "dataContext", "nodeSelectionContext", "runningProxyIdentity", "currentAiProxySnapshot", "currentNodeSelection", ...names];
  const functions = ast.statements.filter(statement => ts.isFunctionDeclaration(statement) && selected.includes(statement.name?.text)).map(statement => statement.getText(ast)).join("\n");
  vm.createContext(context);
  vm.runInContext(ts.transpileModule(pageReads + "\nconst pageReads = new PageReads();\n" + functions, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText, context);
  return { context, calls };
}
const receipt = (override = {}) => ({ writeState: "accepted", group: OPENAI_GROUP, groupType: "Selector", now: "B", fixed: null, manualNode: "B", ...override });

test("AI apply is one explicit action, deduplicates, and completes without waiting for a full refresh", async () => {
  const { context: c, calls } = controllerFixture(["changeNode"]);
  const write = deferred(), read = deferred();
  const previous = c.store.proxies;
  c.aiNodeDraft = { context: c.nodeSelectionContext(), node: "B" };
  c.api.selectProxy = (...args) => { calls.push(["select", ...args]); return write.promise; };
  c.refreshProxies = quiet => { calls.push(["refresh", quiet]); return read.promise; };
  const first = c.changeNode(OPENAI_GROUP, "B");
  assert.equal(c.nodeSelectionPending.phase, "write");
  assert.equal(c.store.proxies, previous);
  assert.deepEqual(calls.slice(0, 2), [["render", "write"], ["select", OPENAI_GROUP, "B", "one", "r1", "m1"]]);
  assert.ok(!calls.some(call => ["confirm", "refresh", "toast"].includes(call[0])));
  await c.changeNode(OPENAI_GROUP, "A");
  assert.equal(calls.filter(call => call[0] === "select").length, 1);

  let finished = false;
  void first.then(() => { finished = true; });
  write.resolve(receipt()); await flush();
  assert.equal(finished, true, "a pending whole-list refresh must not hold the selection operation");
  assert.equal(c.nodeSelectionPending, null);
  assert.equal(c.nodeSelectionBusy, false);
  assert.equal(c.aiNodeDraft, null);
  assert.equal(c.store.proxies.proxies[OPENAI_GROUP].now, "B");
  assert.equal(c.store.proxies.proxies[OPENAI_GROUP].manualNode, "B");
  assert.equal(c.store.proxies.proxies.PROXY, previous.proxies.PROXY, "other strategy groups remain unchanged");
  assert.deepEqual(calls.filter(call => call[0] === "refresh"), [["refresh", true]]);
  assert.deepEqual(calls.filter(call => call[0] === "toast"), [["toast", "已应用节点：B", "success"]]);
  read.resolve(); await first;
});

test("an already verified AI choice cannot resubmit, even through the command handler", async () => {
  const { context: c, calls } = controllerFixture(["changeNode"]);
  await c.changeNode(OPENAI_GROUP, "A");
  assert.deepEqual(calls, [], "verified A: no confirmation, PUT or unrelated refresh");
  await c.changeNode(OPENAI_GROUP, "B");
  await c.changeNode(OPENAI_GROUP, "B");
  assert.equal(calls.filter(call => call[0] === "select").length, 1);
  await c.changeNode(OPENAI_GROUP, null);
  await c.changeNode(OPENAI_GROUP, "B");
  assert.equal(calls.filter(call => call[0] === "select").length, 2, "automatic B can be explicitly pinned");
});

test("a replacement core needs fresh AI verification even when profile and mode did not change", async () => {
  const { context: c, calls } = controllerFixture(["changeNode", "performProxyRead"]);
  const stale = c.store.proxies;
  c.store.runtime = { ...c.store.runtime, startedAt: "later" };
  await c.changeNode(OPENAI_GROUP, "B");
  assert.ok(!calls.some(call => call[0] === "select"), "an old runtime snapshot cannot authorize AI selection");
  c.api.proxies = async () => stale;
  await c.performProxyRead(true, c.pageContext());
  await c.changeNode(OPENAI_GROUP, "B");
  assert.equal(calls.filter(call => call[0] === "select").length, 1, "a fresh read reenables selection");
});

test("an unknown-cost confirmation rechecks a newly verified identical choice before writing", async () => {
  const { context: c, calls } = controllerFixture(["changeNode"]);
  c.store.proxies.proxies.B.trafficMultiplier = null;
  c.confirmAction = async () => {
    Object.assign(c.store.proxies.proxies[OPENAI_GROUP], { now: "B", manualNode: "B" });
    return true;
  };
  await c.changeNode(OPENAI_GROUP, "B");
  assert.ok(!calls.some(call => ["select", "refresh", "toast"].includes(call[0])));
  assert.equal(c.nodeSelectionBusy, false);
});

test("an uncertain receipt cannot grey out Apply even if it observes the attempted exit", async () => {
  const { context: c, calls } = controllerFixture(["changeNode", "refreshProxies", "performProxyRead"]);
  const fresh = deferred();
  c.api.selectProxy = async () => receipt({ writeState: "unknown" });
  c.api.proxies = () => fresh.promise;
  const snapshot = c.store.proxies;
  await c.changeNode(OPENAI_GROUP, "B");
  const observed = c.store.proxies.proxies[OPENAI_GROUP];
  assert.equal(observed.now, "B");
  assert.doesNotMatch(nodeSelectionMarkup(OPENAI_GROUP, observed, false, undefined, c.store.proxies.proxies), /data-ai-choice-applied/);
  assert.deepEqual(calls.filter(call => call[0] === "toast"), [["toast", "节点操作结果未确认，核心当前出口：B。请刷新核对。", "info"]]);
  const reconcile = c.refreshProxies(true);
  fresh.resolve({ ...snapshot, proxies: { ...snapshot.proxies, [OPENAI_GROUP]: { ...snapshot.proxies[OPENAI_GROUP], now: "B", manualNode: "B" } } });
  await reconcile;
  assert.match(nodeSelectionMarkup(OPENAI_GROUP, c.store.proxies.proxies[OPENAI_GROUP], false, undefined, c.store.proxies.proxies), /data-ai-choice-applied/);
});

test("known budget and eligibility blocks do not request write, and show the reason", async () => {
  const { context: c, calls } = controllerFixture(["changeNode"]);
  const b = c.store.proxies.proxies.B;
  b.withinCostBudget = false;
  assert.match(nodeSelectionMarkup(OPENAI_GROUP, c.store.proxies.proxies[OPENAI_GROUP], false, ["A", "B"], c.store.proxies.proxies, undefined, { draft: "B" }), /value="B" selected disabled[^>]*>[^<]*超出当前流量倍率预算/);
  await c.changeNode(OPENAI_GROUP, "B");
  b.metadata = { eligible: false, regionReason: "地区待确认" };
  await c.changeNode(OPENAI_GROUP, "B");
  assert.deepEqual(calls, [["toast", "超出当前流量倍率预算", "info"], ["toast", "地区待确认", "info"]]);
});

test("unknown but permitted traffic cost prompts once; restore-auto and manual row preserve confirmation", async () => {
  const { context: c, calls } = controllerFixture(["changeNode"]);
  const b = c.store.proxies.proxies.B;
  b.trafficMultiplier = null;
  const confirmation = deferred();
  c.confirmAction = ({ title }) => { calls.push(["confirm", title]); return confirmation.promise; };
  const first = c.changeNode(OPENAI_GROUP, "B");
  await c.changeNode(OPENAI_GROUP, "A");
  assert.deepEqual(calls, [["confirm", "确认节点费用"]]);
  confirmation.resolve(true); await first;
  assert.equal(calls.filter(call => call[0] === "confirm").length, 1);
  assert.equal(calls.filter(call => call[0] === "select").length, 1);

  calls.length = 0;
  c.confirmAction = async ({ title }) => { calls.push(["confirm", title]); return false; };
  await c.changeNode(OPENAI_GROUP, null);
  c.store.manualOutbound.mode = "manual";
  await c.changeNode("PROXY", "B");
  assert.deepEqual(calls.filter(call => call[0] !== "render"), [["confirm", "恢复自动选择？"], ["confirm", "应用所选节点？"]]);
});

test("restore-auto uses the verified receipt and leaves unrelated groups unchanged", async () => {
  const { context: c, calls } = controllerFixture(["changeNode"]);
  const previous = c.store.proxies;
  await c.changeNode(OPENAI_GROUP, null);
  assert.equal(c.store.proxies.proxies[OPENAI_GROUP].manualNode, null);
  assert.equal(c.store.proxies.proxies.PROXY, previous.proxies.PROXY);
  assert.deepEqual(calls.filter(call => call[0] === "confirm"), [["confirm", "恢复自动选择？"]]);
  assert.deepEqual(calls.filter(call => call[0] === "auto"), [["auto", OPENAI_GROUP, "one", "r1", "m1"]]);
  assert.deepEqual(calls.filter(call => call[0] === "toast"), [["toast", "已恢复自动选择", "success"]]);
});

test("real errors, unknown or missing read-back, fallback, and missing intent do not claim verified success", async () => {
  const scenarios = [
    { error: "synthetic failure", toast: ["synthetic failure", "error"] },
    { result: receipt({ writeState: "unknown", now: "A", manualNode: "A" }), toast: ["节点操作结果未确认，核心当前出口：A。请刷新核对。", "info"] },
    { result: receipt({ groupType: null, now: null, manualNode: null }), toast: ["节点操作已提交，但暂未核实实际出口，请刷新后查看。", "info"] },
    { result: receipt({ groupType: "Fallback", fixed: "B", now: "A", manualNode: "B" }), toast: ["已设为优先节点，当前出口：A", "info"] },
    { result: receipt({ manualNode: null }), toast: ["节点操作已提交，但暂未核实实际出口，请刷新后查看。", "info"] },
    { result: receipt({ groupType: "Fallback", fixed: null, manualNode: null }), toast: ["节点操作已提交，但暂未核实实际出口，请刷新后查看。", "info"] },
    { result: receipt({ group: "PROXY" }), toast: ["节点操作结果未确认，请刷新核对。", "info"] },
  ];
  for (const { error, result, toast } of scenarios) {
    const { context: c, calls } = controllerFixture(["changeNode"]);
    const previous = c.store.proxies, draft = { context: c.nodeSelectionContext(), node: "B" };
    c.aiNodeDraft = draft;
    c.api.selectProxy = async () => { if (error) throw new Error(error); return result; };
    await c.changeNode(OPENAI_GROUP, "B");
    assert.equal(c.nodeSelectionBusy, false);
    assert.equal(c.nodeSelectionPending, null);
    assert.equal(c.aiNodeDraft, draft, "keep the user's selected node for checking or retry");
    assert.deepEqual(calls.filter(call => call[0] === "toast"), [["toast", ...toast]]);
    assert.equal(c.store.proxies.proxies.PROXY, previous.proxies.PROXY);
    if (result?.groupType === "Fallback" && result.fixed === "B") {
      const group = c.store.proxies.proxies[OPENAI_GROUP];
      assert.equal(group.manualNode, undefined, "Fallback retains preference, not Selector's fixed intent");
      assert.equal(group.type, "Fallback");
      assert.equal(group.fixed, "B");
      assert.equal(group.now, "A");
      assert.match(nodeSelectionMarkup(OPENAI_GROUP, group, false), /失效时由核心回退/);
    }
    if (error || !result.groupType || result.group !== OPENAI_GROUP) assert.equal(c.store.proxies, previous);
    assert.equal(calls.filter(call => call[0] === "refresh").length, error ? 0 : 1);
  }
});

test("an old full list cannot overwrite the verified exit; reconciliation refresh runs in the background", async () => {
  const { context: c, calls } = controllerFixture(["changeNode", "refreshProxies", "performProxyRead"]);
  const stale = deferred(), actual = deferred();
  let reads = 0;
  const previous = c.store.proxies;
  c.api.proxies = () => ++reads === 1 ? stale.promise : actual.promise;
  const old = c.refreshProxies(true); await flush();
  assert.equal(reads, 1);
  await c.changeNode(OPENAI_GROUP, "B");
  assert.equal(reads, 1);
  assert.equal(c.store.proxies.proxies[OPENAI_GROUP].now, "B");
  assert.equal(c.nodeSelectionPending, null);
  assert.deepEqual(calls.filter(call => call[0] === "toast"), [["toast", "已应用节点：B", "success"]]);
  stale.resolve(previous); await old; await flush();
  assert.equal(reads, 2);
  assert.equal(c.store.proxies.proxies[OPENAI_GROUP].now, "B");
  const refresh = c.refreshProxies(true);
  actual.resolve({ ...previous, proxies: { ...previous.proxies, [OPENAI_GROUP]: { ...previous.proxies[OPENAI_GROUP], now: "B", manualNode: "B" } } });
  await refresh;
  assert.equal(reads, 2);
  assert.equal(c.store.proxies.proxies[OPENAI_GROUP].now, "B");
});

test("a profile switch during cost confirmation prevents writing to the new profile", async () => {
  const { context: c, calls } = controllerFixture(["changeNode"]);
  c.store.proxies.proxies.B.trafficMultiplier = null;
  c.confirmAction = async () => { c.store.activeProfile = { profile: { id: "other", activeRevisionId: "r2" } }; return true; };
  await c.changeNode(OPENAI_GROUP, "B");
  assert.ok(!calls.some(call => call[0] === "select"));
});

test("a runtime, profile or outbound change drops the old selection receipt", async () => {
  for (const change of [
    c => { c.store.runtime = { ...c.store.runtime, pid: 2 }; },
    c => { c.store.activeProfile = { profile: { id: "other", activeRevisionId: "r2" } }; },
    c => { c.store.manualOutbound = { mode: "manual", revision: "m2" }; },
  ]) {
    const { context: c, calls } = controllerFixture(["changeNode"]);
    const pending = deferred();
    c.api.selectProxy = () => pending.promise;
    const selected = c.changeNode(OPENAI_GROUP, "B");
    assert.equal(c.nodeSelectionPending.phase, "write");
    change(c); c.store.proxies = null;
    pending.resolve(receipt()); await selected;
    assert.equal(c.store.proxies, null);
    assert.equal(c.nodeSelectionPending, null);
    assert.ok(!calls.some(call => ["toast", "refresh"].includes(call[0])));
  }
});

test("current AI node never opens an ordinary group, including inactive and non-rule modes", () => {
  const { context: c, calls } = controllerFixture(["showCurrentAiNode"]);
  c.showCurrentAiNode();
  assert.deepEqual(calls, [["details", OPENAI_GROUP]]);
  const profile = c.store.activeProfile.profile;
  calls.length = 0;
  for (const [mode, routingMode, enabled] of [["manual", "rule", true], ["ai", "global", true], ["ai", "rule", false]]) {
    c.store.manualOutbound.mode = mode;
    Object.assign(profile, { routingMode, openaiPolicy: { enabled } });
    c.showCurrentAiNode();
  }
  assert.deepEqual(calls, Array(3).fill(["toast", "当前未启用规则模式下的 AI 策略组", "info"]));
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  assert.match(main, /id="proxies-current-node">当前 AI 节点<\/button>/);
});
test("late node reads never revive stopped or switched-profile state", async () => {
  for (const mutation of [c => { c.store.runtime = { phase: "stopped", pid: null }; }, c => { c.store.activeProfile = { profile: { id: "other", activeRevisionId: "r2" } }; }]) {
    const { context: c } = controllerFixture(["performProxyRead"]);
    let finish;
    c.api.proxies = () => new Promise(resolve => { finish = resolve; });
    const pending = c.performProxyRead(true, c.pageContext());
    mutation(c); c.store.proxies = null;
    finish({ profileId: "one", revisionId: "r1", modeRevision: "m1", proxies: map }); await pending;
    assert.equal(c.store.proxies, null);
  }
});
test("mismatched node snapshot context clears stale details and exposes the read failure", async () => {
  const { context: c } = controllerFixture(["performProxyRead"]);
  c.api.proxies = async () => ({ profileId: "one", revisionId: "old", modeRevision: "m1", proxies: map });
  await c.performProxyRead(true, c.pageContext());
  assert.equal(c.store.proxies, null); assert.match(c.proxyReadError, /不一致/);
  assert.equal(Object.keys(c.overviewNodeDetails).length, 0);
});
