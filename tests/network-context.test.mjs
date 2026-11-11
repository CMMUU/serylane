import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const code=ts.transpileModule(readFileSync(new URL('../src/network-context.ts',import.meta.url),'utf8'),{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText;
const {createNetworkContext}=await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
const snapshot=(generation=1)=>({revision:`net-${generation}`,generation,observed:true,stale:false,policy:'system'});
const deferred=()=>{let resolve;return {promise:new Promise(r=>resolve=r),resolve:v=>resolve(v)};};
test('same network refresh preserves results, new epoch invalidates once and older replies are ignored',()=>{
 const events=[];const c=createNetworkContext({read:async()=>snapshot(),changed:(...v)=>events.push(v)});
 c.accept(snapshot());assert.equal(events.at(-1)[1],false);
 c.accept(snapshot());assert.equal(events.at(-1)[1],false);
 c.accept(snapshot(2));assert.equal(events.at(-1)[1],true);
 const count=events.length;c.accept(snapshot(1));assert.equal(events.length,count);assert.equal(c.current().revision,'net-2');
});
test('singleflight retains real worker and stale/wake context suppresses measurements until fresh state',async()=>{
 const gate=deferred();let clock=10,calls=0;const events=[];
 const c=createNetworkContext({now:()=>clock,read:()=>{calls++;return gate.promise;},changed:(...a)=>events.push(a)});
 c.accept(snapshot());clock=30000;assert.equal(c.current(),null);
 const a=c.refresh(),b=c.refresh();assert.equal(a,b);assert.equal(calls,1);assert.equal(events.at(-1)[0],null);
 gate.resolve(snapshot(2));await a;assert.equal(c.current().revision,'net-2');
 clock=0;assert.equal(c.current(),null);
});
test('unknown read failure is not converted to disabled Tailscale or a new synthetic success',async()=>{
 const events=[];const c=createNetworkContext({read:async()=>{throw Error('denied');},changed:(...a)=>events.push(a)});
 await c.refresh();assert.equal(c.current(),null);assert.deepEqual(events,[[null,true]]);
});
