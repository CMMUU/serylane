import type { CurrentNodeDetails, ProfileRecord, RouteSnapshot, RoutingMode } from "./types";

export const OPENAI_GROUP = "🤖 OpenAI 自动灾备";
export type NodeMetadata = { regions: string[]; regionStatus: "supported" | "unsupported" | "restricted" | "unknown" | "ambiguous"; regionReason: string; eligible: boolean; ruleVersion: string; ruleSource: string; checkedAt: string; nameMultiplier: number | null; multiplierSource: "name" | "manual" | "unknown" | "conflict" | "invalid" };
export const multiplierSourceLabel = (source: NodeMetadata["multiplierSource"] | undefined): string => ({ name: "名称识别", manual: "手动覆盖", unknown: "倍率未知", conflict: "倍率冲突", invalid: "倍率无效" })[source ?? "unknown"];
export type ProxyNode = { history?: { delay: number; time?: string }[]; type?: string; now?: string; all?: string[]; fixed?: string | boolean; manualNode?: string | null; alive?: boolean; udp?: boolean; trafficMultiplier?: number | null; withinCostBudget?: boolean; metadata?: NodeMetadata };
export type ProxyMap = Record<string, ProxyNode>;
export type OpenAiRecommendation = { name: string; rank: number | null; score: number | null; streamSamples: number; connectionSamples: number; cooling: boolean; unavailable: boolean };
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

export function openAiRecommendations(
  profile: Pick<ProfileRecord, "id" | "activeRevisionId" | "openaiPolicy">,
  snapshot?: RouteSnapshot["stability"],
  available?: string[],
): OpenAiRecommendation[] {
  const policy = profile.openaiPolicy;
  if (!policy.enabled || !(policy.benchmarkVersion >= 2)) return [];
  const hasLiveRanking = snapshot?.running && snapshot.eligible && snapshot.lastCheck &&
    snapshot.profileId === profile.id && snapshot.revisionId === profile.activeRevisionId;
  const names = policy.selectedNodes.filter(node => available?.includes(node.name) ?? true);
  const rows = names.map((node, index): OpenAiRecommendation => {
    const live = hasLiveRanking ? snapshot.nodes.find(row => row.name === node.name) : undefined;
    return {
      name: node.name,
      rank: hasLiveRanking ? live?.rank ?? null : index + 1,
      score: hasLiveRanking ? live?.recommendationScore ?? null : node.score,
      streamSamples: live?.streamSamples ?? 0,
      connectionSamples: live?.connectionSamples ?? node.connectionSamples ?? 0,
      cooling: (live?.cooldownSeconds ?? 0) > 0,
      unavailable: Boolean(hasLiveRanking && !live?.probeOk),
    };
  });
  return rows.sort((a, b) => (a.rank ?? Number.MAX_SAFE_INTEGER) - (b.rank ?? Number.MAX_SAFE_INTEGER));
}

export function openAiRecommendationMarkup(rows: OpenAiRecommendation[], current?: string, prices?: ProxyMap): string {
  if (!rows.length) return "";
  const status = (row: OpenAiRecommendation) => [
    row.name === current ? "当前使用" : "",
    row.cooling ? "冷却中 · 暂停自动选点" : row.unavailable ? "暂不参与自动选点" : "",
    row.streamSamples >= 3 ? `模型流样本 ${row.streamSamples} 条` : row.streamSamples > 0 ? `真实模型流待验证（${row.streamSamples} 条样本）` : "真实模型流待验证",
    row.connectionSamples > 0 ? `连通性样本 ${row.connectionSamples} 次` : "",
  ].filter(Boolean).join(" · ");
  return `<section class="openai-recommendations" aria-labelledby="openai-recommendation-title">
    <h4 id="openai-recommendation-title">推荐顺序</h4>
    <ul role="list">${rows.map(row => {
      const multiplier = prices?.[row.name]?.trafficMultiplier;
      return `<li>
        <span class="openai-recommendation-rank">${row.rank === null ? "—" : `${row.rank}.`}</span>
        <span class="openai-recommendation-node"><strong>${escapeNode(row.name)}</strong><small>${status(row)}</small></span>
        <span class="openai-recommendation-score">${prices ? `${multiplier == null ? "倍率未知" : `${multiplier}×`} · ` : ""}${row.score === null ? "评分待更新" : `${Math.round(row.score * 10) / 10} 分`}</span>
      </li>`;
    }).join("")}</ul>
  </section>`;
}

