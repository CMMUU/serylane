import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const code=ts.transpileModule(readFileSync(new URL('../src/diagnostics.ts',import.meta.url),'utf8'),{compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText;
const {createDiagnostics,diagnosticRowsMarkup}=await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
const tick=()=>new Promise(resolve=>setImmediate(resolve));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};};
const ids=['binary','runtime','profile','system_proxy','api','local_proxy','controller','http_proxy'];
function fixture(overrides={}) {
 const calls=[],updates=[];let context='profile/session/1',allowed=true;
 const values={binary:{available:true,version:'fixture-v1'},runtime:{phase:'running',message:'合成运行态'},profile:{profile:{displayName:'合成订阅'}},system_proxy:{active:true},api:{proxies:{}}};
 const read=async id=>{calls.push(id);return id in values?values[id]:{stage:id,success:true,latencyMs:3,detail:'合成检查'};};
 const options={context:()=>context,allowed:()=>allowed,networkMode:()=> 'system_proxy',binary:()=>read('binary'),runtime:()=>read('runtime'),activeProfile:()=>read('profile'),systemProxy:()=>read('system_proxy'),proxies:()=>read('api'),diagnosticStep:read,error:e=>e.message,changed:s=>updates.push(s),...overrides};
 const controller=createDiagnostics(options);
 return {controller,calls,updates,values,options,context:v=>{context=v;},allowed:v=>{allowed=v;}};
}

test('each real step is published while pending, followed immediately by its result and the next row',async()=>{
 const gates=Object.fromEntries(ids.map(id=>[id,deferred()]));
 const f=fixture();
 for(const [name,id] of [['binary','binary'],['runtime','runtime'],['activeProfile','profile'],['systemProxy','system_proxy'],['proxies','api']]) f.options[name]=()=>{f.calls.push(id);return gates[id].promise;};
 f.options.diagnosticStep=id=>{f.calls.push(id);return gates[id].promise;};
 const done=f.controller.run();
 for(let index=0;index<ids.length;index++){
  await tick();
  const snapshot=f.controller.snapshot();
  assert.equal(snapshot.phase,'running');assert.equal(snapshot.pending,true);
  assert.deepEqual(snapshot.rows.map(r=>r.id),ids.slice(0,index+1));
  assert.equal(snapshot.rows.at(-1).status,'running');
  assert.ok(snapshot.rows.slice(0,-1).every(r=>r.status==='pass'));
  assert.deepEqual(f.calls,ids.slice(0,index+1));
  const id=ids[index];gates[id].resolve(f.values[id]??{stage:id,success:true,latencyMs:2,detail:'实际步骤已结束'});
 }
 await done;
 assert.equal(f.controller.snapshot().phase,'completed');assert.equal(f.controller.snapshot().pending,false);
 assert.ok(f.controller.snapshot().rows.every(r=>r.status==='pass'));
 // Saved publications must not change when their row is subsequently updated.
 assert.equal(f.updates.find(s=>s.rows.length===1).rows[0].status,'running');
});

test('duplicate clicks share a single physical run and do not start duplicate probes',async()=>{
 const gate=deferred();let calls=0;const f=fixture({binary:()=>{calls++;return gate.promise;}});
 const a=f.controller.run(),b=f.controller.run();assert.equal(a,b);await tick();assert.equal(calls,1);
 gate.resolve({available:true,version:'fixture'});await a;assert.equal(f.controller.snapshot().rows.length,8);
});

test('one failed request is visible immediately without discarding earlier results or blocking later steps',async()=>{
 const f=fixture({diagnosticStep:async stage=>{if(stage==='controller')throw Error('合成端口读取失败');return {stage,success:true,detail:'合成检查',latencyMs:1};}});
 await f.controller.run();const s=f.controller.snapshot();
 assert.equal(s.rows[6].status,'fail');assert.equal(s.rows[6].detail,'合成端口读取失败');
 assert.equal(s.rows[5].status,'pass');assert.equal(s.rows[7].status,'pass');assert.match(s.message,/未通过 1/);
});

test('a stopped core explicitly skips the four network checks and never invokes them',async()=>{
 const f=fixture();f.values.runtime={phase:'stopped',message:'核心未运行'};await f.controller.run();
 assert.deepEqual(f.calls,ids.slice(0,4));assert.equal(f.controller.snapshot().rows.filter(r=>r.status==='skipped').length,4);
 assert.ok(f.controller.snapshot().rows.slice(4).every(r=>/核心未运行/.test(r.detail)));
});

test('unknown runtime does not turn missing network results into success',async()=>{
 const f=fixture({runtime:async()=>{throw Error('运行态读取超时');}});await f.controller.run();const s=f.controller.snapshot();
 assert.equal(s.rows[1].status,'fail');assert.ok(s.rows.slice(4).every(r=>r.status==='skipped'&&/读取失败/.test(r.detail)));
 assert.ok(!f.calls.includes('http_proxy'));
});

