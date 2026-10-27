import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const read = path => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const source = ts.transpileModule(read('src/app-lifecycle.ts'), {compilerOptions: {target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText;
const {AppLifecycle} = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
test('shutdown cancellation resumes interaction and drops out-of-order events',()=>{
 const lifecycle=new AppLifecycle();
 for(let i=0;i<100;i++){
  assert.equal(lifecycle.update({generation:i*2+1,phase:'preparing'}),true);
  assert.throws(()=>lifecycle.requireOpen(),/暂停/);
  assert.equal(lifecycle.update({generation:i*2+2,phase:'open'}),true);
  assert.equal(lifecycle.update({generation:i*2+1,phase:'preparing'}),false);
  assert.equal(lifecycle.paused,false);lifecycle.requireOpen();
 }
 lifecycle.update({generation:201,phase:'closing'});
 assert.equal(lifecycle.update({generation:202,phase:'open'}),false);
 assert.equal(lifecycle.paused,true);
});
test('ordinary visibility/navigation changes cannot freeze lifecycle',()=>{
 const lifecycle=new AppLifecycle();
 assert.equal(lifecycle.update({generation:0,phase:'preparing'}),false);
 assert.equal(lifecycle.update({generation:1,phase:'hidden'}),false);
 assert.equal(lifecycle.update({generation:NaN,phase:'closing'}),false);
 assert.equal(lifecycle.paused,false);
});
test('status/stop are probe-free and all Windows child roles cross one native gate',()=>{
 const runtime=read('src-tauri/src/runtime.rs');
 const status=runtime.slice(runtime.indexOf('pub fn status('),runtime.indexOf('pub fn logs('));
 assert.doesNotMatch(status,/probe_binary\(|binary_version\(|\.spawn\(/);
 assert.match(status,/binary_snapshot\(app\)/);
 const spawn=read('src-tauri/src/windows_process.rs');
 assert.match(spawn,/lifecycle::admission\(purpose\)[\s\S]*CreateProcessW\(/);
 const ui=read('src/api.ts');assert.match(ui,/appLifecycle.requireOpen\(\)/);
 assert.match(read('src/main.ts'),/app-lifecycle/);
});

test('pending reads never commit across prepare and cancel; fresh reads resume',async()=>{
 const lifecycle=new AppLifecycle();let finish;
 const old=lifecycle.read(()=>new Promise(resolve=>{finish=resolve;}));
 lifecycle.update({generation:1,phase:'preparing'});
 lifecycle.update({generation:2,phase:'open'});finish('old');
 await assert.rejects(old,/会话状态已变化/);
 assert.equal(await lifecycle.read(async()=> 'fresh'),'fresh');
 lifecycle.update({generation:3,phase:'updating'});
 await assert.rejects(lifecycle.read(async()=> 'blocked'),/安装更新/);
 assert.doesNotThrow(()=>lifecycle.requireStatus());
 lifecycle.update({generation:4,phase:"preparing"});
 assert.throws(()=>lifecycle.requireStatus(),/暂停/);
 lifecycle.update({generation:5,phase:'open'});
 assert.equal(await lifecycle.read(async()=> 'resumed'),'resumed');
});
