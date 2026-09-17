import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
const read = name => readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
const url = source => `data:text/javascript;base64,${Buffer.from(ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText).toString("base64")}`;
const {validateRouteSettings,routeSettingsError,modelHealthLabel,probeLabel,routeDiagnosticLabel,localRoutingMarkup,routeMetricsMarkup,mountLocalRouting} = await import(url(read("local-routing.ts").replace('"./ui"',JSON.stringify(url(read("ui.ts"))))));
const base = {listenPort:15731,mode:"compatible",upstream:"chatgpt",outboundProxy:""};
test("dual probe labels distinguish reachability, unverified HTTP and stale observations",()=>{
  const report={state:"passed",checkedAt:100,latencyMs:80};
  assert.match(probeLabel(report,110000),/基础检测通过/);
  assert.doesNotMatch(probeLabel(report,110000),/模型.*已验证/);
  assert.match(probeLabel({...report,state:"http_unverified"},110000),/预期响应未验证/);
  assert.match(probeLabel({...report,state:"failed"},110000),/检测失败/);
  assert.match(probeLabel(report,341000),/已过期/);
  assert.match(probeLabel(undefined),/未检测/);
});
test("local 502 is not mislabeled an upstream HTTP 502; diagnostic has stage, time and attribution",()=>{
  const d={timestamp:100000,target:"chatgpt",stage:"request",elapsedMs:10000,httpStatus:null,attribution:"unconfirmed",code:"upstream_tls_failed",message:"TLS 握手中断"};
  assert.match(routeDiagnosticLabel(d),/ChatGPT.*发送请求.*10000 ms.*未收到上游 HTTP 响应.*实际出口未确认/);
  assert.match(routeDiagnosticLabel({...d,httpStatus:429}),/上游 HTTP 429/);
});
test("route status exposes both targets and distinguishes enabled from running without mutation",async()=>{
  const h=routeHarness();
  const report={state:"http_unverified",checkedAt:Math.floor(Date.now()/1000),latencyMs:80};
  h.snapshot.stability={...h.snapshot.stability,enabled:true,running:false,selectionTarget:"openai_api",lastCheck:report.checkedAt,commonFailure:true,nodes:[{name:"<img src=x>",probeOk:false,successRate:null,samples:0,cooldownSeconds:0,recoveryPasses:0,modelCompleted:0,modelInterrupted:0,chatgpt:report,openaiApi:{...report,state:"failed"}}]};
  await h.controller.refresh();
  assert.match(h.control("check-state").textContent,/已开启 · 自动检测未运行.*OpenAI API.*多出口同时失败/);
  assert.match(h.control("nodes").innerHTML,/ChatGPT：HTTP 可达，预期响应未验证/);
  assert.match(h.control("nodes").innerHTML,/OpenAI API：检测失败/);
  assert.doesNotMatch(h.control("nodes").innerHTML,/<img/);
  assert.deepEqual(h.calls,[["status"]]);
});
test("loopback routes reject secrets, remote destinations, self loops, malformed ports",()=>{
  assert.equal(validateRouteSettings(base),null);
  for(const outboundProxy of ["http://remote.invalid:7890","http://user:pass@127.0.0.1:7890","http://127.0.0.1:15731","http://localhost:7890/path","http://localhost:7890?token=x","http://localhost"]) assert.ok(validateRouteSettings({...base,outboundProxy}));
  for(const listenPort of [0,1023,65536,1.5,NaN]) assert.ok(validateRouteSettings({...base,listenPort}));
  assert.equal(validateRouteSettings({...base,outboundProxy:"socks5h://127.0.0.1:7890"}),null);
});
test("basic reachability never claims a model stream is verified",()=>{
  assert.equal(modelHealthLabel({probeOk:true,modelCompleted:0,modelInterrupted:0}),"模型流尚未验证");
  assert.match(modelHealthLabel({modelCompleted:2,modelInterrupted:0}),/已完成 2/);
  assert.match(modelHealthLabel({modelCompleted:2,modelInterrupted:1}),/1 次模型流异常/);
});
test("route defaults are off with distinct save, enable, attach and restore actions",()=>{
  assert.doesNotMatch(localRoutingMarkup, /\bchecked\b/);
  const ids=[...localRoutingMarkup.matchAll(/\bid="([^"]+)"/g)].map(x=>x[1]);
  assert.equal(new Set(ids).size,ids.length);
  for(const id of ["local-route-enabled","local-route-save","local-route-attach","local-route-restore","local-route-stability"]) assert.ok(ids.includes(id));
  assert.match(localRoutingMarkup,/保存不启动、不接入/);
  assert.match(localRoutingMarkup,/默认关闭/);
});
test("controller doesn't mutate global proxy or launch/close processes",()=>{
  const source=read("local-routing.ts");
  assert.doesNotMatch(source,/setNetworkMode|launchProxyProgram|stopRuntime|window\.open|setInterval/);
  assert.match(source,/expectedConfigRevision/);assert.match(source,/draftRevision/);
});

