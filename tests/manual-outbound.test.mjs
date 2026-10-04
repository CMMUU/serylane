import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const source = readFileSync(new URL('../src/manual-outbound.ts', import.meta.url), 'utf8');
const js = ts.transpileModule(source, {compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText;
const {mountManualOutbound, manualOutboundMarkup} = await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
const node = {profileId:'p',revisionId:'r',profileName:'<private-name>',name:'<node>',protocol:'socks5'};
const state = {selection:null,nodes:[node],notices:[]};
const deferred = () => {let resolve;const promise = new Promise(r=>resolve=r);return {promise,resolve};};
const flush = () => new Promise(r=>setImmediate(r));
function fixture(overrides={}) {
  const els = new Map([...manualOutboundMarkup.matchAll(/id="([^"]+)"/g)].map(m=>[m[1],{value:'',textContent:'',innerHTML:'',disabled:false,handlers:{},addEventListener(k,v){this.handlers[k]=v;}}]));
  const writes=[];
  const manager=mountManualOutbound({querySelector:s=>els.get(s.slice(1))},{
    read:async()=>structuredClone(state), write:async n=>{writes.push(n);return {...state,selection:n?{profileId:n.profileId,nodeName:n.name}:null};},
    confirm:async()=>true,changed:async()=>{},error:e=>e.message,...overrides});
  manager.accept(structuredClone(state));
  return {manager,writes,els,click:id=>els.get(id).handlers.click(),choose:()=>els.get('manual-node-list').handlers.change({target:{name:'manual-node',value:'0'}})};
}
test('manual markup states actual mutual exclusion and never opts into system networking',()=>{
  assert.match(manualOutboundMarkup,/与「代理」页互斥/);
  assert.match(manualOutboundMarkup,/不会自动开启系统代理或 TUN/);
  const f=fixture();assert.match(f.els.get('manual-node-list').innerHTML,/&lt;node&gt;/);assert.doesNotMatch(f.els.get('manual-node-list').innerHTML,/<node>/);
});
test('cancel sends no write; successful apply and return update actual mode',async()=>{
  const cancelled=fixture({confirm:async()=>false}); cancelled.choose();cancelled.click('manual-apply');await flush();assert.equal(cancelled.writes.length,0);
  const f=fixture();f.choose();f.click('manual-apply');await flush();assert.deepEqual(f.writes,[node]);assert.match(f.els.get('manual-current').textContent,/自选节点/);
  await f.manager.disable();assert.equal(f.writes[1],null);assert.match(f.els.get('manual-current').textContent,/代理模式/);
});
test('failed mode write keeps previous mode and allows retry',async()=>{
  const f=fixture({write:async()=>{throw new Error('校验未通过，原模式已保留');}});f.choose();f.click('manual-apply');await flush();
  assert.match(f.els.get('manual-current').textContent,/代理模式/);assert.match(f.els.get('manual-result').textContent,/原模式已保留/);assert.equal(f.els.get('manual-apply').disabled,false);
});
test('a late read cannot overwrite a confirmed write and filtering clears hidden choices',async()=>{
  const read=deferred();const f=fixture({read:()=>read.promise});const pending=f.manager.refresh();f.choose();f.click('manual-apply');await flush();read.resolve(state);await pending;
  assert.match(f.els.get('manual-current').textContent,/自选节点/);
  f.els.get('manual-search').value='no match';f.els.get('manual-search').handlers.input();assert.equal(f.els.get('manual-apply').disabled,true);
});
test('base read is guarded throughout manual writes and backend mode guards cover policy paths',()=>{
  const main=readFileSync(new URL('../src/main.ts',import.meta.url),'utf8');
  assert.match(main,/requestedDuringRuntimeWrite = .*manualModeSwitching/);
  for(const file of ['node_selection.rs','openai_policy.rs','openai_stability.rs']) {
    assert.match(readFileSync(new URL(`../src-tauri/src/${file}`,import.meta.url),'utf8'),/require_proxy_mode/);
  }
});
