import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
const read=f=>readFileSync(new URL(`../src/${f}`,import.meta.url),'utf8');
const compile=s=>ts.transpileModule(s,{compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText;
const load=async f=>import('data:text/javascript;base64,'+Buffer.from(compile(read(f))).toString('base64'));
const {PageReads}=await load('read-coordinator.ts');
const {InteractionSession}=await load('interaction-session.ts');
const ast=ts.createSourceFile('main.ts',read('main.ts'),ts.ScriptTarget.Latest,true);
const names=['openNodeDetails','closeNodeDetails','pageRead'];
const source=ast.statements.filter(s=>ts.isFunctionDeclaration(s)&&names.includes(s.name?.text)).map(s=>s.getText(ast)).join('\n');
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve};};
class Element {
 constructor(text=''){this.textContent=text;this.disabled=false;this.attrs={};this.classes=new Set(['is-hidden']);this.classList={add:x=>this.classes.add(x),remove:x=>this.classes.delete(x)};this.isConnected=true;}
 setAttribute(k,v){this.attrs[k]=v;}removeAttribute(k){delete this.attrs[k];}focus(){}
}
function fixture(){
 const data=deferred(),modal=new Element(),refresh=new Element('刷新');let context='a',calls=0;
 const ctx={HTMLElement:Element,document:{activeElement:refresh,querySelector:()=>refresh},store:{runtime:{phase:'running'},nodeDetails:null},
 nodeDetailsSession:new InteractionSession(),nodeDetailsContext:g=>'runtime/'+g,nodeDetailsGroup:null,nodeDetailsReturnFocus:null,
 api:{currentNodeDetails:()=>{calls++;return data.promise;}},renderNodeDetails:()=>{},syncModalScrollLock:()=>{},toast:()=>{},errorMessage:String,
 stableMarkup:(element,html)=>{element.innerHTML=html;},escapeHtml:s=>s,$:()=>modal,pageReads:new PageReads(),pageContext:()=>context,buttonReads:new WeakMap(),Promise};
 vm.createContext(ctx);vm.runInContext(compile(source),ctx);
 return {ctx,data,modal,refresh,calls:()=>calls,setContext:c=>{context=c;}};
}
test('current-node loading is synchronous, duplicate clicks coalesce, closed details stay closed',async()=>{
 const f=fixture();const reading=f.ctx.openNodeDetails('group');
 assert.equal(f.modal.classes.has('is-hidden'),false);assert.equal(f.modal.attrs['aria-busy'],'true');assert.match(f.modal.innerHTML,/正在读取/);
 await f.ctx.openNodeDetails('group');assert.equal(f.calls(),1);
 f.ctx.closeNodeDetails();f.data.resolve({nodeName:'node'});await reading;
 assert.equal(f.modal.classes.has('is-hidden'),true);assert.equal(f.ctx.store.nodeDetails,null);
});
test('queued page read acknowledges immediately and earlier completion cannot clear newer feedback',async()=>{
 const f=fixture(),old=deferred(),next=deferred();let started=false;
 const first=f.ctx.pageRead('rules','#refresh',async()=>old.promise);await Promise.resolve();await Promise.resolve();
 f.setContext('b');f.refresh.textContent='刷新';f.refresh.disabled=false;f.refresh.removeAttribute('aria-busy');
 const second=f.ctx.pageRead('rules','#refresh',async()=>{started=true;await next.promise;});
 assert.equal(started,false);assert.equal(f.refresh.textContent,'正在刷新…');assert.equal(f.refresh.attrs['aria-busy'],'true');
 old.resolve();await first;await new Promise(r=>setImmediate(r));
 assert.equal(started,true);assert.equal(f.refresh.disabled,true);
 next.resolve();await second;assert.equal(f.refresh.disabled,false);assert.equal(f.refresh.textContent,'刷新');
});