test('fresh active-profile and OS proxy errors remain visible rather than being read from old cached state',async()=>{
 const f=fixture({activeProfile:async()=>{throw Error('配置读取失败');},systemProxy:async()=>{throw Error('系统代理读取失败');}});
 await f.controller.run();assert.equal(f.controller.snapshot().rows[2].detail,'配置读取失败');assert.equal(f.controller.snapshot().rows[3].status,'fail');
 assert.equal(f.controller.snapshot().rows[7].status,'pass');
});

test('a mode that does not require the OS proxy marks that check inapplicable',async()=>{
 for(const mode of ['manual','tun']){const f=fixture({networkMode:()=>mode});f.values.system_proxy={active:false};await f.controller.run();assert.equal(f.controller.snapshot().rows[3].status,'skipped');}
 const f=fixture({networkMode:()=>undefined});f.values.system_proxy={active:false};await f.controller.run();assert.equal(f.controller.snapshot().rows[3].status,'warn');
});

test('late results from a different proxy or subscription context are rejected and stop further probes',async()=>{
 const gate=deferred();const f=fixture({binary:()=>gate.promise});const done=f.controller.run();await tick();f.context('other/profile/session/2');gate.resolve({available:true,version:'stale-result'});await done;
 const s=f.controller.snapshot();assert.equal(s.phase,'interrupted');assert.equal(s.rows.length,1);assert.equal(s.rows[0].status,'skipped');assert.ok(!s.rows[0].detail.includes('stale-result'));assert.equal(f.calls.length,0);
});

test('a changed context during the last slow request cannot publish a final success',async()=>{
 const gate=deferred();const f=fixture({diagnosticStep:async stage=>stage==='http_proxy'?gate.promise:{stage,success:true,detail:'合成检查',latencyMs:null}});
 const done=f.controller.run();await tick();assert.equal(f.controller.snapshot().rows.length,8);f.context('another-runtime');
 gate.resolve({stage:'http_proxy',success:true,detail:'迟到成功',latencyMs:5});await done;
 assert.equal(f.controller.snapshot().phase,'interrupted');assert.equal(f.controller.snapshot().rows[7].status,'skipped');assert.ok(!f.controller.snapshot().message.includes('诊断完成'));
});

test('leaving the page immediately clears the checking indicator and does not permit overlapping restart',async()=>{
 const gate=deferred();const f=fixture({binary:()=>gate.promise});const done=f.controller.run();await tick();
 f.controller.invalidate('已离开页面');assert.equal(f.controller.snapshot().phase,'interrupted');assert.equal(f.controller.snapshot().rows[0].status,'skipped');assert.equal(f.controller.snapshot().pending,true);
 assert.equal(f.controller.run(),done);gate.resolve({available:true,version:'late'});await done;assert.equal(f.controller.snapshot().pending,false);assert.equal(f.controller.snapshot().message,'已离开页面');
});

test('lifecycle pause prevents starting and discards in-flight results',async()=>{
 const f=fixture();f.allowed(false);await f.controller.run();assert.deepEqual(f.calls,[]);assert.equal(f.controller.snapshot().phase,'idle');
 const gate=deferred();f.options.binary=()=>gate.promise;f.allowed(true);const done=f.controller.run();await tick();f.allowed(false);gate.reject(Error('native session ended'));await done;
 assert.equal(f.controller.snapshot().phase,'interrupted');assert.equal(f.controller.snapshot().rows[0].status,'skipped');
});

test('a new completed run replaces old rows and allows retry after a failure',async()=>{
 const f=fixture();f.values.binary={available:false,message:'合成未找到'};await f.controller.run();assert.equal(f.controller.snapshot().rows[0].status,'fail');
 const next=f.updates.length;f.values.binary={available:true,version:'fixture-fixed'};await f.controller.run();
 assert.equal(f.updates[next].rows.length,0);assert.equal(f.controller.snapshot().rows.length,8);assert.equal(f.controller.snapshot().rows[0].status,'pass');
});

test('mismatched native stage cannot attribute its success to another row',async()=>{
 const f=fixture({diagnosticStep:async()=>({stage:'http_proxy',success:true,detail:'wrong stage',latencyMs:1})});await f.controller.run();
 assert.equal(f.controller.snapshot().rows[5].status,'fail');assert.match(f.controller.snapshot().rows[5].detail,/不匹配/);assert.equal(f.controller.snapshot().rows[7].status,'pass');
});

test('labels and error text are escaped, while rows expose stable identity and real states',()=>{
 const html=diagnosticRowsMarkup({rows:[{id:'x\"<',label:'<img src=x>',status:'running',detail:'<script>bad()</script>'},{id:'done',label:'端口',status:'fail',detail:'a & b'}]});
 assert.ok(!html.includes('<img'));assert.ok(!html.includes('<script>'));assert.match(html,/&lt;script&gt;/);assert.match(html,/data-render-key="x&quot;&lt;"/);assert.match(html,/status-running/);assert.match(html,/检测中/);assert.match(html,/未通过/);assert.match(html,/role="listitem"/);
});
