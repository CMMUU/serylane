import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const read = name => readFileSync(new URL(`../${name}`, import.meta.url), 'utf8');
const js = ts.transpileModule(read('src/read-coordinator.ts'), {compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText;
const {ReadCoordinator, PageReads} = await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};};
test('100 concurrent readers share exactly one IPC and receive isolated values',async()=>{
 const r=new ReadCoordinator();const d=deferred();let calls=0;
 const requests=Array.from({length:100},()=>r.read('connections',()=>{calls++;return d.promise;}));
 await Promise.resolve(); assert.equal(calls,1);d.resolve({value:1});const result=await Promise.all(requests);
 result[0].value=2;assert.equal(result[1].value,1);
});
test('read during a mutation waits then obtains a fresh snapshot, never parallel',async()=>{
 const r=new ReadCoordinator();const d=deferred();let calls=0;
 const op=()=>++calls===1?d.promise:Promise.resolve('fresh');
 const old=r.read('runtime',op);await Promise.resolve();r.invalidate();const recent=r.read('runtime',op);
 assert.equal(calls,1);d.resolve('stale');assert.deepEqual(await Promise.all([old,recent]),['fresh','fresh']);assert.equal(calls,2);
});
test('timeout reports an error but does not release still-running native flight',async()=>{
 const r=new ReadCoordinator(15);const d=deferred();let calls=0;const op=()=>{calls++;return d.promise;};
 await assert.rejects(r.read('slow',op),/超时/);await assert.rejects(r.read('slow',op),/超时/);assert.equal(calls,1);
 d.resolve('done');await new Promise(r=>setTimeout(r,0));assert.equal(await r.read('slow',()=>Promise.resolve('retry')),'retry');
});
test('failed reads can retry; writes are never part of the coordinator',async()=>{
 const r=new ReadCoordinator();await assert.rejects(r.read('one',()=>Promise.reject(new Error('offline'))),/offline/);
 assert.equal(await r.read('one',()=>Promise.resolve('online')),'online');
});
test('view changes drop queued obsolete work and coalesce whole render operations',async()=>{
 const p=new PageReads(),d=deferred();let context='a',renders=0,reads=0;
 const load=()=>{const requested=context;return p.run('view',requested,async()=>{if(requested!==context)return;reads++;await d.promise;if(requested===context)renders++;});};
 const pending=[load(),load()];await Promise.resolve();context='b';pending.push(load());context='c';pending.push(load(),load());d.resolve();await Promise.all(pending);
 assert.equal(reads,2);assert.equal(renders,1);
});
test('native filesystem and OS queries are async background reads, writes retain direct ownership checks',()=>{
 const native=read('src-tauri/src/lib.rs');for(const command of ['system_proxy_status','runtime_status','probe_mihomo','list_subscriptions','get_profile_details','get_active_profile']){
  assert.match(native,new RegExp(`async fn ${command}[\\s\\S]{0,160}ui_read::run`));
 }
 assert.match(read('src-tauri/src/ui_read.rs'),/spawn_blocking/);assert.match(read('src-tauri/src/ui_read.rs'),/Semaphore::new\(4\)/);
 assert.match(read('src-tauri/src/platform.rs'),/child\.kill\(\)[\s\S]{0,60}child\.wait\(\)/);
 assert.match(read('src/main.ts'),/if \(previousView === view\) return/);
});
