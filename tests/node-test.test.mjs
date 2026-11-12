import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const source=readFileSync(new URL('../src/node-test.ts',import.meta.url),'utf8');
const js=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText;
const {mountNodeTest,sameNodeTestScope,nodeTestSummary}=await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
const gridJs=ts.transpileModule(readFileSync(new URL('../src/manual-outbound.ts',import.meta.url),'utf8'),{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText;
const {nodeGridMarkup}=await import(`data:text/javascript;base64,${Buffer.from(gridJs).toString('base64')}`);
const scope={profileId:'p',revisionId:'r',modeRevision:'m',runtimePid:2,runtimeStartedAt:'2026-10-13T00:00:00Z',routingMode:'rule'};
const snapshot=(changes={})=>({id:'j1',scope:{...scope},sequence:1,valid:true,phase:'running',message:'测试',nodes:[{name:'A',state:'pending',delayMs:null},{name:'B',state:'pending',delayMs:null}],...changes});
const deferred=()=>{let resolve,reject;return {promise:new Promise((r,j)=>{resolve=r;reject=j;}),resolve:v=>resolve(v),reject:e=>reject(e)};};
const flush=()=>new Promise(r=>setImmediate(r));
function fixture(overrides={}) {
 const elements=new Map(['manual-test-all','manual-test-cancel','manual-test-progress','manual-test-status'].map(id=>[id,{
   disabled:false,hidden:false,textContent:'',handlers:{},attributes:new Map(),
   getAttribute(k){return this.attributes.get(k)??null;},setAttribute(k,v){this.attributes.set(k,v);},addEventListener(k,v){this.handlers[k]=v;},
 }]));
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
 const pending=deferred(), cancellation=deferred(), calls=[];
 const f=fixture({start:()=>{calls.push('start');return pending.promise;},cancel:id=>{calls.push(id);return cancellation.promise;}});
 const start=f.elements.get('manual-test-all'),cancel=f.elements.get('manual-test-cancel');
 f.click('manual-test-all');f.click('manual-test-all');
 assert.deepEqual(calls,['start']);assert.equal(start.textContent,'正在开始测试…');assert.equal(start.getAttribute('aria-busy'),'true');
 assert.equal(start.disabled,true);assert.equal(cancel.hidden,true);
 pending.resolve(snapshot());await flush();
 assert.equal(start.textContent,'测试进行中…');assert.equal(start.getAttribute('aria-busy'),'true');assert.equal(cancel.hidden,false);
 f.click('manual-test-cancel');f.click('manual-test-cancel');
 assert.deepEqual(calls,['start','j1']);assert.equal(cancel.textContent,'正在停止…');assert.equal(cancel.getAttribute('aria-busy'),'true');
 assert.equal(start.getAttribute('aria-busy'),'false');assert.equal(start.disabled,true);
 cancellation.resolve(snapshot({phase:'cancelled',sequence:2}));await flush();
 assert.equal(start.disabled,false);assert.equal(start.textContent,'测试全部');assert.equal(cancel.getAttribute('aria-busy'),'false');
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
 const f=fixture({status:async()=>snapshot({phase:'completed'})});await f.manager.refresh();
 await f.manager.runSingle('G','A',async()=>({delay:55}));assert.equal(f.manager.results()[0].delayMs,55);
 assert.equal(f.manager.snapshot().nodes[0].state,'pending');
 const old=deferred(), late=f.manager.runSingle('G','A',()=>old.promise);
 f.click('manual-test-all');await flush();old.resolve({delay:99});
 assert.equal(await late,null);assert.equal(f.manager.results()[0].delayMs,null);
 f.context({...scope,profileId:'other'});assert.deepEqual(f.manager.results(),[]);
});
test('same node in another group is disabled without a spinner and repaint cannot resubmit it',async()=>{
 const work=deferred(), f=fixture();
 let calls=0;
 const first=f.manager.runSingle('G1','A',()=>{calls++;return work.promise;});
 assert.equal(f.manager.pendingSingles().get('A'),'G1');
 const map={G1:{type:'Selector',all:['A']},G2:{type:'Selector',all:['A']}};
 const grid=g=>nodeGridMarkup(g,map,'','source',false,f.manager.results(),false,200,f.manager.pendingSingles());
 assert.match(grid('G1'),/class="proxy-delay[^>]*aria-busy="true" disabled>测速中…/);
 assert.match(grid('G2'),/class="proxy-delay[^>]*disabled>测速</);
 assert.doesNotMatch(grid('G2'),/aria-busy="true"/);
 await f.manager.refresh();
 assert.equal(await f.manager.runSingle('G2','A',()=>{calls++;return work.promise;}),null);
 assert.equal(calls,1);
 work.resolve({delay:36});
 assert.deepEqual(await first,{kind:'complete',delayMs:36});
 assert.equal(f.manager.pendingSingles().size,0);
 assert.match(grid('G2'),/>36 ms</);
 assert.doesNotMatch(grid('G1'),/aria-busy="true"/);
});
test('late same-name reply cannot overwrite or unlock a current-scope speed test',async()=>{
 const old=deferred(), next=deferred(), f=fixture();
 const oldRun=f.manager.runSingle('G','A',()=>old.promise);
 f.context({...scope,revisionId:'new'});
 assert.equal(f.manager.pendingSingles().size,0);
 const newRun=f.manager.runSingle('G','A',()=>next.promise);
 old.resolve({delay:900});
 assert.equal(await oldRun,null);
 assert.equal(f.manager.pendingSingles().get('A'),'G');
 assert.deepEqual(f.manager.results(),[]);
 const error=new Error('合成测试失败');next.reject(error);
 assert.deepEqual(await newRun,{kind:'failed',error});
 assert.equal(f.manager.pendingSingles().size,0);
 assert.deepEqual(f.manager.results(),[{name:'A',state:'unavailable',delayMs:null}]);
 assert.deepEqual(await f.manager.runSingle('G','A',async()=>({delay:20})),{kind:'complete',delayMs:20});
});