test("approved workspace keeps safety details and has no decorative English sections", () => {
  assert.match(localRoutingMarkup, /local-route-workspace/);
  assert.match(localRoutingMarkup, /<details class="local-route-help">/);
  assert.match(localRoutingMarkup, /使用与恢复说明/);
  for (const text of ["不修改 auth.json", "默认关闭", "恢复后若原入口是 CC Switch", "不清空正常连接", "不自动重发模型请求", "兼容模式也不保证永不断线"])
    assert.ok(localRoutingMarkup.includes(text), text);
  assert.doesNotMatch(localRoutingMarkup, /LOCAL ROUTING|TRANSPORT|CODEX CONNECTION|STABILITY FIRST|section-label/);
  const ids = [...localRoutingMarkup.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]);
  assert.equal(new Set(ids).size, ids.length);
  const css = read("local-routing.css");
  assert.match(css, /@container local-route \(max-width: 880px\)/);
  assert.match(css, /grid-template-areas: "service" "transport" "binding" "statistics"/);
});

test("unread counters are dashes while confirmed zero counts remain zero", () => {
  assert.equal((routeMetricsMarkup(null).match(/<dd>—<\/dd>/g) ?? []).length, 4);
  assert.equal((routeMetricsMarkup({requests:0,active:0,completed:0,failed:0}).match(/<dd>0<\/dd>/g) ?? []).length, 4);
  assert.match(routeMetricsMarkup(null), /转发完成/);
  assert.match(routeMetricsMarkup(null), /未确认完整（含取消）/);
  assert.equal(routeSettingsError({...base,listenPort:80}).field, "port");
  assert.equal(routeSettingsError({...base,outboundProxy:"https://remote.invalid:7890"}).field, "proxy");
});

test("route glass surfaces have solid, blur-free accessibility and unsupported fallbacks", () => {
  const css = read("local-routing.css");
  for (const condition of [/@supports not \(backdrop-filter: blur\(1px\)\) \{([\s\S]*?)\n\}/, /@media \(prefers-reduced-transparency: reduce\) \{([\s\S]*?)\n\}/]) {
    const block = css.match(condition)?.[1];
    assert.ok(block, `missing fallback ${condition}`);
    assert.match(block, /\.local-route-workspace,\s*\.local-route-stability-panel/);
    assert.match(block, /background: var\(--panel-solid\)/);
    assert.match(block, /backdrop-filter: none;/);
    assert.match(block, /-webkit-backdrop-filter: none;/);
  }
  const forcedColors = css.match(/@media \(forced-colors: active\) \{([\s\S]*?)\n\}/)?.[1];
  assert.match(forcedColors, /\.local-route-workspace,\s*\.local-route-stability-panel \{ backdrop-filter: none; -webkit-backdrop-filter: none; \}/);
  assert.match(forcedColors, /background: Canvas/);
  assert.match(css.slice(0, css.indexOf("@supports not (backdrop-filter")), /backdrop-filter: blur\(var\(--glass-blur, 20px\)\)/);
});

