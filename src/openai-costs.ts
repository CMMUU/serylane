import { escapeNode } from "./node-selection";
export type CostMode = "quality" | "value";
export type CostSnapshot = { profileId: string; profileRevision: string; revision: number; mode: CostMode; maxMultiplier: number | null; allowUnknown: boolean; nodes: { name: string; multiplier: number | null }[] };
export function parseMultiplier(value: string): number | null {
  if (!value.trim()) return null;
  const number = Number(value);
  if (!Number.isFinite(number) || number < 0.01 || number > 1000) throw new Error("倍率需在 0.01～1000 之间；留空表示未知。");
  return number;
}
export const openAiCostsMarkup = `<details id="openai-cost-settings" class="openai-cost-settings"><summary>成本与性价比 <span id="openai-cost-caption">配置节点流量倍率</span></summary><div id="openai-cost-body"></div></details>`;
export function costFormMarkup(s: CostSnapshot): string {
  return `<form id="openai-cost-form"><p class="node-choice-help">按服务商规则手动填写倍率，留空为未知，不从名称猜测。1× 表示使用 1 GB 通常扣除 1 GB 套餐流量；实际账单以服务商为准。此处只影响 OpenAI 自动筛选，不修改普通代理组或手动选点。</p>
    <div class="cost-preferences"><label>自动选点偏好<select name="mode"><option value="quality" ${s.mode === "quality" ? "selected" : ""}>质量优先（原策略）</option><option value="value" ${s.mode === "value" ? "selected" : ""}>性价比优先（推荐）</option></select></label>
    <label>倍率上限（仅性价比模式）<input name="maxMultiplier" type="number" min="0.01" max="1000" step="any" value="${s.maxMultiplier ?? ""}" placeholder="不限制" /></label></div>
    <label class="cost-unknown"><input name="allowUnknown" type="checkbox" ${s.allowUnknown ? "checked" : ""} /><span>允许未知倍率兜底（不能保证费用上限）</span></label>
    <p class="node-choice-help">质量优先沿用原策略，不限制倍率。性价比模式先保证基本质量，再比较倍率；跳过大流量带宽测速，但连通性检测仍消耗少量流量。健康且符合预算的当前节点保持使用；无符合预算的健康节点时拒绝新请求，不擅自使用超预算节点。手动固定节点仍优先。</p>
    <label class="cost-search">查找节点<input name="search" type="search" placeholder="按节点名称筛选" /></label>
    <div class="cost-node-list">${s.nodes.map((n,i) => `<label class="cost-node-row" data-cost-index="${i}"><span>${escapeNode(n.name)}</span><span class="cost-rate-input"><input name="node-${i}" aria-label="${escapeNode(n.name)} · 流量倍率" type="number" min="0.01" max="1000" step="any" placeholder="未知" value="${n.multiplier ?? ""}" /><span>×</span></span></label>`).join("") || '<p class="node-choice-help">没有可定价的显式节点；Provider 动态节点暂不支持 OpenAI 筛选。</p>'}</div>
    <p class="node-choice-help">连接信息变更后倍率作废，需重新确认。保存不启动核心或生成灾备；下一轮稳定检查使用新预算，重新生成时再筛选候选池。已有基础 Fallback 时，请先启用稳定优先。</p>
    <div class="toolbar"><button class="button button-primary" type="submit">保存成本策略</button><button class="button button-quiet" type="button" data-cost-refresh>重新读取</button><span id="cost-feedback" role="status"></span></div></form>`;
}
export function mountOpenAiCosts(services: {
  profile: () => { id: string; revision: string | null } | null;
  read: (id: string) => Promise<CostSnapshot>; save: (input: CostSnapshot) => Promise<CostSnapshot>;
  confirm: (message: string) => Promise<boolean>; changed: () => void;
}) {
  const root = document.querySelector<HTMLDetailsElement>("#openai-cost-settings")!;
  const body = root.querySelector<HTMLElement>("#openai-cost-body")!;
  let snapshot: CostSnapshot | null = null, busy = false, dirty = false, sequence = 0, editRevision = 0;
  const feedback = (message: string) => { const node = root.querySelector("#cost-feedback"); if (node) node.textContent = message; };
  const errorText = (error: unknown) => error instanceof Error ? error.message : typeof error === "object" && error !== null && "message" in error ? String(error.message) : String(error);
  const render = () => {
    root.querySelector("#openai-cost-caption")!.textContent = snapshot ? snapshot.mode === "value" ? "性价比优先" : "质量优先" : "配置节点流量倍率";
    body.innerHTML = snapshot ? costFormMarkup(snapshot) : '<p class="node-choice-help">请先选用配置。</p>';
    dirty = false;
  };
  async function refresh(force = false) {
    if (busy) return;
    const profile = services.profile();
    const same = snapshot?.profileId === profile?.id && snapshot?.profileRevision === profile?.revision;
    if (!root.open && same) return;
    if (dirty && snapshot?.profileId === profile?.id && !force) { feedback(same ? "有未保存的编辑，未覆盖草稿。" : "配置版本已变化，草稿未提交；请重新读取后编辑。"); return; }
    if (force && dirty && !await services.confirm("重新读取将放弃未保存的倍率编辑，是否继续？")) return;
    const request = ++sequence;
    if (!same) { snapshot = null; render(); }
    if (!profile?.revision || !root.open) return;
    busy = true;
    const requestedEdit = editRevision;
    try {
      const result = await services.read(profile.id);
      if (request !== sequence || services.profile()?.id !== profile.id || services.profile()?.revision !== profile.revision) return;
      if (requestedEdit !== editRevision) { feedback("读取期间发生编辑，未覆盖草稿。"); return; }
      if (result.profileId !== profile.id || result.profileRevision !== profile.revision) throw new Error("配置已变化，请重新读取成本清单。");
      snapshot = result; render();
    } catch (error) {
      if (snapshot) feedback(errorText(error));
      else body.innerHTML = `<p class="node-choice-help">${escapeNode(errorText(error))}</p><button class="button button-quiet" data-cost-refresh>重试</button>`;
    } finally { busy = false; }
  }
  root.addEventListener("toggle", () => { if (root.open) void refresh(); });
  root.addEventListener("input", event => {
    const input = event.target as HTMLInputElement;
    if (input.name === "search") root.querySelectorAll<HTMLElement>("[data-cost-index]").forEach(row => { row.hidden = !snapshot?.nodes[Number(row.dataset.costIndex)].name.toLowerCase().includes(input.value.toLowerCase()); });
    else { dirty = true; editRevision++; feedback("有未保存的更改"); }
  });
  root.addEventListener("click", event => { if ((event.target as HTMLElement).closest("[data-cost-refresh]")) void refresh(true); });
  root.addEventListener("submit", async event => {
    event.preventDefault();
    if (busy || !snapshot) return;
    const form = root.querySelector<HTMLFormElement>("form")!;
    const input = (name: string) => form.elements.namedItem(name) as HTMLInputElement;
    let draft: CostSnapshot;
    try {
      draft = { ...snapshot, mode: input("mode").value as CostMode, maxMultiplier: parseMultiplier(input("maxMultiplier").value), allowUnknown: input("allowUnknown").checked,
        nodes: snapshot.nodes.map((n,i) => ({ name: n.name, multiplier: parseMultiplier(input(`node-${i}`).value) })) };
    } catch (error) { feedback(errorText(error)); return; }
    busy = true;
    try {
      if (!await services.confirm("性价比模式会限制下一轮自动选点的倍率；无符合预算的健康节点时可能拒绝后续新请求。不主动断开已有连接，不覆盖手动固定节点。是否保存？")) return;
      if (services.profile()?.id !== draft.profileId || services.profile()?.revision !== draft.profileRevision) throw new Error("活动配置已变化，请重新读取。");
      form.querySelectorAll<HTMLInputElement>("input, select, button").forEach(control => control.disabled = true);
      const saved = await services.save(draft);
      if (services.profile()?.id !== draft.profileId || services.profile()?.revision !== draft.profileRevision) { snapshot = null; render(); return; }
      snapshot = saved; render(); feedback("成本策略已保存；未启动核心或生成灾备。"); services.changed();
    } catch (error) { feedback(errorText(error)); }
    finally { busy = false; form.querySelectorAll<HTMLInputElement>("input, select, button").forEach(control => control.disabled = false); }
  });
  render();
  return { refresh };
}
