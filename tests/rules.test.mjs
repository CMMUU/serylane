import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(new URL("../src/rule-manager.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
}).outputText;
const { RULE_KINDS, RulesDraft, parseVisualRule, visualRuleText, serializeUserRules, rulesFingerprint, copyRuleText } =
  await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);

const rule = (id, enabled = true) => ({ id, enabled, rule: `DOMAIN,${id}.example,DIRECT`, note: `备注 ${id}` });
const state = (revision = 1, rules = [rule("one"), rule("two", false)]) => ({ revision, rules, history: [], targets: ["DIRECT", "REJECT"], warnings: [], routingMode: "rule" });
const valid = (rules) => ({ valid: true, errors: [], warnings: [], normalizedRules: rules, preview: "rules only" });
const fixture = () => { const model = new RulesDraft(); model.sync(state()); return model; };

test("visual editor supports the agreed simple rule kinds", () => {
  assert.deepEqual(RULE_KINDS, ["DOMAIN", "DOMAIN-SUFFIX", "DOMAIN-KEYWORD", "IP-CIDR", "IP-CIDR6", "PROCESS-NAME", "PROCESS-PATH", "NETWORK", "DST-PORT"]);
  assert.deepEqual(parseVisualRule("DOMAIN-SUFFIX,example.com,开发策略"), { kind: "DOMAIN-SUFFIX", value: "example.com", target: "开发策略", noResolve: false });
});

test("IP no-resolve option survives visual conversion", () => {
  const text = "IP-CIDR6,2001:db8::/32,DIRECT,no-resolve";
  assert.equal(visualRuleText(parseVisualRule(text)), text);
  assert.equal(visualRuleText({ kind: "DOMAIN", value: " example.com ", target: " DIRECT ", noResolve: true }), "DOMAIN,example.com,DIRECT");
});

test("complex and unsupported rules stay raw rather than being truncated", () => {
  for (const text of ["AND,((NETWORK,TCP),(DST-PORT,443)),DIRECT", "MATCH,DIRECT", "RULE-SET,private,DIRECT", "DOMAIN,example.com,DIRECT,unexpected", "DOMAIN,,DIRECT"]) assert.equal(parseVisualRule(text), null);
  const raw = { ...rule("advanced"), rule: "AND,((NETWORK,TCP),(DST-PORT,443)),DIRECT" };
  assert.ok(serializeUserRules([raw]).endsWith(raw.rule));
});

test("text serialization preserves IDs, disabled entries, and multiline notes safely", () => {
  const item = { ...rule("disabled", false), note: "line one\nline two \"quoted\"" };
  const [comment, raw] = serializeUserRules([item]).split("\n");
  assert.deepEqual(JSON.parse(comment.replace("# mihomo-codex-rule: ", "")), { id: item.id, enabled: false, note: item.note });
  assert.equal(raw, item.rule);
});

test("initial state is clean and does not permit saving", () => {
  const model = fixture();
  assert.equal(model.dirty, false); assert.equal(model.canSave, false); assert.equal(model.conflict, false);
  assert.equal(model.text, serializeUserRules(model.rules));
});

test("editing clones inputs and invalidates prior validation", () => {
  const model = fixture(); const incoming = [...model.rules, rule("three")];
  model.replace(incoming); incoming[2].note = "external mutation";
  assert.equal(model.rules[2].note, "备注 three");
  assert.equal(model.dirty, true);
  model.markValidated(valid(model.rules), model.epoch);
  assert.equal(model.canSave, true);
  model.replace(model.rules.map((item) => ({ ...item, enabled: !item.enabled })));
  assert.equal(model.canSave, false);
});

test("reordering preserves every ID and observes bounds", () => {
  const model = fixture(); const epoch = model.epoch;
  model.move("one", -1); model.move("missing", 1);
  assert.equal(model.epoch, epoch);
  model.move("two", -1); assert.deepEqual(model.rules.map(({ id }) => id), ["two", "one"]);
  assert.equal(model.rules[0].enabled, false);
  model.move("two", 1); assert.equal(model.dirty, false);
});

test("refresh preserves a dirty draft and detects external revisions", () => {
  const model = fixture(); model.replace([...model.rules, rule("local")]);
  const before = rulesFingerprint(model.rules);
  model.sync(state(2, [rule("remote")]));
  assert.equal(rulesFingerprint(model.rules), before);
  assert.equal(model.base.revision, 1); assert.equal(model.latest.revision, 2); assert.equal(model.conflict, true);
  model.markValidated(valid(model.rules), model.epoch); assert.equal(model.canSave, false);
});