function routeHarness(overrides = {}) {
  let snapshot = {
    revision: 1, enabled: false, running: false, settings: {...base}, endpoint: "http://127.0.0.1:15731",
    requests: 0, active: 0, completed: 0, failed: 0, lastStatus: 0, lastError: null,
    codex: {configRevision:"config-1",attached:false,hasBackup:false,provider:"openai",endpoint:null,warning:null,backupPath:null},
    stability: {enabled:false,eligible:false,running:false,profileId:null,revisionId:null,current:null,message:"",nodes:[]},
    ...overrides,
  };
  const elements = new Map(), calls = [], confirmations = [], answers = [], failures = new Map();
  const root = {attributes: new Map(), setAttribute(name,value) { this.attributes.set(name,value); }, querySelector(selector) { return elements.get(selector.slice(1)); }};
  const control = id => elements.get(`local-route-${id}`);
  for (const [,id] of localRoutingMarkup.matchAll(/\bid="([^"]+)"/g)) {
    elements.set(id, {id,disabled:false,checked:false,hidden:false,value:"",textContent:"",innerHTML:"",dataset:{},attributes:new Map(),listeners:new Map(),
      addEventListener(name,handler) { const handlers=this.listeners.get(name)??[]; handlers.push(handler); this.listeners.set(name,handlers); },
      setAttribute(name,value) { this.attributes.set(name,value); }, removeAttribute(name) { this.attributes.delete(name); },
      focus() { root.focused = this.id; },
    });
  }
  const reply = () => structuredClone(snapshot);
  const api = {
    async localRouteStatus() { calls.push(["status"]); if (failures.has("status")) throw failures.get("status"); return reply(); },
    async saveLocalRoute(settings,revision) {
      calls.push(["save",structuredClone(settings),revision]);
      if (revision !== snapshot.revision) throw new Error("路由设置已变化，请刷新后重试");
      if (failures.has("save")) throw failures.get("save");
      snapshot.settings = structuredClone(settings); snapshot.revision++; return reply();
    },
    async setLocalRouteEnabled(enabled,revision,confirmed) {
      calls.push(["enable",enabled,revision,confirmed]);
      if (failures.has("enable")) throw failures.get("enable");
      if (!enabled && snapshot.active > 0) throw new Error("仍有进行中的路由请求，请等待结束后关闭；未中断请求");
      snapshot.enabled=enabled; snapshot.running=enabled; snapshot.revision++;
      if (!enabled) snapshot.codex={...snapshot.codex,attached:false,hasBackup:false,backupPath:null,provider:"openai",endpoint:null};
      return reply();
    },
    async setCodexRoute(attach,revision,confirmed) {
      calls.push(["codex",attach,revision,confirmed]);
      if (failures.has("codex")) throw failures.get("codex");
      snapshot.codex={...snapshot.codex,attached:attach,hasBackup:attach,provider:attach?"routedeck":"openai",backupPath:attach?"C:/synthetic/codex-backup.toml":null};
      return reply();
    },
    async setOpenAiStability(enabled,profileId,revisionId,confirmed) {
      calls.push(["stability",enabled,profileId,revisionId,confirmed]); snapshot.stability.enabled=enabled;
    },
  };
  const controller = mountLocalRouting(root, {api,confirm: async options => { confirmations.push(options); return answers.length ? await answers.shift() : true; },error: e => e.message});
  const fire = (id,event,values={}) => { const element=control(id); Object.assign(element,values); for(const fn of element.listeners.get(event)??[]) fn({target:element,currentTarget:element,preventDefault(){}}); };
  const settle = () => new Promise(resolve => setImmediate(resolve));
  return {root,control,calls,confirmations,answers,failures,api,controller,fire,settle,get snapshot(){return snapshot;},set snapshot(value){snapshot=value;}};
}

test("reading routing state is read-only and distinguishes zero from no model evidence", async () => {
  const h=routeHarness();
  assert.equal(h.control("save").disabled,true);
  assert.equal(h.control("enabled").disabled,true);
  assert.equal(h.control("attach").disabled,true);
  await h.controller.refresh();
  assert.deepEqual(h.calls,[["status"]]);
  assert.equal(h.control("enabled").checked,false);
  assert.equal(h.control("save").disabled,false);
  assert.equal(h.control("attach").disabled,true);
  assert.equal(h.control("restore").disabled,true);
  assert.equal(h.control("stability").disabled,true);
  assert.match(h.control("metrics").innerHTML,/<dd>0<\/dd>/);
  assert.match(h.control("evidence-message").textContent,/无模型样本/);
});

