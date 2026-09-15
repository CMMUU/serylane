import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import ts from "typescript";
import vm from "node:vm";
const source = readFileSync(new URL("../src/node-selection.ts", import.meta.url), "utf8");
const js = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText;
const { generalGroups, resolvedNode, nodeSelectionMarkup, currentNodeMarkup, canRestoreAuto, OPENAI_GROUP } = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);
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
test("manual intent is checked again after probes and Selector auto bypasses DELETE", () => {
  const stability = readFileSync(new URL("../src-tauri/src/openai_stability.rs", import.meta.url), "utf8");
  const selection = readFileSync(new URL("../src-tauri/src/node_selection.rs", import.meta.url), "utf8");
  assert.ok(stability.indexOf("openai_manual_node", stability.indexOf(".collect::<Vec<_>>()")) > 0);
  assert.match(selection, /None if managed => Ok\(\(\)\)/);
  assert.doesNotMatch(selection, /close_connection|reload_config|stop_runtime/);
  assert.match(selection, /state.active_revision_id != Some\(revision\)/);
});

function controllerFixture(names) {
  const calls = [];
  const profile = { id: "one", activeRevisionId: "r1", routingMode: "rule", openaiPolicy: { enabled: true, stabilityEnabled: true } };
  const context = {
    store: { view: "proxies", activeProfile: { profile }, runtime: { phase: "running", pid: 1 }, proxies: { proxies: map }, openAiTask: null },
    nodeSelectionBusy: false, proxyReadSequence: 0, runtimeMutationRevision: 0, overviewNodeDetails: {}, proxyReadError: "", OPENAI_GROUP_NAME: OPENAI_GROUP,
    confirmAction: async () => true,
    api: {
      selectProxy: async (...args) => calls.push(["select", ...args]),
      clearProxySelection: async (...args) => calls.push(["auto", ...args]),
      proxies: async () => ({ profileId: "one", revisionId: "r1", proxies: map }),
    },
    action: async (_label, run) => { try { return await run(); } catch { return null; } },
    toast: (...args) => calls.push(["toast", ...args]), errorMessage: e => e.message,
    refreshProxies: async () => calls.push(["refresh"]), renderProxies() {}, renderOverviewNodes() {},
  };
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const ast = ts.createSourceFile("main.ts", main, ts.ScriptTarget.Latest, true);
  const functions = ast.statements.filter(statement => ts.isFunctionDeclaration(statement) && names.includes(statement.name?.text)).map(statement => statement.getText(ast)).join("\n");
  vm.createContext(context);
  vm.runInContext(ts.transpileModule(functions, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText, context);
  return { context, calls };
}
test("selection cancel, failures and duplicate submissions never claim an optimistic selection", async () => {
  const { context: c, calls } = controllerFixture(["changeNode"]);
  c.confirmAction = async () => false;
  await c.changeNode(OPENAI_GROUP, "B");
  assert.deepEqual(calls, [["refresh"]]);
  let finish;
  c.confirmAction = () => new Promise(resolve => { finish = resolve; });
  const first = c.changeNode(OPENAI_GROUP, "B");
  await c.changeNode(OPENAI_GROUP, "A");
  finish(true); await first;
  assert.deepEqual(calls.filter(call => call[0] === "select"), [["select", OPENAI_GROUP, "B", "one", "r1"]]);
  c.api.selectProxy = async () => { throw new Error("synthetic failure"); };
  c.confirmAction = async () => true;
  const previous = c.store.proxies;
  await c.changeNode(OPENAI_GROUP, "B");
  assert.equal(c.store.proxies, previous); assert.equal(c.nodeSelectionBusy, false);
});
test("a profile switch during confirmation prevents writing to the new profile", async () => {
  const { context: c, calls } = controllerFixture(["changeNode"]);
  c.confirmAction = async () => { c.store.activeProfile = { profile: { id: "other", activeRevisionId: "r2" } }; return true; };
  await c.changeNode(OPENAI_GROUP, "B");
  assert.ok(!calls.some(call => call[0] === "select"));
});
test("late node reads never revive stopped or switched-profile state", async () => {
  for (const mutation of [c => { c.store.runtime = { phase: "stopped", pid: null }; }, c => { c.store.activeProfile = { profile: { id: "other", activeRevisionId: "r2" } }; }]) {
    const { context: c } = controllerFixture(["refreshProxies"]);
    let finish;
    c.api.proxies = () => new Promise(resolve => { finish = resolve; });
    const pending = c.refreshProxies(true);
    mutation(c); c.store.proxies = null;
    finish({ profileId: "one", revisionId: "r1", proxies: map }); await pending;
    assert.equal(c.store.proxies, null);
  }
});
test("mismatched node snapshot context clears stale details and exposes the read failure", async () => {
  const { context: c } = controllerFixture(["refreshProxies"]);
  c.api.proxies = async () => ({ profileId: "one", revisionId: "old", proxies: map });
  await c.refreshProxies(true);
  assert.equal(c.store.proxies, null); assert.match(c.proxyReadError, /不一致/);
  assert.equal(Object.keys(c.overviewNodeDetails).length, 0);
});