test("same-revision refresh does not erase text that has not parsed", () => {
  const model = fixture(); model.editText("INVALID,custom,unparsed"); model.sync(state());
  assert.equal(model.text, "INVALID,custom,unparsed"); assert.equal(model.textPending, true); assert.equal(model.dirty, true);
});

test("undo loads the newest observed state after a conflict", () => {
  const model = fixture(); model.replace([rule("local")]); model.sync(state(2, [rule("remote")])); model.undo();
  assert.equal(model.base.revision, 2); assert.equal(model.rules[0].id, "remote"); assert.equal(model.dirty, false); assert.equal(model.conflict, false);
});

test("successful save fences older in-flight refresh results", () => {
  const model = fixture(); model.replace([rule("saved")]); model.accept(state(3, model.rules));
  assert.equal(model.sync(state(2, [rule("stale")])), false);
  assert.equal(model.base.revision, 3); assert.equal(model.rules[0].id, "saved"); assert.equal(model.dirty, false);
});

test("stale validation cannot bless a newer draft", () => {
  const model = fixture(); model.replace([rule("first")]); const epoch = model.epoch;
  model.replace([rule("newer")]);
  assert.equal(model.markValidated(valid([rule("first")]), epoch), false);
  assert.equal(model.rules[0].id, "newer"); assert.equal(model.canSave, false);
});

test("failed validation preserves draft and prevents save", () => {
  const model = fixture(); model.replace([rule("bad")]);
  model.markValidated({ ...valid([]), valid: false, errors: ["第 1 行：格式错误"] }, model.epoch);
  assert.equal(model.rules[0].id, "bad"); assert.equal(model.canSave, false);
});

test("normalized validated draft becomes the exact save candidate", () => {
  const model = fixture(); model.replace([{ ...rule("one"), rule: "DOMAIN,one.example,DIRECT " }]);
  const normalized = [{ ...rule("one"), note: "updated" }];
  model.markValidated(valid(normalized), model.epoch);
  assert.equal(rulesFingerprint(model.rules), rulesFingerprint(normalized));
  assert.equal(model.canSave, true); assert.equal(model.text, serializeUserRules(normalized));
});

test("rule manager API has no unrelated settings or proxy mutation methods", () => {
  assert.doesNotMatch(source, /api\.(?:updateSettings|setTheme|setNetworkMode|startActive|stop|installTunHelper)\(/);
  assert.match(source, /expectedRevision: number/);
  assert.match(source, /if \(!model\.canSave \|\| editorDirty\) return/);
});

test("configuration activation/import/refresh never stops the core before backend validation", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const configActions = main.slice(main.indexOf("async function createSubscription("), main.indexOf("function openAiTaskPhaseLabel("));
  assert.ok(configActions.includes("api.createSubscriptionProfile("));
  assert.ok(configActions.includes("refreshSubscriptionTracked("));
  const refreshWrapper = main.slice(main.indexOf("function refreshSubscriptionTracked("), main.indexOf("async function createSubscription("));
  assert.match(refreshWrapper, /api\.refreshProfile\(profileId, id, download\)/);
  assert.doesNotMatch(refreshWrapper, /api\.stop\(|api\.startActive\(/);
  assert.ok(configActions.includes("api.activateProfile("));
  assert.doesNotMatch(configActions, /api\.(?:stop|startActive)\(/);
  assert.match(main.slice(main.indexOf("async function stopRuntime("), main.indexOf("function toggleGlobalNetworkMode(")), /api\.stop\(/);
});

test("clipboard export only reports success after the write promise fulfills", async () => {
  let finish;
  let received;
  const expected = serializeUserRules([rule("one"), rule("disabled", false)]);
  const pending = copyRuleText(expected, (text) => { received = text; return new Promise((resolve) => { finish = resolve; }); });
  let completed = false;
  void pending.then(() => { completed = true; });
  await Promise.resolve();
  assert.equal(completed, false);
  assert.equal(received, expected);
  finish();
  assert.equal(await pending, true);
});

test("clipboard rejection preserves a manual-copy fallback rather than claiming success", async () => {
  assert.equal(await copyRuleText("DOMAIN,example.com,DIRECT", async () => { throw new Error("permission denied"); }), false);
  assert.equal(await copyRuleText("text", () => { throw new Error("clipboard unavailable"); }), false);
  assert.match(source, /id="user-rules-export-text"[^>]*readonly/);
  assert.match(source, /id="user-rules-select-all"/);
  assert.doesNotMatch(source, /createObjectURL|\.download\s*=/);
});

test("hidden no-resolve checkbox wins over the shared flex checkbox rule", () => {
  const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
  assert.match(css, /\.checkbox-row\.is-hidden\s*\{\s*display:\s*none\s*!important/);
});
