import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const read=f=>readFileSync(new URL(`../${f}`,import.meta.url),'utf8');
const url=s=>`data:text/javascript;base64,${Buffer.from(ts.transpileModule(s,{compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText).toString('base64')}`;
const {InteractionSession}=await import(url(read('src/interaction-session.ts')));
const {parseDomains,domainRoutingMarkup}=await import(url(read('src/domain-routing.ts').replace("'./node-selection'",JSON.stringify(url(read('src/node-selection.ts'))))));
test('100 close/reopen cycles drop stale responses, stale errors and retests',()=>{
 const s=new InteractionSession();
 for(let i=0;i<100;i++){
  const context=`p1/r${i}/manual/core1`; const first=s.begin(context);
  assert.notEqual(first,null);assert.equal(s.begin(context),null);assert.equal(s.refresh(context),null);
  s.close();assert.equal(s.accepts(first,context),false);
  const next=s.begin(context);s.settle(first);assert.equal(s.refresh(context),null);
  s.settle(next); const retest=s.refresh(context);assert.notEqual(retest,null);assert.equal(s.refresh(context),null);
  s.close();assert.equal(s.accepts(retest,context),false);assert.equal(s.refresh(context),null);
 }
});
test('profile, view, mode and runtime context changes invalidate work',()=>{
 const s=new InteractionSession();const first=s.begin('profile1/rev1/manual/core1');
 assert.equal(s.accepts(first,'profile2/rev1/manual/core1'),false);
 const next=s.begin('profile2/rev2/ai/core2');assert.equal(s.accepts(first,'profile1/rev1/manual/core1'),false);assert.equal(s.accepts(next,'profile2/rev2/ai/core2'),true);
});
test('presets and custom addresses share domain-only validation and no Codex attach UI',()=>{
 assert.deepEqual(parseDomains('https://*.OpenAI.com/*\nchatgpt.com'),['*.openai.com','chatgpt.com']);
 for(const value of ['*','*.com','https://api.openai.com/v1/*','a.com:443','x@a.com','127.0.0.1','openai.com?secret=x','a..com'])assert.throws(()=>parseDomains(value));
 assert.throws(()=>parseDomains('OPENAI.com\nopenai.com'));
 assert.match(domainRoutingMarkup,/常见 AI 工具/);assert.match(domainRoutingMarkup,/自定义地址/);assert.match(domainRoutingMarkup,/保存草稿/);
 assert.doesNotMatch(domainRoutingMarkup,/id="local-route-(attach|stability)"/);
});
test('disabled capabilities have UI, IPC and worker boundaries',()=>{
 const main=read('src/main.ts'), backend=read('src-tauri/src/program_proxy.rs'),stability=read('src-tauri/src/openai_stability.rs');
 assert.doesNotMatch(main,/mountProgramManager|data-openai-action="stability"|mountLocalRouting/);
 assert.equal((backend.match(/require_program_proxy\(\)\?/g)||[]).length,7);
 assert.match(stability,/if !crate::capabilities::ENHANCED_STABILITY/);
 assert.doesNotMatch(main.slice(main.indexOf('"retest" && details'),main.indexOf('document.addEventListener("keydown"')),/openNodeDetails/);
});
