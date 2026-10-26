import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const source=readFileSync(new URL('../src/stable-dom.ts',import.meta.url),'utf8');
const js=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText;
const {stableMarkup}=await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
// Minimal structural DOM double; pointer/focus/browser behavior is separately
// tested in the real isolated frontend with Playwright, not inferred here.
class Element {
 constructor(name,attrs={},children=[]){this.nodeType=1;this.nodeName=name;this.attrs={...attrs};this.childNodes=[];for(const c of children)this.insertBefore(c,null);}
 get attributes(){return Object.entries(this.attrs).map(([name,value])=>({name,value}));}
 get id(){return this.attrs.id??'';}
 getAttribute(k){return this.attrs[k]??null;}hasAttribute(k){return k in this.attrs;}
 setAttribute(k,v){this.attrs[k]=v;}removeAttribute(k){delete this.attrs[k];}
 get firstChild(){return this.childNodes[0]??null;}get nextSibling(){return this.parentNode?.childNodes[this.parentNode.childNodes.indexOf(this)+1]??null;}
 insertBefore(n,c){if(n.parentNode)n.parentNode.removeChild(n);const i=c?this.childNodes.indexOf(c):this.childNodes.length;this.childNodes.splice(i,0,n);n.parentNode=this;}
 removeChild(n){this.childNodes.splice(this.childNodes.indexOf(n),1);n.parentNode=null;}
 cloneNode(deep){return new Element(this.nodeName,this.attrs,deep?this.childNodes.map(c=>c.cloneNode(true)):[]);}
}
test('keyed refresh preserves existing controls, handles reorder and removes ended rows',()=>{
 globalThis.Element=Element;globalThis.Node={TEXT_NODE:3};let next;
 globalThis.document={createElement:()=>({set innerHTML(_){},get content(){return next;}})};
 try{
  const row=(id,label)=>new Element('TR',{'data-render-key':id},[new Element('BUTTON',{'aria-label':label})]);
  const a=row('a','old'),b=row('b','keep'),button=a.firstChild;
  const root=new Element('TBODY',{},[a,b]);
  next=new Element('FRAGMENT',{},[row('a','new'),row('b','keep')]);stableMarkup(root,'synthetic');
  assert.equal(root.firstChild,a);assert.equal(a.firstChild,button);assert.equal(button.getAttribute('aria-label'),'new');
  next=new Element('FRAGMENT',{},[row('b','keep'),row('a','new')]);stableMarkup(root,'synthetic');
  assert.deepEqual(root.childNodes,[b,a]);assert.equal(a.firstChild,button);
  next=new Element('FRAGMENT',{},[row('b','keep')]);stableMarkup(root,'synthetic');
  assert.equal(a.parentNode,null);assert.equal(root.firstChild,b);
 }finally{delete globalThis.Element;delete globalThis.Node;delete globalThis.document;}
});
