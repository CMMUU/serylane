// Read-only gate for resuming publication of existing, verified CI artifacts.
import assert from 'node:assert/strict';
import {appendFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

export const targets=['linux-arm64','linux-x64','macos-aarch64','macos-x64','windows-arm64','windows-x64'];
export function inspectRecovery({repo,run,jobs,artifacts,ref,tag}) {
  assert.equal(repo.id,1355770287,'Unexpected repository');
  assert.match(tag,/^v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/);
  assert.equal(run.repository?.id,repo.id);assert.equal(run.head_repository?.id,repo.id);
  assert.equal(run.path,'.github/workflows/release.yml');assert.equal(run.name,'Release bundles');
  assert.ok(['push','workflow_dispatch'].includes(run.event));
  assert.ok(run.head_branch==='main'||run.head_branch===tag);
  assert.equal(run.status,'completed','Original publisher must stop before recovery');
  assert.ok(['failure','cancelled','timed_out'].includes(run.conclusion),'Successful runs do not need recovery');
  assert.match(run.head_sha,/^[a-f0-9]{40}$/);assert.equal(ref.object.type,'commit');assert.equal(ref.object.sha,run.head_sha);
  assert.equal(jobs.find(j=>j.name==='source')?.conclusion,'success');
  const bundles=jobs.filter(j=>j.name.startsWith('bundle ('));
  assert.equal(bundles.length,6,'All six platform gates are required');
  const names=bundles.map(j=>{
    const name=j.name.match(/^bundle \([^,]+, ([^,)]+)(?:,|\))/)?.[1];
    assert.ok(targets.includes(name),'Unexpected platform');assert.equal(j.conclusion,'success');
    assert.equal(j.steps.find(s=>s.name==='Build signed installers and updater artifacts')?.conclusion,'success');
    if(name.startsWith('windows-')){
      for(const step of ['Verify Windows installers on disposable runner','Verify version-independent Windows application binding'])assert.equal(j.steps.find(s=>s.name===step)?.conclusion,'success');
    }
    return name;
  });
  assert.deepEqual(names.sort(),targets);
  assert.deepEqual(artifacts.map(a=>a.name).sort(),targets,'Only the original six platform artifacts are accepted');
  for(const artifact of artifacts){
    assert.equal(artifact.expired,false);assert.ok(artifact.size_in_bytes>0);
    assert.equal(artifact.workflow_run?.id,run.id);assert.equal(artifact.workflow_run?.head_sha,run.head_sha);
  }
  return run.head_sha;
}

if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  assert.equal(process.env.GITHUB_REPOSITORY,'CMMUU/serylane');assert.equal(process.env.GITHUB_REF,'refs/heads/main');
  const id=process.env.SOURCE_RUN_ID,tag=process.env.RELEASE_TAG;
  assert.match(id??'',/^\d+$/);assert.ok(process.env.GITHUB_TOKEN);
  const base='https://api.github.com/repos/CMMUU/serylane';
  async function get(path){
    const r=await fetch(base+path,{headers:{Authorization:`Bearer ${process.env.GITHUB_TOKEN}`,Accept:'application/vnd.github+json','User-Agent':'Serylane-Verified-Release-Recovery'},redirect:'error',signal:AbortSignal.timeout(30000)});
    if(!r.ok)throw Error(`GitHub verification HTTP ${r.status}`);return r.json();
  }
  assert.match(tag??'',/^v\d+\.\d+\.\d+$/);
  const [repo,run,jobs,artifacts,ref]=await Promise.all([get(''),get(`/actions/runs/${id}`),get(`/actions/runs/${id}/jobs?per_page=100`),get(`/actions/runs/${id}/artifacts?per_page=100`),get(`/git/ref/tags/${tag}`)]);
  assert.equal(jobs.total_count,jobs.jobs.length,'Incomplete job listing');assert.equal(artifacts.total_count,artifacts.artifacts.length,'Incomplete artifact listing');
  const commit=inspectRecovery({repo,run,jobs:jobs.jobs,artifacts:artifacts.artifacts,ref,tag});
  const active=await get('/actions/workflows/release.yml/runs?per_page=100');
  assert.ok(active.workflow_runs.every(r=>r.status==='completed'),'Another release build/writer is active');
  appendFileSync(process.env.GITHUB_OUTPUT,`commit=${commit}\n`);
  console.log(`Verified ${tag}: original six-platform gates and artifact provenance at ${commit}.`);
}
