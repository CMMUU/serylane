import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const read = name => readFileSync(new URL(`../${name}`, import.meta.url), 'utf8');
const moduleUrl = source => `data:text/javascript;base64,${Buffer.from(ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText).toString('base64')}`;
const { parseMultiplier, costFormMarkup } = await import(moduleUrl(read('src/openai-costs.ts').replace('"./node-selection"', JSON.stringify(moduleUrl(read('src/node-selection.ts'))))));
test('multiplier input distinguishes unknown from invalid and never invents 1x', () => {
  assert.equal(parseMultiplier(''), null); assert.equal(parseMultiplier('  '), null);
  assert.equal(parseMultiplier('0.1'), .1); assert.equal(parseMultiplier('3'),3);
  for(const input of ['0','-1','Infinity','NaN','3x','0.001','1001']) assert.throws(()=>parseMultiplier(input));
});
test('cost controls clearly scope caps, unknown fallback and explicit save', () => {
  const html = costFormMarkup({ mode:'value', maxMultiplier:2, allowUnknown:false, nodes:[{name:'<img onerror="x">',multiplier:null}] });
  assert.match(html,/仅性价比模式/); assert.match(html,/不限倍率/);
  assert.match(html,/两种预算模式都按同一套推荐权重选点/);
  assert.match(html,/手动覆盖优先/); assert.match(html,/placeholder="自动 \/ 未知" value=""/);
  assert.match(html,/拒绝新请求/); assert.match(html,/保存成本策略/);
  assert.doesNotMatch(html,/<img/); assert.doesNotMatch(html,/name="allowUnknown" type="checkbox" checked/);
});
test('automatic name rate stays a placeholder; a budget save never freezes it as a manual price', () => {
  const html = costFormMarkup({mode:'value',maxMultiplier:2,allowUnknown:false,nodes:[{name:'JP 0.5x',multiplier:0.5,manualMultiplier:null,metadata:{eligible:true,regionReason:'名称地区在 API 支持名单内',multiplierSource:'name',nameMultiplier:0.5}}]});
  assert.match(html, /名称识别 0.5×/);
  assert.match(html, /placeholder="0.5" value=""/);
  assert.match(html, /1 个名称符合 · 0 个排除/);
  assert.match(read('src/openai-costs.ts'), /multiplier: parseMultiplier\(input\(`node-\$\{i\}`\).value\)/);
});
test('country eligibility gates every managed entry point, including historical manual intent', () => {
  for (const file of ['openai_policy','effective','openai_stability','node_selection']) assert.match(read(`src-tauri/src/${file}.rs`), /node_metadata::eligible/);
  const stability = read('src-tauri/src/openai_stability.rs');
  assert.match(stability, /if !valid[\s\S]*?api.select_proxy\(GROUP, "REJECT"\)/);
});
test('benchmark filters before probing, rechecks connections without bulk downloads and checks policy revision before application', () => {
  const policy = read('src-tauri/src/openai_policy.rs');
  assert.ok(policy.indexOf('filter(|node| costs.allowed') < policy.indexOf('benchmark_nodes(app'));
  const benchmark = policy.slice(policy.indexOf('async fn benchmark_nodes'), policy.indexOf('async fn probe_candidate'));
  assert.equal((benchmark.match(/probe_candidate\(/g) ?? []).length, 2);
  assert.doesNotMatch(policy,/measure_bandwidth|BANDWIDTH_BYTES/);
  assert.match(policy,/storage.openai_costs\(profile_id\)\?\.revision != costs/);
});
test('manual selection and transactional budget filtering reject stale versions', () => {
  const stability = read('src-tauri/src/openai_stability.rs');
  assert.ok(stability.indexOf('openai_manual_node(id)') < stability.indexOf('let cost_preferences'));
  assert.match(stability,/storage.openai_costs\(active.unwrap\(\)\)\?\.revision != cost_preferences.revision/);
  const cost = read('src-tauri/src/openai_cost.rs');
  assert.match(cost,/current.revision != input.revision/); assert.match(cost,/apply_profile_config/);
  assert.match(read("src-tauri/src/effective.rs"),/costs.allowed/);
  assert.match(cost,/if !confirmed/); assert.doesNotMatch(cost,/\.start_runtime|\.stop_runtime|\.close_connection/);
});
