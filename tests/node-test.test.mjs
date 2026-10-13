import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const source=readFileSync(new URL('../src/node-test.ts',import.meta.url),'utf8');
const js=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText;
const {mountNodeTest,sameNodeTestScope,nodeTestSummary}=await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
const scope={profileId:'p',revisionId:'r',modeRevision:'m',runtimePid:2,runtimeStartedAt:'2026-10-13T00:00:00Z',routingMode:'rule'};
const snapshot=(changes={})=>({id:'j1',scope:{...scope},sequence:1,valid:true,phase:'running',message:'测试',nodes:[{name:'A',state:'pending',delayMs:null},{name:'B',state:'pending',delayMs:null}],...changes});
const deferred=()=>{let resolve;return {promise:new Promise(r=>resolve=r),resolve:(v)=>resolve(v)};};
const flush=()=>new Promise(r=>setImmediate(r));
function fixture(overrides={}) {
 const elements=new Map(['manual-test-all','manual-test-cancel','manual-test-progress','manual-test-status'].map(id=>[id,{disabled:false,hidden:false,textContent:'',handlers:{},addEventListener(k,v){this.handlers[k]=v;}}]));
 let context={...scope};const calls=[];
 const manager=mountNodeTest({querySelector:s=>elements.get(s.slice(1))},{context:()=>context,start:async s=>{calls.push(['start',s]);return snapshot();},status:async()=>null,cancel:async id=>{calls.push(['cancel',id]);return snapshot({phase:'cancelled',sequence:2});},changed:()=>{},error:e=>e.message,...overrides});
 return {manager,elements,calls,context:v=>{context=v;manager.render();},click:id=>elements.get(id).handlers.click()};
}
test('scope includes profile, revision, mode, runtime identity and route; timestamps normalize',()=>{
 assert.equal(sameNodeTestScope(scope,{...scope,runtimeStartedAt:'2026-10-13T00:00:00+00:00'}),true);
 for(const key of Object.keys(scope)) assert.equal(sameNodeTestScope(scope,{...scope,[key]:key==='runtimePid'?3:'different'}),false);
 assert.equal(sameNodeTestScope(null,scope),false);
});
test('unknown, failed and untested results never become zero latency',()=>{
 const s=snapshot({phase:'cancelled',nodes:['passed','timeout','failed','unavailable','pending'].map(state=>({name:state,state,delayMs:state==='passed'?10:null}))});
 assert.match(nodeTestSummary(s),/已完成 4\/5 · 可用 1 · 超时 1 · 失败 1 · 暂未测得 1 · 未测试 1/);
});
test('duplicate clicks submit once and cancel uses exact task identity',async()=>{
 const pending=deferred();let starts=0;const f=fixture({start:()=>{starts++;return pending.promise;}});
 f.click('manual-test-all');f.click('manual-test-all');assert.equal(starts,1);pending.resolve(snapshot());await flush();
 assert.equal(f.elements.get('manual-test-all').disabled,true);f.click('manual-test-cancel');await flush();
 assert.deepEqual(f.calls,[['cancel','j1']]);assert.equal(f.elements.get('manual-test-all').disabled,false);
});
test('late read cannot overwrite newly started job',async()=>{
 const pending=deferred();const f=fixture({status:()=>pending.promise});const read=f.manager.refresh();
 f.click('manual-test-all');await flush();pending.resolve(snapshot({id:'older',phase:'completed'}));await read;
 assert.equal(f.manager.snapshot().id,'j1');
});
test('lower sequence, stale scope and invalidated tasks are not rendered',async()=>{
 let value=snapshot({sequence:5});const f=fixture({status:async()=>value});await f.manager.refresh();
 value=snapshot({sequence:4,phase:'completed'});await f.manager.refresh();assert.equal(f.manager.snapshot().phase,'running');
 f.context({...scope,revisionId:'r2'});assert.equal(f.manager.snapshot(),null);await f.manager.refresh();assert.equal(f.manager.snapshot(),null);
 f.context(scope);value=snapshot({sequence:6,valid:false});await f.manager.refresh();assert.equal(f.manager.snapshot(),null);
});
test('configuration switch during start drops its late reply; stopped core stays disabled',async()=>{
 const pending=deferred();const f=fixture({start:()=>pending.promise});f.click('manual-test-all');f.context(null);pending.resolve(snapshot());await flush();
 assert.equal(f.manager.snapshot(),null);assert.equal(f.elements.get('manual-test-all').disabled,true);
});
test('start failure remains actionable through polling and a retry clears it',async()=>{
 let fail=true;const f=fixture({start:async()=>{if(fail)throw new Error('请拆分配置后测试');return snapshot();}});
 f.click('manual-test-all');await flush();await f.manager.refresh();assert.match(f.elements.get('manual-test-status').textContent,/请拆分配置/);
 fail=false;f.click('manual-test-all');await flush();assert.doesNotMatch(f.elements.get('manual-test-status').textContent,/请拆分配置/);
});
test('status failure leaves cancellation available and preserves measured results',async()=>{
 const f=fixture({status:async()=>{throw new Error('offline');}});f.click('manual-test-all');await flush();await f.manager.refresh();
 assert.equal(f.manager.snapshot().phase,'running');assert.equal(f.elements.get('manual-test-cancel').hidden,false);assert.match(f.elements.get('manual-test-status').textContent,/正在重试/);
});
test('single retests replace batch cell results without changing batch summary; late responses cannot cross a new job',async()=>{
 const f=fixture({status:async()=>snapshot({phase:'completed'})});await f.manager.refresh();const rev=f.manager.revision();
 f.manager.recordSingle(scope,rev,{name:'A',state:'passed',delayMs:55});assert.equal(f.manager.results()[0].delayMs,55);
 assert.equal(f.manager.snapshot().nodes[0].state,'pending');
 f.click('manual-test-all');await flush();f.manager.recordSingle(scope,rev,{name:'A',state:'passed',delayMs:99});assert.equal(f.manager.results()[0].delayMs,null);
 f.context({...scope,profileId:'other'});assert.deepEqual(f.manager.results(),[]);
});
