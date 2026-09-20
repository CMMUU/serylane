import type { CurrentNodeDetails, RoutingMode } from "./types";

export const OPENAI_GROUP = "🤖 OpenAI 自动灾备";
export type NodeMetadata = { regions: string[]; regionStatus: "supported" | "unsupported" | "restricted" | "unknown" | "ambiguous"; regionReason: string; eligible: boolean; ruleVersion: string; ruleSource: string; checkedAt: string; nameMultiplier: number | null; multiplierSource: "name" | "manual" | "unknown" | "conflict" | "invalid" };
export const multiplierSourceLabel = (source: NodeMetadata["multiplierSource"] | undefined): string => ({ name: "名称识别", manual: "手动覆盖", unknown: "倍率未知", conflict: "倍率冲突", invalid: "倍率无效" })[source ?? "unknown"];
export type ProxyNode = { type?: string; now?: string; all?: string[]; fixed?: string | boolean; manualNode?: string | null; alive?: boolean; udp?: boolean; trafficMultiplier?: number | null; withinCostBudget?: boolean; metadata?: NodeMetadata };
export type ProxyMap = Record<string, ProxyNode>;
export const escapeNode = (value: unknown): string => String(value ?? "—").replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
export const canSelectNode = (node: ProxyNode): boolean => ["Selector", "Fallback", "URLTest"].includes(node.type ?? "") && Array.isArray(node.all);
export const canRestoreAuto = (group: string, node: ProxyNode): boolean => Boolean(node.manualNode && group === OPENAI_GROUP) || Boolean(node.fixed && ["Fallback", "URLTest"].includes(node.type ?? ""));
export function generalGroups(proxies: ProxyMap, mode: RoutingMode): string[] {
  if (mode === "direct") return [];
  if (mode === "global") return Array.isArray(proxies.GLOBAL?.all) ? ["GLOBAL"] : [];
  return Object.keys(proxies).filter(name => name !== OPENAI_GROUP && !["GLOBAL", "COMPATIBLE", "PASS"].includes(name) && Array.isArray(proxies[name].all));
}

// A group is not necessarily one outbound (e.g. load balancing). Never invent
// a leaf or call the first candidate selected when `now` is missing.
export function resolvedNode(proxies: ProxyMap, group: string): { name: string | null; chain: string[] } {
  const chain: string[] = [];
  let name = group;
  for (let depth = 0; depth < 16; depth++) {
    if (chain.includes(name) || !Object.prototype.hasOwnProperty.call(proxies, name)) return { name: null, chain };
    chain.push(name);
    const node = proxies[name];
    if (!Array.isArray(node.all)) return { name, chain };
    if (!node.now || ["LoadBalance", "Relay"].includes(node.type ?? "")) return { name: null, chain };
    name = node.now;
  }
  return { name: null, chain };
}

export function nodeSelectionMarkup(group: string, node: ProxyNode, busy: boolean, candidates = node.all ?? [], prices?: ProxyMap): string {
  if (!canSelectNode(node)) return '<p class="node-choice-help">此组由配置决定出口，不支持单节点手动选择。</p>';
  const selected = node.manualNode || (typeof node.fixed === "string" ? node.fixed : null) || node.now;
  const mode = node.manualNode ? "手动固定 · 自动切换已暂停" : node.type === "Selector" ? (group === OPENAI_GROUP ? "自动 · 稳定优先" : "手动选择") : node.fixed ? "手动优先 · 失效时由核心回退" : "自动选择";
  return `<form class="node-choice" data-node-group="${escapeNode(group)}">
    <label><span>${escapeNode(mode)}</span><select aria-label="${escapeNode(group)} · 选择节点" ${busy || !candidates.length ? "disabled" : ""}>
    ${!candidates.includes(selected ?? "") ? '<option value="" selected disabled>请选择节点</option>' : ""}
    ${candidates.map(name => `<option value="${escapeNode(name)}" ${name === selected ? "selected" : ""} ${group === OPENAI_GROUP && prices?.[name]?.metadata?.eligible !== true ? "disabled" : ""}>${escapeNode(name)}${prices ? ` · ${prices[name]?.trafficMultiplier == null ? "倍率未知" : `${prices[name].trafficMultiplier}×`}` : ""}${group === OPENAI_GROUP && prices?.[name]?.metadata?.eligible !== true ? ` · ${escapeNode(prices?.[name]?.metadata?.regionReason ?? "地区待确认")}` : ""}</option>`).join("")}</select></label>
    <button type="submit" class="button button-primary" ${busy || !candidates.length ? "disabled" : ""}>应用节点</button>
    ${canRestoreAuto(group, node) ? `<button type="button" class="button button-quiet" data-node-auto="${escapeNode(group)}" ${busy ? "disabled" : ""}>恢复自动</button>` : ""}
  </form>${node.manualNode && !candidates.includes(node.manualNode) ? '<p class="node-choice-help">原手动节点已不在候选中，请重新选择或恢复自动；不会静默恢复自动。</p>' : ""}`;
}

export function currentNodeMarkup(label: string, group: string, proxies: ProxyMap, details: CurrentNodeDetails | undefined): string {
  const resolved = resolvedNode(proxies, group);
  const node = resolved.name ? proxies[resolved.name] : undefined;
  const verified = details && details.nodeName === resolved.name && details.routeChain.join("\0") === resolved.chain.join("\0") ? details : undefined;
  const alive = verified?.alive ?? node?.alive;
  const status = resolved.name === "DIRECT" ? "直连出口" : resolved.name?.startsWith("REJECT") ? "阻断出口" : alive === true ? "最近探测可达（非模型验证）" : alive === false ? "最近探测失败" : "尚无有效检测";
  const rows = [
    ["策略组", group], ["协议", verified?.nodeType ?? node?.type ?? "—"],
    ["最近有效延迟", verified?.lastDelayMs == null ? "暂无" : `${verified.lastDelayMs} ms`],
    ["服务器（脱敏）", verified?.maskedServer ?? "未提供"],
    ["端口 / 传输", verified ? `${verified.port ?? "—"} / ${verified.network ?? "未声明"}` : "—"],
    ["来源", verified?.providerName ?? (verified ? "本地配置" : "待查询")],
    ["地区资格", node?.metadata?.regionReason ?? "地区待确认"],
    [`流量倍率（${multiplierSourceLabel(node?.metadata?.multiplierSource)}）`, node?.trafficMultiplier == null ? "未知" : `${node.trafficMultiplier}×`],
  ];
  return `<section class="overview-node"><div class="overview-node-heading"><h3>${escapeNode(label)}</h3><button class="button button-quiet" data-overview-node-group="${escapeNode(group)}">${canSelectNode(proxies[group] ?? {}) ? "切换节点" : "查看策略组"}</button></div>
    <strong class="overview-node-name">${escapeNode(resolved.name ?? "无法确定单一出口")}</strong><p class="node-choice-help">${status}</p>
    <p class="overview-node-chain">${resolved.chain.map(escapeNode).join(" → ")}</p>
    <dl class="overview-node-details">${rows.map(([key, value]) => `<div><dt>${key}</dt><dd>${escapeNode(value)}</dd></div>`).join("")}</dl>
    <button class="button button-quiet" data-overview-node-details="${escapeNode(group)}" ${resolved.name ? "" : 'disabled title="当前无法确定单一节点，请查看策略组"'}>查看完整详情</button></section>`;
}
