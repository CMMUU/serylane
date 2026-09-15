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
  assert.match(html,/仅性价比模式/); assert.match(html,/质量优先沿用原策略，不限制倍率/);
  assert.match(html,/不从名称猜测/); assert.match(html,/placeholder="未知" value=""/);
  assert.match(html,/拒绝新请求/); assert.match(html,/保存成本策略/);
  assert.doesNotMatch(html,/<img/); assert.doesNotMatch(html,/name="allowUnknown" type="checkbox" checked/);
});
test('value benchmark filters before probing, skips bandwidth and checks policy revision before application', () => {
  const policy = read('src-tauri/src/openai_policy.rs');
  assert.ok(policy.indexOf('filter(|node| costs.allowed') < policy.indexOf('benchmark_nodes(app'));
  assert.match(policy,/if costs.value_mode\(\) \{\s*vec!\[None; reachable.len\(\)\]/);
  assert.match(policy,/None => None/); assert.match(policy,/costs.value_mode\(\) && second_delay.is_none\(\)/);
  assert.match(policy,/storage.openai_costs\(profile_id\)\?\.revision != costs/);
});
test('manual selection takes priority while cost updates reject stale versions and incompatible fallback', () => {
  const stability = read('src-tauri/src/openai_stability.rs');
  assert.ok(stability.indexOf('openai_manual_node(id)') < stability.indexOf('let cost_preferences'));
  assert.match(stability,/storage.openai_costs\(active.unwrap\(\)\)\?\.revision != cost_preferences.revision/);
  const cost = read('src-tauri/src/openai_cost.rs');
  assert.match(cost,/current.revision != input.revision/); assert.match(cost,/!policy.stability_enabled/);
  assert.match(cost,/if !confirmed/); assert.doesNotMatch(cost,/\.start_runtime|\.stop_runtime|\.close_connection/);
});
