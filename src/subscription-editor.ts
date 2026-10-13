import { InteractionSession } from './interaction-session';
export type SubscriptionEdit = { profileId: string; expectedRevision: string; displayName: string; url: string; userAgent: string };
export function mountSubscriptionEditor(list: HTMLElement, services: {
  read(id: string): Promise<SubscriptionEdit>; save(input: SubscriptionEdit): Promise<SubscriptionEdit>;
  changed(): Promise<unknown>; error(error: unknown): string;
}) {
  const dialog = document.createElement('dialog'); dialog.className = 'subscription-edit-dialog';
  dialog.setAttribute('aria-labelledby','subscription-edit-title');
  dialog.innerHTML = `<form><header class="panel-heading"><h2 id="subscription-edit-title">编辑订阅</h2><button type="button" class="button button-quiet" data-edit-close>关闭</button></header>
    <p class="hint">保存名称、订阅地址与客户端标识；保留配置版本、已选用状态与历史记录。新地址不会自动下载，请保存后点击刷新。地址可能含私人凭据，请勿分享。</p>
    <fieldset><label>订阅名称<input name="displayName" required maxlength="100" /></label><label>订阅地址<input name="url" type="url" required maxlength="8192" autocomplete="off" spellcheck="false" /></label><label>客户端标识（User-Agent）<input name="userAgent" maxlength="512" placeholder="clash.meta" autocomplete="off" /></label><button type="submit" class="button button-primary">保存信息</button></fieldset><p role="status" data-edit-feedback></p></form>`;
  document.body.append(dialog);
  const menu = document.createElement('div'); menu.className = 'subscription-context-menu'; menu.hidden = true; menu.setAttribute('role','menu');
  menu.innerHTML = '<button role="menuitem" type="button" class="button button-quiet">编辑订阅信息</button>'; document.body.append(menu);
  const form=dialog.querySelector('form')!, fields=dialog.querySelector('fieldset')!, feedback=dialog.querySelector<HTMLElement>('[data-edit-feedback]')!;
  const input=(name:string)=>form.elements.namedItem(name) as HTMLInputElement;
  const session=new InteractionSession(); let snapshot:SubscriptionEdit|null=null, returnFocus:HTMLElement|null=null, menuId:string|null=null, returnProfileId:string|null=null, busy=false;
  function dismissMenu() { menu.hidden=true; menuId=null; }
  function close() { session.close(); snapshot=null; busy=false; form.reset(); feedback.textContent=''; dialog.close(); const target=returnFocus?.isConnected ? returnFocus : Array.from(list.querySelectorAll<HTMLElement>("[data-subscription-id]")).find(c=>c.dataset.subscriptionId===returnProfileId); target?.focus({preventScroll:true}); }
  async function open(id:string, trigger?:HTMLElement) {
    const ticket=session.begin(id); if(ticket===null)return;
    dismissMenu(); returnProfileId=id; returnFocus=trigger??document.activeElement as HTMLElement; snapshot=null; form.reset(); fields.disabled=true; feedback.textContent='正在读取订阅信息…';
    if(!dialog.open)dialog.showModal();
    try {
      const result=await services.read(id); if(!session.accepts(ticket,id))return;
      snapshot=result; for(const name of ['displayName','url','userAgent'] as const)input(name).value=result[name];
      fields.disabled=false; feedback.textContent=''; input('displayName').focus();
    } catch(error){if(session.accepts(ticket,id))feedback.textContent=services.error(error);}
    finally{session.settle(ticket);}
  }
  function showMenu(card:HTMLElement,x:number,y:number) {
    if(!card.dataset.subscriptionId)return;
    menuId=card.dataset.subscriptionId;returnFocus=card;menu.hidden=false;
    menu.style.left=`${Math.max(8,Math.min(x,innerWidth-menu.offsetWidth-8))}px`;
    menu.style.top=`${Math.max(8,Math.min(y,innerHeight-menu.offsetHeight-8))}px`;
    menu.querySelector('button')!.focus();
  }
  list.addEventListener('contextmenu',event=>{const card=(event.target as HTMLElement).closest<HTMLElement>('[data-subscription-id]');if(!card)return;event.preventDefault();showMenu(card,event.clientX,event.clientY);});
  list.addEventListener('keydown',event=>{if(event.key!=='ContextMenu'&&!(event.shiftKey&&event.key==='F10'))return;const card=(event.target as HTMLElement).closest<HTMLElement>('[data-subscription-id]');if(!card)return;event.preventDefault();const r=card.getBoundingClientRect();showMenu(card,r.left+20,r.top+20);});
  list.addEventListener('click',event=>{const target=(event.target as HTMLElement).closest<HTMLElement>('[data-subscription-action="edit"]');if(target?.dataset.profileId)void open(target.dataset.profileId,target);});
  menu.addEventListener('click',()=>{if(menuId)void open(menuId,returnFocus??undefined);});
  document.addEventListener('pointerdown',event=>{if(!menu.contains(event.target as Node))dismissMenu();});
  document.addEventListener('keydown',event=>{if(event.key==='Escape'&&!menu.hidden){event.preventDefault();dismissMenu();returnFocus?.focus();}});
  document.addEventListener('scroll',dismissMenu,true);
  dialog.querySelector('[data-edit-close]')!.addEventListener('click',close);
  dialog.addEventListener('cancel',event=>{event.preventDefault();close();});
  dialog.addEventListener('click',event=>{if(event.target===dialog) {const r=dialog.getBoundingClientRect();if(event.clientX<r.left||event.clientX>r.right||event.clientY<r.top||event.clientY>r.bottom)close();}});
  form.addEventListener('submit',async event=>{
    event.preventDefault();if(!snapshot||busy)return;
    const draft={...snapshot,displayName:input('displayName').value.trim(),url:input('url').value.trim(),userAgent:input('userAgent').value.trim()};
    const ticket=session.refresh(draft.profileId);if(ticket===null)return;busy=true;fields.disabled=true;feedback.textContent='正在保存信息…';
    try{const result=await services.save(draft);await services.changed();if(session.accepts(ticket,draft.profileId)){snapshot=result;feedback.textContent='已保存；选用状态与版本未改变。新地址请手动刷新。';}}
    catch(error){if(session.accepts(ticket,draft.profileId))feedback.textContent=services.error(error);}
    finally{if(session.accepts(ticket,draft.profileId)){busy=false;fields.disabled=false;form.querySelector<HTMLButtonElement>("button[type=submit]")?.focus();}session.settle(ticket);}
  });
  return { close:()=>{dismissMenu();if(dialog.open)close();} };
}