test("draft prevents implicit start; save has no confirmation and starts nothing", async () => {
  const h=routeHarness(); await h.controller.refresh();
  h.control("port").value="15732"; h.fire("form","input");
  assert.equal(h.control("enabled").disabled,true);
  assert.equal(h.control("reset").disabled,false);
  assert.match(h.control("form-help").textContent,/未保存/);
  h.fire("enabled","change",{checked:true}); await h.settle();
  assert.equal(h.control("enabled").checked,false);
  assert.equal(h.confirmations.length,0);
  h.fire("form","submit"); await h.settle();
  assert.equal(h.calls.filter(([name])=>name==="save").length,1);
  assert.equal(h.confirmations.length,0);
  assert.equal(h.snapshot.settings.listenPort,15732);
  assert.equal(h.snapshot.enabled,false);
  assert.equal(h.snapshot.codex.attached,false);
  assert.equal(h.control("enabled").disabled,false);
  assert.match(h.control("feedback").textContent,/服务和 Codex 接入未自动开启/);
});

test("validation keeps draft and focuses the invalid field without writing", async () => {
  const h=routeHarness(); await h.controller.refresh();
  h.control("port").value="80"; h.fire("form","input"); h.fire("form","submit");
  assert.equal(h.control("port").value,"80");
  assert.equal(h.control("port").attributes.get("aria-invalid"),"true");
  assert.match(h.control("port-error").textContent,/1024–65535/);
  assert.equal(h.root.focused,"local-route-port");
  assert.equal(h.calls.length,1);
  h.fire("reset","click");
  assert.equal(h.control("port").value,"15731");
  assert.equal(h.control("port").attributes.has("aria-invalid"),false);
  assert.equal(h.control("port-error").hidden,true);
});

test("refresh conflicts preserve draft and stale revision cannot silently overwrite", async () => {
  const h=routeHarness(); await h.controller.refresh();
  h.control("port").value="15732"; h.fire("form","input");
  h.snapshot.revision=2; h.snapshot.settings.listenPort=15733;
  await h.controller.refresh();
  assert.equal(h.control("port").value,"15732");
  assert.match(h.control("feedback").textContent,/编辑内容已保留/);
  assert.equal(h.control("feedback").dataset.tone,"warning");
  h.fire("form","submit"); await h.settle();
  assert.equal(h.calls.at(-1)[2],1);
  assert.equal(h.snapshot.settings.listenPort,15733);
  assert.equal(h.control("port").value,"15732");
  h.fire("reset","click");
  assert.equal(h.control("port").value,"15733");
});

test("cancel start keeps the switch off and returns keyboard focus", async () => {
  const h=routeHarness(); await h.controller.refresh(); h.answers.push(false);
  h.fire("enabled","change",{checked:true}); await h.settle();
  assert.equal(h.confirmations[0].title,"启动本地路由？");
  assert.match(h.confirmations[0].message,/不会自动修改 Codex 配置/);
  assert.equal(h.confirmations[0].returnFocus,h.control("enabled"));
  assert.equal(h.calls.some(([name])=>name==="enable"),false);
  assert.equal(h.control("enabled").checked,false);
  assert.equal(h.root.focused,"local-route-enabled");
});

test("switch waits for confirmation and operation result, then Codex attach is separate", async () => {
  const h=routeHarness(); await h.controller.refresh();
  let confirm; h.answers.push(new Promise(resolve=>{confirm=resolve;}));
  h.fire("enabled","change",{checked:true});
  assert.equal(h.control("enabled").checked,false);
  assert.equal(h.control("enabled").disabled,true);
  assert.equal(h.control("pending").hidden,false);
  assert.equal(h.calls.some(([name])=>name==="enable"),false);
  confirm(true); await h.settle();
  assert.equal(h.control("enabled").checked,true);
  assert.equal(h.control("save").disabled,true);
  assert.equal(h.control("attach").disabled,false);
  assert.equal(h.calls.some(([name])=>name==="codex"),false);
  h.fire("attach","click"); await h.settle();
  assert.equal(h.confirmations[1].title,"备份并接入 Codex？");
  assert.match(h.confirmations[1].message,/不退出 Codex/);
  assert.deepEqual(h.calls.at(-1),["codex",true,"config-1",true]);
  assert.equal(h.control("binding").textContent,"已写入 Serylane 接入配置");
  assert.match(h.control("feedback").textContent,/尚未验证新会话是否生效/);
  assert.equal(h.control("attach").disabled,true);
  assert.equal(h.control("restore").disabled,false);
  assert.equal(h.control("backup").hidden,false);
});

