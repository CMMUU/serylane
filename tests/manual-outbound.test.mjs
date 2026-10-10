import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const source = readFileSync(new URL('../src/manual-outbound.ts', import.meta.url), 'utf8');
const js = ts.transpileModule(source, {compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText;
const {mountManualOutbound, manualOutboundMarkup, nodeGridMarkup} = await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
const state = {mode:'ai',revision:'r1',activeProfileId:'p',legacySelection:false};
const deferred = () => {let resolve;const promise = new Promise(r=>resolve=r);return {promise,resolve};};
const flush = () => new Promise(r=>setImmediate(r));
function fixture(overrides={}) {
  const els = new Map([...manualOutboundMarkup.matchAll(/id="([^"]+)"/g)].map(m=>[m[1],{value:'',textContent:'',innerHTML:'',disabled:false,handlers:{},addEventListener(k,v){this.handlers[k]=v;}}]));
  const writes=[];
  const manager=mountManualOutbound({querySelector:s=>els.get(s.slice(1))},{
    read:async()=>structuredClone(state), write:async (mode,revision)=>{writes.push([mode,revision]);return {...state,mode,revision:'r2'};},
    confirm:async()=>true,changed:async()=>{},error:e=>e.message,...overrides});
  manager.accept(structuredClone(state));
  return {manager,writes,els,click:id=>els.get(id).handlers.click()};
}
test('group UI explains rules, exclusivity and independent Codex route without turning on networking',()=>{
  assert.match(manualOutboundMarkup,/与 AI 代理互斥/);assert.match(manualOutboundMarkup,/Codex 路由接入独立/);
  assert.match(manualOutboundMarkup,/不会自动开启系统代理或 TUN/);assert.match(manualOutboundMarkup,/保留原有分流规则/);
});
test('cancel writes nothing; apply sends expected revision and returning to AI is explicit',async()=>{
  const cancelled=fixture({confirm:async()=>false});cancelled.click('manual-apply');await flush();assert.equal(cancelled.writes.length,0);
  const f=fixture();f.click('manual-apply');await flush();assert.deepEqual(f.writes,[['manual','r1']]);assert.match(f.els.get('manual-current').textContent,/自选节点/);
  await f.manager.enableAi();assert.deepEqual(f.writes[1],['ai','r2']);assert.match(f.els.get('manual-current').textContent,/AI 代理/);
});
test('failed mode commit retains previous mode and allows retry',async()=>{
  const f=fixture({write:async()=>{throw new Error('校验未通过，原模式已保留');}});f.click('manual-apply');await flush();
  assert.match(f.els.get('manual-current').textContent,/AI 代理/);assert.match(f.els.get('manual-result').textContent,/原模式已保留/);assert.equal(f.els.get('manual-apply').disabled,false);
});
test('late reads and double clicks cannot replace a confirmed mode switch',async()=>{
  const read=deferred(),write=deferred();const f=fixture({read:()=>read.promise,write:()=>write.promise});
  const pending=f.manager.refresh();f.click('manual-apply');f.click('manual-apply');await flush();
  write.resolve({...state,mode:'manual',revision:'r2'});await flush();read.resolve(state);await pending;
  assert.match(f.els.get('manual-current').textContent,/自选节点/);assert.equal(f.els.get('manual-controls').disabled,false);
});
test('empty state stays usable and legacy selection requires explicit migration',()=>{
  const f=fixture();f.manager.accept({...state,activeProfileId:null});assert.match(f.els.get('manual-current').textContent,/未启用/);
  f.manager.accept({...state,mode:'manual',legacySelection:true});assert.equal(f.els.get('manual-apply').disabled,false);assert.equal(f.els.get('manual-controls').disabled,true);
});
test('node grid escapes names, filters full membership and sorts known delays ahead of unknowns',()=>{
  const map={Group:{type:'Selector',all:['<A>','B','C'],now:'B'},'<A>':{type:'Trojan'},B:{type:'SS',history:[{delay:30}]},C:{type:'VMess',history:[{delay:10}]}};
  const grid=nodeGridMarkup('Group',map,'','delay',false);
  assert.ok(grid.indexOf('data-node-choice="C"')<grid.indexOf('data-node-choice="B"'));assert.ok(grid.indexOf('data-node-choice="B"')<grid.indexOf('data-node-choice="&lt;A&gt;"'));
  assert.doesNotMatch(grid,/<A>/);assert.match(grid,/aria-pressed="true"/);
  assert.doesNotMatch(nodeGridMarkup('Group',map,'vmess','source',false),/data-node-choice="B"/);
  assert.match(nodeGridMarkup('Group',map,'','source',true),/aria-pressed="true" disabled/);
});
test('backend guards cover policy paths, stale tasks, and startup deletion recovery; no Codex writes',()=>{
  const read=f=>readFileSync(new URL(`../src-tauri/src/${f}`,import.meta.url),'utf8');
  for(const file of ['node_selection.rs','openai_policy.rs','openai_stability.rs']) {assert.match(read(file),/require_proxy_mode/);assert.match(read(file),/require_revision/);}
  assert.doesNotMatch(read('manual_outbound.rs'),/set_codex|set_local_route|set_local_route_enabled/);
  const main=readFileSync(new URL('../src/main.ts',import.meta.url),'utf8');assert.match(main,/requestedDuringRuntimeWrite = .*manualModeSwitching/);
  assert.ok(read('lib.rs').indexOf('storage.recover_profile_deletion()')<read('lib.rs').indexOf('let persistent = storage.state().map_err'));
});
