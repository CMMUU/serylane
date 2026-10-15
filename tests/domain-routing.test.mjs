import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const read=f=>readFileSync(new URL(`../${f}`,import.meta.url),'utf8');
const url=s=>`data:text/javascript;base64,${Buffer.from(ts.transpileModule(s,{compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText).toString('base64')}`;
const {mountDomainRouting,domainRoutingMarkup}=await import(url(read('src/domain-routing.ts').replace("'./node-selection'",JSON.stringify(url(read('src/node-selection.ts'))))));
const clone=v=>structuredClone(v);
function deferred(){let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};}
const settle=async()=>{for(let i=0;i<8;i++)await new Promise(r=>setImmediate(r));};
function harness(enabled=false){
 const elements=new Map();
 const element=id=>{if(!elements.has(id))elements.set(id,{id,textContent:'',innerHTML:'',value:'',disabled:false,hidden:false,dataset:{},attrs:{},listeners:{},focused:false,focus(){this.focused=true;},classList:{values:{},toggle(k,v){this.values[k]=v;}},setAttribute(k,v){this.attrs[k]=v;},addEventListener(k,f){this.listeners[k]=f;}});return elements.get(id);};
 const tool={checked:true,dataset:{tool:'openai'}};
 const root={querySelector:s=>element(s.replace('#domain-','')),querySelectorAll:s=>s.endsWith(':checked')?(tool.checked?[tool]:[]):[tool]};
 const draft={tools:['openai'],customDomains:[],domains:['openai.com'],target:'出口'};
 let snapshot={document:{schemaVersion:1,revision:1,enabled,draft,applied:enabled?clone(draft):null},presets:[{id:'openai',name:'OpenAI',domains:['openai.com'],note:''}],targets:['出口'],profileId:'p',profileRevision:'r',status:enabled?'已应用 · 仅处理进入代理核心的流量':'已关闭',warning:'规则说明',userRuleCount:0};
 const calls=[],confirmations=[],answers=[],reads=[],writes=[],notices=[],busy=[];
 let context='p/r/running',blocked=false;
 let opened=0;
 const services={openSetup:()=>opened++,globalToggle:element('global'),isBlocked:()=>blocked,contextKey:()=>context,busyChanged:b=>busy.push(b),reportError:s=>notices.push(s),error:e=>e.message,
 confirm:async o=>{confirmations.push(o);return answers.length?await answers.shift():true;},
 api:{domainRouteStatus:async()=>{calls.push(['read']);return reads.length?await reads.shift():clone(snapshot);},legacyRouteStatus:async()=>null,
 saveDomainRoute:async(policy,revision)=>{calls.push(['save',clone(policy),revision]);if(revision!==snapshot.document.revision)throw Error('修订冲突');snapshot.document={...snapshot.document,revision:revision+1,draft:{...policy,domains:[...(policy.tools.includes('openai')?['openai.com']:[]),...policy.customDomains]}};return clone(snapshot);},
 applyDomainRoute:async(enabled,expected)=>{calls.push(['apply',enabled,expected.document.revision]);if(writes.length)await writes.shift();if(expected.document.revision!==snapshot.document.revision)throw Error('修订冲突');snapshot.document={...snapshot.document,enabled,revision:expected.document.revision+1,applied:enabled?clone(snapshot.document.draft):snapshot.document.applied};snapshot.status=enabled?'已应用 · 仅处理进入代理核心的流量':'已关闭';return clone(snapshot);}}};
 const controller=mountDomainRouting(root,services);
 return {get opened(){return opened;},controller,control:element,calls,confirmations,answers,reads,writes,notices,busy,tool,get snapshot(){return snapshot;},set snapshot(v){snapshot=v;},set context(v){context=v;},set blocked(v){blocked=v;},fire(id,type='click',values={}){Object.assign(element(id),values);element(id).listeners[type]?.({preventDefault(){}});},settle};
}
test('both entry points share authoritative state, confirmation and one mutation lock',async()=>{
 const h=harness();assert.equal(h.control('global').disabled,true);await h.controller.refresh();
 const gate=deferred();h.answers.push(gate.promise);h.fire('global');await settle();
 assert.equal(h.control('global').textContent,'正在开启…');assert.equal(h.control('toggle').disabled,true);assert.equal(h.control('global').attrs['aria-pressed'],'false');
 h.fire('toggle');assert.equal(h.calls.filter(c=>c[0]==='apply').length,0);assert.equal(h.confirmations.length,1);
 gate.resolve(true);await settle();assert.equal(h.control('global').attrs['aria-pressed'],'true');assert.equal(h.control('toggle').textContent,'关闭路由');assert.equal(h.calls.filter(c=>c[0]==='apply').length,1);
 assert.equal(h.confirmations[0].returnFocus,h.control('global'));
 h.fire('toggle');await settle();assert.equal(h.control('global').attrs['aria-pressed'],'false');assert.equal(h.control('toggle').textContent,'开启路由');assert.equal(h.confirmations.at(-1).returnFocus,h.control('toggle'));
});
test('cancel and failed write never claim success; busy clears and errors reach global caller',async()=>{
 const h=harness();await h.controller.refresh();h.answers.push(false);h.fire('toggle');await settle();assert.equal(h.snapshot.document.enabled,false);assert.equal(h.calls.filter(c=>c[0]==='apply').length,0);
 const gate=deferred();h.writes.push(gate.promise);h.fire('global');await settle();gate.reject(Error('合成应用失败'));await settle();assert.equal(h.control('global').attrs['aria-pressed'],'false');assert.match(h.control('feedback').textContent,/合成应用失败/);assert.equal(h.notices.length,1);assert.equal(h.control('toggle').disabled,false);
});
test('dirty edits block enabling but permit disabling without erasing edits',async()=>{
 const h=harness(true);await h.controller.refresh();h.control('domains').value='example.com';h.fire('form','input');
 h.fire('global');await settle();assert.equal(h.snapshot.document.enabled,false);assert.equal(h.control('domains').value,'example.com');assert.equal(h.control('toggle').disabled,false);h.fire('toggle');await settle();assert.equal(h.control('save').focused,true);assert.equal(h.snapshot.document.enabled,false);
 h.fire('form','submit');await settle();assert.deepEqual(h.snapshot.document.draft.customDomains,['example.com']);assert.equal(h.snapshot.document.enabled,false);assert.equal(h.control('toggle').disabled,false);
});
test('saved changes apply while enabled, without an off/on cycle',async()=>{
 const h=harness(true);await h.controller.refresh();assert.equal(h.control('apply').disabled,true);
 h.control('domains').value='example.com';h.fire('form','input');h.fire('form','submit');await settle();assert.equal(h.snapshot.document.enabled,true);assert.equal(h.control('apply').disabled,false);
 h.fire('apply');await settle();assert.deepEqual(h.calls.filter(c=>c[0]==='apply').map(c=>c[1]),[true]);assert.deepEqual(h.snapshot.document.applied.customDomains,['example.com']);assert.equal(h.control('apply').disabled,true);
});
test('quiet refresh preserves dirty edits, expanded markup and focus-bearing DOM',async()=>{
 const h=harness();await h.controller.refresh();h.control('tools').innerHTML='expanded sentinel';await h.controller.refresh(true);assert.equal(h.control('tools').innerHTML,'expanded sentinel');
 h.control('domains').value='draft.example.com';h.fire('form','input');h.snapshot.document.revision++;await h.controller.refresh(true);assert.equal(h.control('domains').value,'draft.example.com');h.fire('form','submit');await settle();assert.match(h.control('feedback').textContent,/修订冲突/);
 h.fire('reset');assert.equal(h.control('domains').value,'');
});
test('late reads cannot overwrite a newer toggle',async()=>{
 const h=harness();await h.controller.refresh();const old=clone(h.snapshot),gate=deferred();h.reads.push(gate.promise);const reading=h.controller.refresh(true);
 h.fire('global');await settle();assert.equal(h.snapshot.document.enabled,true);gate.resolve(old);await reading;await settle();assert.equal(h.control('global').attrs['aria-pressed'],'true');
});
test('profile/context changes discard stale reads and cancel pending confirmed writes',async()=>{
 const h=harness();await h.controller.refresh();const gate=deferred(),old=clone(h.snapshot);h.reads.push(gate.promise);const reading=h.controller.refresh(true);h.context='p2/r2/running';h.snapshot.profileId='p2';h.snapshot.status='新配置';gate.resolve(old);await reading;assert.equal(h.control('status').textContent,'新配置');
 const confirmation=deferred();h.answers.push(confirmation.promise);h.fire('global');await settle();h.context='p3/r3/running';confirmation.resolve(true);await settle();assert.equal(h.calls.filter(c=>c[0]==='apply').length,0);assert.match(h.control('feedback').textContent,/配置已变化/);
});
test('invalid setup shows actionable message without confirming or starting core',async()=>{
 const h=harness();h.snapshot.document.draft.target='missing';await h.controller.refresh();h.fire('global');await settle();assert.equal(h.confirmations.length,0);assert.equal(h.calls.filter(c=>c[0]==='apply').length,0);assert.match(h.control('feedback').textContent,/有效出口/);assert.equal(h.opened,1);assert.equal(h.control('target').focused,true);
});
test('enabled preference does not replace waiting/ineffective runtime status',async()=>{
 const h=harness(true);for(const status of ['已配置 · 等待核心启动','已配置 · 当前非规则模式，不生效','需要重新选择出口 · 匹配流量拒绝连接']){h.snapshot.status=status;await h.controller.refresh(true);assert.equal(h.control('status').textContent,status);assert.match(h.control('global').title,new RegExp(status.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')));assert.equal(h.control('toggle').textContent,'关闭路由');}
});
test('external runtime operation blocks both entries and programmatic duplicate calls',async()=>{
 const h=harness();await h.controller.refresh();h.blocked=true;h.controller.render();h.fire('global');h.fire('toggle');await settle();assert.equal(h.control('global').disabled,true);assert.equal(h.confirmations.length,0);h.blocked=false;h.controller.render();assert.equal(h.control('global').disabled,false);
});
test('read failure reports unknown, keeps last known preference and allows retry',async()=>{
 const h=harness(true);await h.controller.refresh();const gate=deferred();h.reads.push(gate.promise);const task=h.controller.refresh();gate.reject(Error('状态读取失败'));await task;assert.match(h.control('status').textContent,/状态待核对/);assert.equal(h.control('global').attrs['aria-pressed'],'true');await h.controller.refresh();assert.match(h.control('status').textContent,/已应用/);
});
test('global and page controls exist; no duplicate footer close; help stays collapsed',()=>{
 assert.match(read('src/main.ts'),/id="global-tun"[\s\S]*id="global-domain-route"[\s\S]*id="global-refresh"/);
 assert.match(domainRoutingMarkup,/id="domain-toggle"[\s\S]*id="domain-refresh"/);assert.doesNotMatch(domainRoutingMarkup,/id="domain-disable"/);assert.match(domainRoutingMarkup,/<details class="domain-help">/);
});

test('disabling after an external draft change does not rebase unsaved edits silently',async()=>{
 const h=harness(true);await h.controller.refresh();h.control('domains').value='my-draft.example.com';h.fire('form','input');
 h.snapshot.document.revision++;h.snapshot.document.draft.customDomains=['external.example.com'];
 h.fire('global');await settle();assert.equal(h.snapshot.document.enabled,false);
 h.fire('form','submit');await settle();assert.match(h.control('feedback').textContent,/修订冲突/);assert.deepEqual(h.snapshot.document.draft.customDomains,['external.example.com']);
});


test('first use guides both entries to explicit exit without saving or applying implicitly',async()=>{
 const h=harness();h.snapshot.document.draft.target='';await h.controller.refresh();
 for(const id of ['global','toggle']){assert.equal(h.control(id).disabled,false);h.fire(id);await settle();}
 assert.equal(h.opened,2);assert.equal(h.control('target').focused,true);assert.equal(h.calls.filter(c=>['apply','save'].includes(c[0])).length,0);
 h.control('target').value='出口';h.fire('form','change');h.fire('global');await settle();assert.equal(h.control('save').focused,true);assert.equal(h.confirmations.length,0);
 h.fire('form','submit');await settle();assert.equal(h.snapshot.document.enabled,false);assert.equal(h.control('toggle').textContent,'开启路由');h.fire('toggle');await settle();assert.equal(h.snapshot.document.enabled,true);
});
test('reverting edits restores readiness without save; change events are supported',async()=>{
 const h=harness();await h.controller.refresh();h.control('domains').value='example.com';h.fire('form','input');assert.match(h.control('status').textContent,/编辑尚未保存/);
 h.control('domains').value='';h.fire('form','change');assert.equal(h.control('toggle').textContent,'开启路由');h.fire('global');await settle();assert.equal(h.confirmations.length,1);assert.equal(h.snapshot.document.enabled,true);
});
test('invalid custom input is retained, no save or enable side effects',async()=>{
 const h=harness();await h.controller.refresh();h.control('domains').value='https://example.com/private/path';h.fire('form','input');h.fire('global');h.fire('form','submit');await settle();assert.equal(h.snapshot.document.enabled,false);assert.equal(h.calls.filter(c=>['save','apply'].includes(c[0])).length,0);assert.match(h.control('feedback').textContent,/路径/);
});
test('no profile and wrong rule mode guide instead of submitting',async()=>{
 const h=harness();h.snapshot.profileId=null;await h.controller.refresh();h.fire('global');await settle();assert.match(h.control('feedback').textContent,/选用订阅/);
 h.snapshot.profileId='p';h.snapshot.readiness='wrong_mode';await h.controller.refresh();h.fire('global');await settle();assert.match(h.control('feedback').textContent,/规则模式/);assert.equal(h.confirmations.length,0);
});
test('runtime mismatch is not highlighted as verified and can reapply identical policy',async()=>{
 const h=harness(true);h.snapshot.verification='mismatch';h.snapshot.status='尚未生效 · 核心规则不一致';await h.controller.refresh();assert.equal(h.control('global').classList.values['is-active'],false);assert.equal(h.control('apply').disabled,false);assert.equal(h.control('apply').textContent,'重新应用');h.fire('apply');await settle();assert.equal(h.calls.filter(c=>c[0]==='apply').length,1);
 h.snapshot.verification='verified';await h.controller.refresh();assert.equal(h.control('global').classList.values['is-active'],true);assert.equal(h.control('apply').disabled,true);
});
test('sanitized native route details survive generic DTO title and failures retain edits',async()=>{
 const h=harness();await h.controller.refresh();const gate=deferred();h.writes.push(gate.promise);h.fire('global');await settle();gate.reject({message:'generic',userMessage:{title:'当前操作暂未完成',details:'路由规则核对失败，旧规则已恢复'}});await settle();assert.match(h.control('feedback').textContent,/旧规则已恢复/);assert.equal(h.control('global').disabled,false);assert.equal(h.snapshot.document.enabled,false);
});


test('fresh authoritative setup change guides focus after the mutation lock releases',async()=>{
 const h=harness();await h.controller.refresh();const gate=deferred();h.reads.push(gate.promise);h.fire('global');await settle();const next=clone(h.snapshot);next.document.draft.target='missing';next.document.revision++;h.snapshot=next;gate.resolve(next);await settle();assert.equal(h.control('target').focused,true);assert.equal(h.control('fields').disabled,false);assert.equal(h.calls.filter(c=>c[0]==='apply').length,0);
});
