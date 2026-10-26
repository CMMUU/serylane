import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const compile = file => ts.transpileModule(readFileSync(new URL(`../src/${file}`, import.meta.url),'utf8'),{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText;
const url = js => `data:text/javascript;base64,${Buffer.from(js).toString('base64')}`;
const {createAiHealth, aiHealthMarkup} = await import(url(compile('ai-health.ts').replace("'./node-test'", JSON.stringify(url(compile('node-test.ts'))))));
const scope = {kind:'ai',profileId:'p',revisionId:'r',modeRevision:'m',runtimePid:1,runtimeStartedAt:'2026-10-26T00:00:00Z',routingMode:'rule'};
const snapshot = (extra={}) => ({id:'job',scope,sequence:1,phase:'running',valid:true,message:'正在检查',startedAt:'2026-10-26T00:00:00Z',finishedAt:null,nodes:[{name:'synthetic',state:'pending',delayMs:null}],...extra});
const pending = () => {let resolve,reject;return {promise:new Promise((a,b)=>{resolve=a;reject=b;}),resolve:v=>resolve(v),reject:e=>reject(e)};};
function fixture(extra={}) {let context=scope, starts=0;const manager=createAiHealth({context:()=>context,start:async()=>{starts++;return snapshot();},status:async()=>snapshot(),cancel:async()=>snapshot({phase:'cancelled',sequence:3}),changed:()=>{},error:e=>e.message,...extra});return {manager,context:v=>context=v,starts:()=>starts};}
test('button responds immediately; duplicates submit one job; completion retains results',async()=>{
 const f=fixture();const run=f.manager.run();assert.equal(f.manager.view().pending,true);assert.match(aiHealthMarkup(f.manager.view()),/正在开始健康检查/);await f.manager.run();await run;
 assert.equal(f.starts(),1);assert.equal(f.manager.view().running,true);await f.manager.run(true);assert.equal(f.manager.view().snapshot.phase,'cancelled');
});
test('late polling does not overwrite a new run',async()=>{
 const d=pending(),f=fixture({status:()=>d.promise});const read=f.manager.refresh();await f.manager.run();d.resolve(snapshot({id:'old',phase:'completed'}));await read;assert.equal(f.manager.view().snapshot.id,'job');
});
test('changed profile or stopped core drops late start responses',async()=>{
 const d=pending(),f=fixture({start:()=>d.promise});const run=f.manager.run();f.context({...scope,profileId:'other'});assert.equal(f.manager.view().snapshot,null);d.resolve(snapshot());await run;assert.equal(f.manager.view().snapshot,null);f.context(null);assert.equal(f.manager.view().enabled,false);
});
test('manual task and older snapshots do not enter AI health results',async()=>{
 let value=snapshot({sequence:5});const f=fixture({status:async()=>value});await f.manager.refresh();value=snapshot({sequence:4,phase:'completed'});await f.manager.refresh();assert.equal(f.manager.view().snapshot.phase,'running');
 value=snapshot({scope:{...scope,kind:'manual'},sequence:9});await f.manager.refresh();assert.equal(f.manager.view().snapshot.sequence,5);
});
test('cancel is single flight and a rejected start stays visible through polls',async()=>{
 const f=fixture({start:async()=>{throw new Error('核心尚未就绪');},status:async()=>null});await f.manager.run();await f.manager.refresh();assert.match(aiHealthMarkup(f.manager.view()),/核心尚未就绪/);assert.equal(f.manager.view().pending,false);
});
test('status outage preserves cancellation and clears notice after recovery',async()=>{
 let fail=true;const f=fixture({status:async()=>{if(fail)throw Error('offline');return snapshot({sequence:2});}});await f.manager.run();await f.manager.refresh();assert.match(aiHealthMarkup(f.manager.view()),/无需重复启动/);assert.match(aiHealthMarkup(f.manager.view()),/取消检查/);fail=false;await f.manager.refresh();assert.equal(f.manager.view().notice,'');
});
test('summary is persistent, escapes node names and keeps unknown different from failed',()=>{
 const html=aiHealthMarkup({enabled:true,pending:false,running:false,notice:'',snapshot:snapshot({phase:'completed',finishedAt:'2026-10-26T00:00:02Z',nodes:[{name:'<script>',state:'unavailable',delayMs:null},{name:'one',state:'passed',delayMs:10}]})});
 assert.match(html,/上次检查完成/);assert.match(html,/暂未测得/);assert.match(html,/&lt;script&gt;/);assert.doesNotMatch(html,/<script>| · 0 ms/);assert.match(html,/10 ms/);
});
