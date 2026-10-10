import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
const source = readFileSync(new URL("../src/subscription-task.ts", import.meta.url), "utf8");
const { downloadOptions, observeSubscriptionTask, taskDescription } = await import(`data:text/javascript;base64,${Buffer.from(ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText).toString("base64")}`);
const tick = () => new Promise(resolve => setImmediate(resolve));
test("paths are explicit and local proxy accepts only a bounded numeric port", () => {
  assert.deepEqual(downloadOptions("follow_core", ""), { path: "follow_core" });
  assert.deepEqual(downloadOptions("direct", "bad"), { path: "direct" });
  assert.deepEqual(downloadOptions("local_proxy", "7890"), { path: "local_proxy", proxyPort: 7890 });
  for (const port of ["", "0", "-1", "65536", "1.2", "1e3", "https://secret"]) assert.throws(() => downloadOptions("local_proxy", port));
  assert.throws(() => downloadOptions("unknown", "7890"));
});
test("observer polls serially, ignores wrong tasks and old sequence numbers", async () => {
  const callbacks=[], values=[], queue=[
    {id:"other",sequence:9}, {id:"mine",sequence:3}, {id:"mine",sequence:2}, {id:"mine",sequence:4},
  ];
  const stop=observeSubscriptionTask("mine", async () => queue.shift(), value=>values.push(value.sequence),
    cb => {callbacks.push(cb); return 1;}, ()=>{});
  await tick();
  for(let i=0;i<3;i++) { callbacks.shift()(); await tick(); }
  stop();
  assert.deepEqual(values,[3,4]);
});
test("late responses after stop never affect UI", async () => {
  let resolve, called=0;
  const stop=observeSubscriptionTask("mine", () => new Promise(r=>resolve=r), ()=>called++,()=>{throw new Error("late timer");},()=>{});
  stop(); resolve({id:"mine",sequence:1}); await tick();
  assert.equal(called,0);
});
test("status read failure remains separate from authoritative import result", async () => {
  const callbacks=[];
  const stop=observeSubscriptionTask("mine", async()=>{throw new Error("status unavailable");},()=>{throw new Error("unexpected");},
    cb=>{callbacks.push(cb);return 1;},()=>{});
  await tick(); assert.equal(callbacks.length,1); stop();
});
test("task description explains actual route and retry count, not core readiness", () => {
  const text=taskDescription({message:"正在下载订阅",path:"serylane",elapsedMs:1800,attempt:2});
  assert.match(text,/经 Serylane 当前核心/); assert.match(text,/第 2 次请求/); assert.match(text,/1 秒/);
});
test("cancel control remains outside the disabled import fieldset", () => {
  const markup=readFileSync(new URL("../src/subscription-import.ts",import.meta.url),"utf8");
  assert.ok(markup.indexOf('id="managed-subscription-abort"') < markup.indexOf("<fieldset"));
  assert.match(markup,/不自动改走直连/);
});
