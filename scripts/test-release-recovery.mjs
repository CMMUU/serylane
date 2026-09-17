import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {inspectRecovery,targets} from './inspect-release-recovery.mjs';
function fixture(){
  const sha='a'.repeat(40),id=123;
  return {repo:{id:1355770287},tag:'v0.7.13',run:{id,repository:{id:1355770287},head_repository:{id:1355770287},path:'.github/workflows/release.yml',name:'Release bundles',event:'workflow_dispatch',head_branch:'main',status:'completed',conclusion:'cancelled',head_sha:sha},ref:{object:{type:'commit',sha}},jobs:[{name:'source',conclusion:'success'},...targets.map(t=>({name:`bundle (runner, ${t}, paths)`,conclusion:'success',steps:['Build signed installers and updater artifacts','Verify Windows installers on disposable runner','Verify version-independent Windows application binding'].map(name=>({name,conclusion:'success'}))}))],artifacts:targets.map(name=>({name,expired:false,size_in_bytes:123,workflow_run:{id,head_sha:sha}}))};
}
test('accepts only an immutable six-platform build already fully tested',()=>{assert.equal(inspectRecovery(fixture()),'a'.repeat(40));});
test('rejects active, successful, foreign, PR and untrusted workflow runs',()=>{
  for(const change of [v=>v.run.status='in_progress',v=>v.run.conclusion='success',v=>v.run.head_repository.id=1,v=>v.run.repository.id=1,v=>v.repo.id=1,v=>v.run.event='pull_request',v=>v.run.path='.github/workflows/ci.yml',v=>v.run.head_branch='feature']){const v=fixture();change(v);assert.throws(()=>inspectRecovery(v));}
});
test('ref mismatch, annotated or injected tags cannot be published',()=>{
  for(const change of [v=>v.ref.object.sha='b'.repeat(40),v=>v.ref.object.type='tag',v=>v.tag='v0.7.13\ninjected=true']){const v=fixture();change(v);assert.throws(()=>inspectRecovery(v));}
});
test('missing, duplicate and failed build or installer gates cannot be bypassed',()=>{
  for(const change of [v=>v.jobs.pop(),v=>v.jobs.push(v.jobs[1]),v=>v.jobs[1].conclusion='failure',v=>v.jobs.find(j=>j.name.includes('windows-x64')).steps[1].conclusion='skipped',v=>v.jobs[0].conclusion='failure']){const v=fixture();change(v);assert.throws(()=>inspectRecovery(v));}
});
test('expired, mismatched, extra or missing artifacts cannot be used',()=>{
  for(const change of [v=>v.artifacts[0].expired=true,v=>v.artifacts[0].workflow_run.id=1,v=>v.artifacts[0].workflow_run.head_sha='b'.repeat(40),v=>v.artifacts[0].size_in_bytes=0,v=>v.artifacts.pop(),v=>v.artifacts.push(v.artifacts[0])]){const v=fixture();change(v);assert.throws(()=>inspectRecovery(v));}
});
test('recovery workflow checks original immutable source and never rebuilds or clobbers',()=>{
  const source=readFileSync(new URL('../.github/workflows/recover-release.yml',import.meta.url),'utf8');
  for(const pattern of [/github\.ref == 'refs\/heads\/main'/,/node scripts\/inspect-release-recovery\.mjs/,/ref: \$\{\{ steps\.original\.outputs\.commit \}\}/,/run-id: \$\{\{ inputs\.source_run_id \}\}/,/GODEBUG: http2client=0/,/python3 scripts\/publish_github_release\.py/])assert.match(source,pattern);
  assert.doesNotMatch(source,/--clobber|tauri build|TAURI_SIGNING_PRIVATE_KEY|--insecure/);
});
