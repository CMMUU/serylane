import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';

const read = name => readFileSync(new URL(`../src/${name}`, import.meta.url), 'utf8');
const compile = source => ts.transpileModule(source, {compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText;
const {proxyPresentation} = await import(`data:text/javascript;base64,${Buffer.from(compile(read('proxy-state.ts'))).toString('base64')}`);
const {canStartRuntime} = await import(`data:text/javascript;base64,${Buffer.from(compile(read('runtime-start.ts'))).toString('base64')}`);
const {PageReads} = await import(`data:text/javascript;base64,${Buffer.from(compile(read('read-coordinator.ts'))).toString('base64')}`);
const {canStopSession} = await import(`data:text/javascript;base64,${Buffer.from(compile(read('session-resume.ts'))).toString('base64')}`);
const input = (mode='system_proxy', phase='running', active=true) => ({mode, runtime:{phase,message:'fixture',lastError:null},systemProxy:{active}});

test('stopped service keeps the remembered mode without checked switches or running lights', () => {
  for (const mode of ['system_proxy','tun','manual']) {
    const result=proxyPresentation(input(mode,'stopped',false));
    assert.equal(result.label,'已关闭'); assert.equal(result.active,false);
    assert.equal(result.systemProxyActive,false); assert.equal(result.tunActive,false);
  }
});
test('system proxy needs an owned active OS setting and a live core', () => {
  assert.equal(proxyPresentation(input()).state,'system_proxy');
  assert.equal(proxyPresentation(input('system_proxy','running',false)).state,'unavailable');
  assert.equal(proxyPresentation(input('system_proxy','stopped',true)).state,'error');
  assert.equal(proxyPresentation({...input(),systemProxy:null}).state,'checking');
});
test('TUN needs a verified current ingress instead of the helper installation flag', () => {
  assert.equal(proxyPresentation(input('tun','running',false)).state,'checking');
  for(const state of ['unknown','stopped']) assert.equal(proxyPresentation({...input('tun','running',false),ingress:{state,message:'pending'}}).state,'checking');
  assert.equal(proxyPresentation({...input('tun','running',false),ingress:{state:'unavailable',message:'route missing'}}).state,'unavailable');
  const ready=proxyPresentation({...input('tun','running',false),ingress:{state:'tun',message:'verified'}});
  assert.equal(ready.label,'TUN 运行中'); assert.equal(ready.tunActive,true);
});
test('explicit advanced Manual is a local listener, not a normal shutdown or global takeover', () => {
  const local=proxyPresentation(input('manual','running',false));
  assert.equal(local.state,'local_only'); assert.equal(local.active,false);
  assert.match(local.label,/本地代理/);
});
test('transition, read and stop failures cannot reuse a ready indicator', () => {
  for(const activity of ['starting','stopping','switching','restoring','working']) assert.equal(proxyPresentation({...input(),activity}).state,'checking');
  assert.equal(proxyPresentation({...input(),activity:'stopping'}).label,'正在关闭代理');
  assert.equal(proxyPresentation({...input(),readIssue:'IPC timeout'}).active,false);
  assert.equal(proxyPresentation({...input('tun','stopped',false),stopIssue:'cleanup failed'}).label,'关闭未完成');
  assert.equal(proxyPresentation(input('system_proxy','crashed',false)).state,'error');
});

const main=read('main.ts');
function stopFixture(mode='system_proxy', fail=false) {
  const calls=[];
  const noop=()=>{};
  const environment={runtimeActionInFlight:false,networkModeSwitching:false,settingsSaving:false,manualModeSwitching:false,domainRouteChanging:false,runtimeActivity:null,proxyStopIssue:'',runtimeMutationRevision:0,
    store:{settings:{networkMode:mode},runtime:{phase:'running'},systemProxy:{active:mode==='system_proxy'},activeProfile:{}},
    desiredRunning:true,routeEnabled:true,
    connectionFeedback:{clearIssue:noop,showError:()=>calls.push('error')},
    localRouting:{invalidate:()=>calls.push('invalidate'),refresh:async()=>{assert.equal(environment.runtimeActionInFlight,false);calls.push('route-read');}},
    themeController:{snapshot:{}},renderHeader:noop,renderOverview:noop,renderAppearance:noop,
    toast:()=>calls.push('success'),errorMessage:e=>e.message,friendlyError:e=>({title:e.message,description:'',action:'refresh',details:''}),
    api:{stop:async()=>{calls.push('stop');environment.desiredRunning=false;if(fail)throw Error('cleanup failed');environment.store.systemProxy.active=false;return {phase:'stopped'};}},
    refreshRuntimeOnly:async allowed=>{assert.equal(allowed,true);assert.equal(environment.runtimeActionInFlight,true);calls.push('readback');},refreshSessionResume:async()=>calls.push('resume-read'),
  };
  vm.createContext(environment);
  vm.runInContext(compile(main.slice(main.indexOf('async function stopRuntime()'),main.indexOf('function toggleGlobalNetworkMode('))),environment);
  return {environment,calls,stop:()=>environment.stopRuntime()};
}
test('ordinary Stop preserves the last mode and route policy while cancelling running intent', async () => {
  for(const mode of ['system_proxy','tun']) {
    const f=stopFixture(mode);await f.stop();
    assert.equal(f.environment.store.settings.networkMode,mode);
    assert.equal(f.environment.routeEnabled,true);
    assert.equal(f.environment.desiredRunning,false);
    assert.equal(f.environment.store.runtime.phase,'stopped');
    assert.deepEqual(f.calls,['invalidate','stop','success','readback','invalidate','route-read','resume-read']);
  }
});
test('failed Stop is visible, keeps actual running state and still refreshes route state', async () => {
  const f=stopFixture('tun',true);await f.stop();
  assert.equal(f.environment.proxyStopIssue,'cleanup failed');
  assert.equal(f.environment.store.runtime.phase,'running');
  assert.equal(f.environment.routeEnabled,true);
  assert.equal(f.environment.desiredRunning,false);
  assert.ok(!f.calls.includes('success'));assert.ok(f.calls.includes('error'));assert.ok(f.calls.includes('route-read'));
});
test('Stop does not overlap a runtime, settings, mode or route write', async () => {
  for (const busy of ['runtimeActionInFlight','networkModeSwitching','settingsSaving','manualModeSwitching','domainRouteChanging']) {
    const f=stopFixture();f.environment[busy]=true;await f.stop();assert.deepEqual(f.calls,[],busy);
  }
});

function baseReadFixture() {
  const noop=()=>{},calls=[],elements=new Map();
  const element=id=>{if(!elements.has(id))elements.set(id,{textContent:'',disabled:false,title:'',classList:{toggle:noop},setAttribute:noop});return elements.get(id);};
  const profile={id:'p',activeRevisionId:'r',displayName:'测试订阅'};
  let failRuntimeOnce=false,settingsWait=null,osProxyActive=true,corePhase='running';
  const environment={
    store:{view:'overview',settings:{networkMode:'system_proxy',theme:'system'},runtime:{phase:'running'},systemProxy:{active:true},activeProfile:{profile},profiles:[profile],subscriptions:[],tunHelper:null,manualOutbound:null,selectedProfile:null},
    runtimeActionInFlight:false,networkModeSwitching:false,domainRouteChanging:false,settingsSaving:false,manualModeSwitching:false,
    runtimeActivity:null,proxyReadIssue:'',proxyStopIssue:'',verifiedIngress:null,runtimeMutationRevision:0,baseReadSequence:0,runtimeReadSequence:0,proxyReadSequence:0,overviewNodeDetails:{},baseLoading:false,
    sessionResumeStatus:null,startupStatus:null,NAV_ITEMS:[{id:'overview',label:'概览'}],desiredRunning:true,
    proxyPresentation,canStartRuntime,canStopSession,sessionResumePresentation:()=>({busy:false}),modeLabel:m=>m,
    document:{documentElement:{dataset:{}}},$:element,pageReads:new PageReads(),dataContext:()=>'',
    action:async(_message,run)=>run(),errorMessage:e=>e.message,friendlyError:e=>({title:e.message,description:'',action:'refresh',details:''}),
    localRouting:{render:noop,invalidate:()=>calls.push('invalidate'),refresh:async()=>calls.push('route-read')},
    connectionFeedback:{clearIssue:noop,showError:noop},themeController:{mutationRevision:0,snapshot:{preference:'system'},sync:()=>true},
    renderOverview:noop,renderProfiles:noop,renderSubscriptions:noop,renderManualMode:noop,renderSettings:noop,renderOpenAiPolicy:noop,renderGlobalTraffic:noop,renderTunHelper:noop,renderAppearance:noop,
    invalidateNodeDetailsContext:noop,newestSubscriptionStatus:(_old,next)=>next,synchronizedProfiles:profiles=>profiles,
    openAiCosts:{refresh:async()=>{}},scheduleAutomaticUpdateCheck:noop,refreshSessionResume:async()=>{},refreshProxies:async()=>{},refreshConnectionFeedback:async()=>{},toast:noop,
    api:{
      appInfo:async()=>({version:'0.7.36',targetOs:'windows',targetArch:'x86_64'}),
      settings:async()=>{if(settingsWait)await settingsWait;return {networkMode:'system_proxy',theme:'system'};},
      binary:async()=>({}),runtime:async()=>{calls.push('runtime');if(failRuntimeOnce){failRuntimeOnce=false;throw Error('transient IPC timeout');}return {phase:corePhase};},
      systemProxy:async()=>({active:osProxyActive}),profiles:async()=>[profile],subscriptions:async()=>[],activeProfile:async()=>({profile}),
      openAiPolicyTask:async()=>null,globalTraffic:async()=>null,manualOutbound:async()=>null,tunHelperStatus:async()=>null,
      stop:async()=>{calls.push('stop');corePhase='stopped';osProxyActive=false;environment.desiredRunning=false;return {phase:'stopped'};},
    },
  };
  vm.createContext(environment);
  const sources=[
    main.slice(main.indexOf('function currentProxyPresentation()'),main.indexOf('function renderOverview()')),
    main.slice(main.indexOf('function refreshRuntimeOnly('),main.indexOf('async function startRuntime(')),
    main.slice(main.indexOf('async function performBaseRead()'),main.indexOf('function renderSessionResume()')),
    main.slice(main.indexOf('async function stopRuntime()'),main.indexOf('function toggleGlobalNetworkMode(')),
  ];
  vm.runInContext(compile(sources.join('\n')),environment);
  return {environment,calls,control:element,failNextRuntime:()=>{failRuntimeOnce=true;},waitForSettings:promise=>{settingsWait=promise;},refresh:()=>environment.performBaseRead(),stop:()=>environment.stopRuntime()};
}

test('global refresh recovers Start after Stop succeeds but its immediate status read fails',async()=>{
  const f=baseReadFixture();f.failNextRuntime();await f.stop();
  assert.equal(f.environment.store.runtime.phase,'stopped');
  assert.equal(f.environment.desiredRunning,false);
  assert.equal(f.environment.proxyReadIssue,'transient IPC timeout');
  assert.equal(f.control('#global-start').disabled,true);
  assert.equal(f.control('#application-runtime-state').textContent,'状态待确认');
  assert.equal(await f.refresh(),true);
  assert.equal(f.environment.proxyReadIssue,'');
  assert.equal(f.control('#application-runtime-state').textContent,'已关闭');
  assert.equal(f.control('#global-start').disabled,false);
  assert.equal(f.control('#global-stop').disabled,true);
  assert.deepEqual(f.calls.filter(c=>c==='stop'),['stop'],'refresh is read-only');
});

test('a base read invalidated by a newer operation cannot clear the unresolved status error',async()=>{
  const f=baseReadFixture();f.environment.proxyReadIssue='unresolved IPC error';f.environment.renderHeader();
  let release;const gate=new Promise(resolve=>{release=resolve;});f.waitForSettings(gate);
  const reading=f.refresh();await new Promise(resolve=>setImmediate(resolve));
  f.environment.runtimeMutationRevision++;release();
  assert.equal(await reading,false);
  assert.equal(f.environment.proxyReadIssue,'unresolved IPC error');
  f.environment.renderHeader();assert.equal(f.control('#global-start').disabled,true);
});

test('an incomplete Stop leaves retry Stop available and forbids starting a duplicate live core',async()=>{
  const f=baseReadFixture();f.environment.api.stop=async()=>{f.environment.desiredRunning=false;throw Error('proxy cleanup failed');};
  await f.stop();
  assert.equal(f.environment.store.runtime.phase,'running');
  assert.equal(f.control('#application-runtime-state').textContent,'关闭未完成');
  assert.equal(f.control('#global-start').disabled,true);
  assert.equal(f.control('#global-stop').disabled,false);
});

test('native Stop signals invalidate route state and read the current runtime without changing it',async()=>{
  const calls=[];
  const env={appLifecycle:{paused:false},runtimeActionInFlight:false,networkModeSwitching:false,settingsSaving:false,manualModeSwitching:false,domainRouteChanging:false,runtimeMutationRevision:7,proxyStopIssue:'',
    localRouting:{invalidate:()=>calls.push('invalidate'),refresh:async()=>calls.push('route-read')},
    refreshRuntimeOnly:async()=>calls.push('runtime-read'),refreshSessionResume:async()=>calls.push('resume-read')};
  vm.createContext(env);
  vm.runInContext(compile(main.slice(main.indexOf('async function refreshAfterNativeStop('),main.indexOf('async function startRuntime('))),env);
  await env.refreshAfterNativeStop(true);
  assert.equal(env.runtimeMutationRevision,8);
  assert.deepEqual(calls,['invalidate','runtime-read','route-read','resume-read']);
  calls.length=0;await env.refreshAfterNativeStop(false);assert.match(env.proxyStopIssue,/清理未完成/);
  assert.deepEqual(calls,['invalidate','runtime-read','route-read','resume-read']);
  for(const busy of ['runtimeActionInFlight','networkModeSwitching','settingsSaving','manualModeSwitching','domainRouteChanging']) {
    env[busy]=true;calls.length=0;await env.refreshAfterNativeStop();assert.deepEqual(calls,[],busy);env[busy]=false;
  }
  env.appLifecycle.paused=true;calls.length=0;await env.refreshAfterNativeStop();assert.deepEqual(calls,[]);
});