test("existing or conflicting backup locks settings and duplicate attach; restore failure retains service", async () => {
  const h=routeHarness({enabled:true,running:true});
  h.snapshot.codex={...h.snapshot.codex,hasBackup:true,attached:false,warning:"配置已被其他程序修改",backupPath:"C:/synthetic/backup.toml"};
  await h.controller.refresh();
  assert.equal(h.control("save").disabled,true);
  assert.equal(h.control("attach").disabled,true);
  assert.equal(h.control("restore").disabled,false);
  h.fire("attach","click"); await h.settle();
  assert.equal(h.confirmations.length,0);
  h.failures.set("codex",new Error("Codex 接入已被其他程序修改，未覆盖；原始备份仍保留"));
  h.fire("restore","click"); await h.settle();
  assert.match(h.confirmations[0].message,/只还原 Serylane 管理的字段/);
  assert.equal(h.snapshot.running,true);
  assert.equal(h.control("enabled").checked,true);
  assert.equal(h.control("backup").hidden,false);
  assert.equal(h.control("feedback").dataset.tone,"warning");
  assert.equal(h.control("restore").disabled,false);
});

test("active requests disable attach and restore and a refused close does not clear state", async () => {
  const h=routeHarness({enabled:true,running:true,active:1});
  h.snapshot.codex.hasBackup=true; h.snapshot.codex.attached=true;
  await h.controller.refresh();
  assert.equal(h.control("attach").disabled,true);
  assert.equal(h.control("restore").disabled,true);
  h.fire("restore","click"); assert.equal(h.confirmations.length,0);
  h.fire("enabled","change",{checked:false}); await h.settle();
  assert.equal(h.confirmations[0].title,"关闭路由并恢复接入？");
  assert.match(h.confirmations[0].message,/有进行中请求时将拒绝关闭/);
  assert.equal(h.snapshot.active,1);
  assert.equal(h.control("enabled").checked,true);
  assert.match(h.control("feedback").textContent,/未中断请求/);
});

test("restore alone does not stop the local service and startup errors do not claim success", async () => {
  const h=routeHarness({enabled:true,running:true}); h.snapshot.codex.hasBackup=true; h.snapshot.codex.attached=true;
  await h.controller.refresh(); h.fire("restore","click"); await h.settle();
  assert.equal(h.snapshot.running,true);
  assert.equal(h.snapshot.codex.attached,false);
  assert.match(h.control("feedback").textContent,/历史备份仍保留/);
  const failed=routeHarness(); await failed.controller.refresh();
  failed.failures.set("enable",new Error("路由端口无法监听或已被占用；未停止占用端口的程序"));
  failed.fire("enabled","change",{checked:true}); await failed.settle();
  assert.equal(failed.control("enabled").checked,false);
  assert.equal(failed.control("status").textContent,"已关闭");
  assert.equal(failed.control("attach").disabled,true);
  assert.match(failed.control("feedback").textContent,/未停止占用端口的程序/);
});

test("eligible stability changes require confirmation and preserve unverified probe semantics", async () => {
  const h=routeHarness();
  h.snapshot.stability={...h.snapshot.stability,eligible:true,profileId:"profile-a",revisionId:"revision-a",nodes:[{name:"<node>",modelCompleted:0,modelInterrupted:0,probeOk:true,cooldownSeconds:0,samples:3,successRate:100}]};
  await h.controller.refresh();
  assert.equal(h.control("nodes").hidden,false);
  assert.match(h.control("nodes").innerHTML,/&lt;node&gt;/);
  assert.match(h.control("nodes").innerHTML,/模型流尚未验证/);
  h.answers.push(false); h.fire("stability","change",{checked:true}); await h.settle();
  assert.equal(h.calls.some(([name])=>name==="stability"),false);
  assert.equal(h.control("stability").checked,false);
  h.fire("stability","change",{checked:true}); await h.settle();
  assert.match(h.confirmations.at(-1).message,/不主动清空连接/);
  assert.deepEqual(h.calls.find(([name])=>name==="stability"),["stability",true,"profile-a","revision-a",true]);
  assert.equal(h.control("stability").checked,true);
});