export function aiNodeBlockReason(node?: ProxyNode): string | null {
  if (node?.metadata?.eligible !== true) return node?.metadata?.regionReason ?? "节点资格尚未核实";
  if (node.withinCostBudget === false) return "超出当前流量倍率预算";
  return null;
}

type NodePickerState = {
  draft?: string;
  pending?: { node: string | null; phase: "write" | "verify" };
};

export function nodeSelectionMarkup(
  group: string,
  node: ProxyNode,
  busy: boolean,
  candidates = node.all ?? [],
  prices?: ProxyMap,
  recommendations?: OpenAiRecommendation[],
  { draft, pending }: NodePickerState = {},
): string {
  if (!canSelectNode(node)) return '<p class="node-choice-help">此组由配置决定出口，不支持单节点手动选择。</p>';
  const observed = node.manualNode || (typeof node.fixed === "string" ? node.fixed : null) || node.now;
  const liveNames = new Set(node.all);
  const blockReason = (name: string) => !liveNames.has(name) ? "节点已不在当前策略组" : group === OPENAI_GROUP ? aiNodeBlockReason(prices?.[name]) : null;
  const available = (name: string) => name !== "" && blockReason(name) === null;
  const hasAvailable = candidates.some(available);
  const intended = pending?.node ?? draft ?? observed;
  const selected = intended && candidates.includes(intended) ? intended : "";
  const selectedAvailable = selected !== "" && available(selected);
  const applying = Boolean(pending && pending.node !== null);
  const restoring = pending?.node === null;
  const locked = busy || Boolean(pending);
  const phaseLabel = pending?.phase === "verify" ? "正在核对出口…" : null;
  const mode = node.manualNode ? "手动固定 · 自动切换已暂停" : node.type === "Selector" ? (group === OPENAI_GROUP ? "自动 · 稳定优先" : "手动选择") : node.fixed ? "手动优先 · 失效时由核心回退" : "自动选择";
  const selectedReason = selected ? blockReason(selected) : null;
  const choiceHelp = !hasAvailable ? `暂无可手动选用的节点。${selectedReason ?? ""}`
    : !selectedAvailable ? `请先选择一个可用节点。${selectedReason ?? ""}` : "";
  return `<form class="node-choice" data-render-key="node-choice:${escapeNode(group)}" data-node-group="${escapeNode(group)}">
    <label><span>${escapeNode(mode)}</span><select data-render-key="node-choice-select" aria-label="${escapeNode(group)} · 选择节点" ${locked || !hasAvailable ? "disabled" : ""}>
    ${!selected ? '<option data-render-key="node-choice-placeholder" value="" selected disabled>请选择节点</option>' : ""}
    ${candidates.map(name => {
      const recommendation = recommendations?.find(row => row.name === name);
      const label = recommendation?.rank != null ? `${recommendation.rank}. ${name}` : name;
      const eligible = available(name);
      return `<option data-render-key="node-choice-option:${escapeNode(name)}" value="${escapeNode(name)}" ${name === selected ? "selected" : ""} ${eligible ? "" : "disabled"}>${escapeNode(label)}${prices ? ` · ${prices[name]?.trafficMultiplier == null ? "倍率未知" : `${prices[name]?.trafficMultiplier}×`}` : ""}${recommendation?.cooling ? " · 冷却中" : ""}${eligible ? "" : ` · ${escapeNode(blockReason(name))}`}</option>`;
    }).join("")}</select></label>
    <button type="submit" class="button button-primary" data-render-key="node-choice-apply" data-node-action-loading aria-busy="${applying}" ${locked || !selectedAvailable ? "disabled" : ""}>${applying ? phaseLabel ?? "正在应用节点…" : "应用节点"}</button>
    ${restoring || canRestoreAuto(group, node) ? `<button type="button" class="button button-quiet" data-render-key="node-choice-auto" data-node-auto="${escapeNode(group)}" data-node-action-loading aria-busy="${restoring}" ${locked ? "disabled" : ""}>${restoring ? phaseLabel ?? "正在恢复自动…" : "恢复自动"}</button>` : ""}
  </form><p class="node-choice-help" data-render-key="node-choice-help:${escapeNode(group)}" role="status" ${choiceHelp ? "" : "hidden"}>${escapeNode(choiceHelp)}</p>${node.manualNode && !candidates.includes(node.manualNode) ? '<p class="node-choice-help">原手动节点已不在候选中，请重新选择或恢复自动；不会静默恢复自动。</p>' : ""}`;
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
