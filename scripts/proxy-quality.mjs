import { readFile, stat } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HOUR = 3_600_000;
const CONFIDENCE_TAIL = 0.05;
const MAX_BYTES = 16 * 1024 * 1024;
const ARMS = ['candidate', 'control'];
const TRANSPORTS = ['sse', 'wss'];
const RESTORE_CASES = ['stop', 'start-failure', 'crash-recovery', 'third-party-owner'];
const ROUTE_CASES = ['domain', 'process', 'direct', 'proxy', 'block', 'ipv6', 'dns', 'conflict', 'fault'];
const MIN_RATE_TRIALS = 2995;

export const excellent = Object.freeze({
  'startup-ready': { p95Ms: 2000, minTrials: 20 },
  'state-restore': { successRate: 1, withinMs: 3000, cases: RESTORE_CASES },
  'route-fidelity': { successRate: 1, forbiddenDirect: 0, cases: ROUTE_CASES },
  'wss-first-handshake': { successRate: 0.999, withinMs: 10000, minIndependentTrials: MIN_RATE_TRIALS },
  'model-stream-completion': { successRate: 0.999, minIndependentTrials: MIN_RATE_TRIALS, transports: TRANSPORTS },
  'paired-first-text': { p95Ms: 100, minPairs: 20, transports: TRANSPORTS },
  'unexpected-disconnects': { per100Hours: 0.5, minConnectionHours: 600 },
  'failover-recovery': { p95Ms: 30000, minTrials: 20, observationMs: 180000, unrecovered: 0 },
  'unnecessary-switches': { count: 0, minHealthyHours: 72 },
  'weighted-task-traffic': { maxRatio: 1.1, minCompletedTasksPerArm: 20 },
});

const id = value => typeof value === 'string' && /^[a-z0-9][a-z0-9_.-]{0,95}$/i.test(value);
const ms = value => Number.isFinite(value) && value >= 0 && value <= Number.MAX_SAFE_INTEGER;
const bytes = value => Number.isSafeInteger(value) && value >= 0;
const yesNo = value => typeof value === 'boolean';
const anyOf = (...values) => value => values.includes(value);
const optional = predicate => value => value == null || predicate(value);
const multiplier = value => typeof value === 'number' && Number.isFinite(value) && value > 0 && value <= 1_000_000;
const arm = anyOf(...ARMS);
const common = { id, arm };
const schemas = {
  run: {
    id, schemaVersion: anyOf(1), source: anyOf('fixture', 'observed'),
    networkMode: anyOf('system-proxy', 'tun'), comparisonId: optional(id),
  },
  startup: { ...common, durationMs: ms, controllerReady: yesNo, expectedEgress: yesNo },
  restore: {
    ...common, scenario: anyOf(...RESTORE_CASES), durationMs: ms, stateMatches: yesNo, ownerPreserved: yesNo,
  },
  route: {
    ...common, scenario: anyOf(...ROUTE_CASES), expected: id, actual: optional(id),
    proofId: optional(id), forbiddenDirect: yesNo,
  },
  wss: {
    ...common, sessionId: id, attemptNumber: value => Number.isSafeInteger(value) && value >= 1,
    reused: yesNo, durationMs: ms, protocol: anyOf('http1', 'http2'), status: optional(value => Number.isInteger(value) && value >= 100 && value < 600),
    negotiated: yesNo, independentId: optional(id),
  },
  attempt: {
    ...common, taskId: id, transport: anyOf(...TRANSPORTS), pairId: optional(id), comparisonId: optional(id),
    startMs: ms, endMs: ms, firstEventMs: optional(ms), firstTextMs: optional(ms),
    outcome: anyOf('completed', 'interrupted', 'timed-out', 'cancelled', 'service-error', 'request-failed'),
    responseId: optional(id), terminalResponseId: optional(id), terminalEvent: optional(anyOf('response.completed', 'response.failed', 'response.incomplete', 'error')),
    egress: optional(id), egressProof: optional(id), independentId: optional(id), faultId: optional(id),
    txBytes: optional(bytes), rxBytes: optional(bytes), multiplier: optional(multiplier),
  },
  task: { ...common, comparisonId: optional(id), outcome: anyOf('completed', 'failed', 'cancelled', 'unknown') },
  connection: {
    ...common, transport: anyOf(...TRANSPORTS), durationMs: ms,
    outcome: anyOf('normal', 'unexpected', 'planned', 'cancelled', 'unknown'),
  },
  fault: {
    ...common, atMs: ms, observedUntilMs: ms, failedEgress: id, standbyEgress: id, injectionProof: id,
  },
  switchWindow: { ...common, startMs: ms, endMs: ms, complete: yesNo },
  switch: {
    ...common, atMs: ms, windowId: id, cause: anyOf('automatic', 'manual', 'rule', 'budget'),
    previousHealth: anyOf('healthy', 'unhealthy', 'unknown'), healthProof: optional(id),
  },
};

function validateRow(row, index, evidence) {
  const { seen, egressByProof, handshakeKeys, completedResponses, injections } = evidence;
  const reject = message => { throw new Error(`记录 ${index + 1} ${message}`); };
  if (!row || Array.isArray(row) || typeof row !== 'object' ||
      typeof row.type !== 'string' || !Object.hasOwn(schemas, row.type)) reject('类型无效');
  if ((index === 0) !== (row.type === 'run')) reject('第一条且仅第一条记录须为 run');
  const schema = schemas[row.type];
  if (Object.keys(row).some(key => key !== 'type' && !Object.hasOwn(schema, key))) reject('存在未声明的字段');
  for (const [key, check] of Object.entries(schema)) {
    if (!check(row[key])) reject(`字段 ${key} 无效`);
  }
  const key = `${row.type}:${row.arm ?? ''}:${row.id}`;
  if (seen.has(key)) reject('编号重复');
  seen.add(key);
  if (row.type === 'wss') {
    const handshake = `${row.arm}:${row.sessionId}:${row.attemptNumber}`;
    if (handshakeKeys.has(handshake)) reject('同一会话的握手序号重复');
    handshakeKeys.add(handshake);
  }
  if (row.type === 'attempt') {
    if (row.startMs > row.endMs || (row.firstEventMs != null && (
      row.startMs > row.firstEventMs || row.firstEventMs > row.endMs
    )) || (row.firstTextMs != null && (
      row.firstEventMs == null || row.firstEventMs > row.firstTextMs || row.firstTextMs > row.endMs
    ))) reject('请求时间顺序无效');
    if ((row.egress == null) !== (row.egressProof == null)) reject('出口和证据编号须同时提供');
    if (row.egressProof != null) {
      const prior = egressByProof.get(row.egressProof);
      if (prior && prior.egress !== row.egress) reject('同一证据指向多个出口');
      if (prior?.independentId && row.independentId && prior.independentId !== row.independentId) reject('同一出口证据跨越独立试验单元');
      if (!prior?.independentId) egressByProof.set(row.egressProof, { egress: row.egress, independentId: row.independentId });
    }
    if (completed(row)) {
      const response = `${row.arm}:${row.responseId}`;
      if (completedResponses.has(response)) reject('成功响应编号重复');
      completedResponses.add(response);
    }
  }
  if (row.type === 'fault') {
    if (row.atMs > row.observedUntilMs || row.failedEgress === row.standbyEgress) reject('故障时序或出口无效');
    const injection = `${row.arm}:${row.injectionProof}`;
    if (injections.has(injection)) reject('同一故障注入证据重复');
    injections.add(injection);
  }
  if (row.type === 'switchWindow' && row.startMs >= row.endMs) reject('观测窗口无效');
}

export function parseNdjson(text) {
  if (typeof text !== 'string' || Buffer.byteLength(text) > MAX_BYTES) throw new Error('数据文件不得超过 16 MiB');
  return text.replace(/^\uFEFF/, '').split(/\r?\n/).filter(line => line.trim()).map((line, index) => {
    try {
      return JSON.parse(line);
    } catch {
      throw new Error(`记录 ${index + 1} JSON 格式无效`);
    }
  });
}

function p95(values) {
  if (!values.length) return null;
  return [...values].sort((a, b) => a - b)[Math.ceil(values.length * 0.95) - 1];
}

function group(rows, key) {
  const groups = new Map();
  for (const row of rows) {
    const name = key(row);
    if (!groups.has(name)) groups.set(name, []);
    groups.get(name).push(row);
  }
  return groups;
}

function result(id, rows, detail, { breach = false, enough = true } = {}) {
  return {
    id, target: excellent[id],
    status: !rows.length ? 'missing' : breach ? 'fails' : enough ? 'meets' : 'insufficient',
    ...detail,
  };
}

function durationResult(id, rows, durations, hardFailures = false) {
  const limit = excellent[id];
  const p = p95(durations);
  const failed = durations.filter(value => !Number.isFinite(value)).length;
  return result(id, rows, { trials: rows.length, p95Ms: Number.isFinite(p) ? p : null, failed }, {
    breach: p > limit.p95Ms || (hardFailures && failed > 0),
    enough: rows.length >= limit.minTrials,
  });
}

function matrixResult(id, rows, pass, known = () => true) {
  const cases = excellent[id].cases;
  const knownRows = rows.filter(known);
  const failed = knownRows.filter(row => !pass(row)).length;
  const missingCases = cases.filter(scenario => !knownRows.some(row => row.scenario === scenario));
  return result(id, rows, { trials: rows.length, failed, unverified: rows.length - knownRows.length, missingCases }, {
    breach: failed > 0,
    enough: missingCases.length === 0 && knownRows.length === rows.length,
  });
}

function cumulativeTail(logMass, failures, logRatio) {
  let peak = logMass;
  let scaled = 1;
  for (let k = 1; k <= failures; k += 1) {
    logMass += logRatio(k);
    if (logMass > peak) {
      scaled = scaled * Math.exp(peak - logMass) + 1;
      peak = logMass;
    } else {
      scaled += Math.exp(logMass - peak);
    }
  }
  return Math.min(1, Math.exp(peak + Math.log(scaled)));
}

// Exact one-sided test at the maximum allowed failure probability. Each
// independent unit fails if any of its observations fails.
function binomialTail(n, failures, probability) {
  const logOdds = Math.log(probability / (1 - probability));
  return cumulativeTail(n * Math.log1p(-probability), failures, k => Math.log((n - k + 1) / k) + logOdds);
}

function rateEvidence(rows, pass, target) {
  const failures = rows.filter(row => !pass(row)).length;
  const independent = group(rows.filter(row => row.independentId != null), row => row.independentId);
  const units = [...independent.values()];
  const unitFailures = units.filter(unit => unit.some(row => !pass(row))).length;
  const completeIds = rows.every(row => row.independentId != null);
  const failureBudget = 1 - target.successRate;
  const tail = units.length ? binomialTail(units.length, unitFailures, failureBudget) : null;
  const successRate = rows.length ? (rows.length - failures) / rows.length : null;
  return {
    trials: rows.length, failures, successRate, independentTrials: units.length, independentFailures: unitFailures,
    confidenceTail: tail,
    breach: successRate != null && successRate < target.successRate,
    enough: completeIds && units.length >= target.minIndependentTrials && tail <= CONFIDENCE_TAIL,
  };
}

function completed(row) {
  return row.outcome === 'completed' && row.firstEventMs != null && row.responseId != null &&
    row.terminalEvent === 'response.completed' && row.terminalResponseId === row.responseId;
}

function egressCompletion(started) {
  const observed = group(started.filter(row => row.egressProof != null), row => `${row.transport}:${row.egress}`);
  return [...observed.values()].map(rows => ({
    transport: rows[0].transport, egress: rows[0].egress,
    started: rows.length, completed: rows.filter(completed).length,
  }));
}

function handshakePassed(row) {
  const protocolStatus = row.protocol === 'http1' ? row.status === 101 : row.status >= 200 && row.status < 300;
  return row.durationMs <= excellent['wss-first-handshake'].withinMs && row.negotiated && protocolStatus;
}

function firstText(attempts, comparisonId) {
  const pairs = group(attempts.filter(row => row.pairId != null), row => row.pairId);
  const matches = TRANSPORTS.map(transport => ({ transport, durations: [], missing: 0 }));
  let unclassified = 0;
  for (const rows of pairs.values()) {
    const first = Object.fromEntries(ARMS.map(arm => [
      arm, rows.filter(row => row.arm === arm).sort((a, b) => a.startMs - b.startMs)[0],
    ]));
    const { candidate, control } = first;
    const evidence = matches.find(entry => entry.transport === candidate?.transport);
    if (!evidence) {
      unclassified += 1;
      continue;
    }
    if (candidate.transport !== control?.transport || !comparisonId ||
        [candidate, control].some(row => row.firstTextMs == null || row.outcome === 'cancelled' || row.comparisonId !== comparisonId)) {
      evidence.missing += 1;
      continue;
    }
    const added = (candidate.firstTextMs - candidate.startMs) - (control.firstTextMs - control.startMs);
    evidence.durations.push(Math.max(0, added));
  }
  return {
    unclassified,
    transports: matches.map(({ transport, durations, missing }) => ({
      transport, pairs: durations.length, missing, p95Ms: p95(durations),
    })),
  };
}

function disconnectEvidence(rows) {
  const { per100Hours, minConnectionHours } = excellent['unexpected-disconnects'];
  const exposure = rows.reduce((sum, row) => sum + row.durationMs, 0) / HOUR;
  const unexpected = rows.filter(row => row.outcome === 'unexpected').length;
  const unclassified = rows.filter(row => row.outcome === 'unknown').length;
  const rate = exposure ? 100 * unexpected / exposure : null;
  const mean = exposure * per100Hours / 100;
  const confidenceTail = exposure ? cumulativeTail(-mean, unexpected, k => Math.log(mean / k)) : null;
  return {
    connectionHours: exposure, unexpected, unclassified, per100Hours: rate, confidenceTail,
    breach: rate > per100Hours,
    enough: unclassified === 0 && exposure >= minConnectionHours && confidenceTail <= CONFIDENCE_TAIL,
  };
}

function failoverTimes(faults, attempts) {
  const byFault = group(attempts.filter(row => row.faultId != null), row => row.faultId);
  const { observationMs } = excellent['failover-recovery'];
  return faults.map(fault => {
    const deadline = fault.atMs + observationMs;
    const observedUntil = Math.min(fault.observedUntilMs, deadline);
    return (byFault.get(fault.id) ?? []).reduce((best, row) => {
      if (row.startMs < fault.atMs || row.endMs > observedUntil || !completed(row) ||
          row.egressProof == null || row.egress !== fault.standbyEgress) return best;
      return Math.min(best, row.firstEventMs - fault.atMs);
    }, Infinity);
  });
}

function switchEvidence(windows, switches) {
  const complete = windows.filter(row => row.complete).sort((a, b) => a.startMs - b.startMs);
  let longest = 0;
  let start = 0;
  let end = 0;
  for (const row of complete) {
    if (row.startMs > end) start = row.startMs;
    end = Math.max(end, row.endMs);
    longest = Math.max(longest, end - start);
  }
  const known = new Map(complete.map(row => [row.id, row]));
  let unnecessary = 0;
  let unclassified = 0;
  for (const row of switches) {
    if (row.cause !== 'automatic') continue;
    const window = known.get(row.windowId);
    if (!window || row.atMs < window.startMs || row.atMs >= window.endMs || !row.healthProof || row.previousHealth === 'unknown') {
      unclassified += 1;
    } else if (row.previousHealth === 'healthy') {
      unnecessary += 1;
    }
  }
  return { healthyHours: longest / HOUR, unnecessary, unclassified, incompleteWindows: windows.length - complete.length };
}

function costEvidence(attempts, tasks, comparisonId) {
  const outcomes = new Map(tasks.map(row => [row.id, row]));
  const attemptTasks = group(attempts, row => row.taskId);
  const missing = attempts.filter(row => row.txBytes == null || row.rxBytes == null || row.multiplier == null ||
    comparisonId == null || row.comparisonId !== comparisonId).length;
  let completedTasks = 0;
  let unknownTasks = 0;
  for (const [id, rows] of attemptTasks) {
    const task = outcomes.get(id);
    if (!task || !comparisonId || task.comparisonId !== comparisonId || task.outcome === 'unknown' ||
        (task.outcome === 'completed' && !rows.some(completed))) {
      unknownTasks += 1;
    } else if (task.outcome === 'completed') {
      completedTasks += 1;
    }
  }
  unknownTasks += tasks.filter(row => !attemptTasks.has(row.id)).length;
  const weightedMiBPerCompletedTask = !missing && !unknownTasks && completedTasks > 0
    ? attempts.reduce((sum, row) => sum + (row.txBytes + row.rxBytes) * row.multiplier, 0) / 2 ** 20 / completedTasks
    : null;
  return { completedTasks, unknownTasks, attemptsMissingTrafficOrComparison: missing, weightedMiBPerCompletedTask };
}

export function evaluate(rows) {
  if (!Array.isArray(rows) || !rows.length) throw new Error('缺少 run 记录');
  const evidence = {
    seen: new Set(), egressByProof: new Map(), handshakeKeys: new Set(),
    completedResponses: new Set(), injections: new Set(),
  };
  rows.forEach((row, index) => validateRow(row, index, evidence));
  const [run, ...records] = rows;
  const byArm = group(records, row => row.arm);
  const byType = group(byArm.get('candidate') ?? [], row => row.type);
  const candidate = type => byType.get(type) ?? [];
  const attempts = candidate('attempt');
  const allAttempts = records.filter(row => row.type === 'attempt');
  const startup = candidate('startup');
  const restore = candidate('restore');
  const routes = candidate('route');
  const handshakeRows = candidate('wss');
  const firstHandshakes = handshakeRows.filter(row => row.attemptNumber === 1 && !row.reused);
  const firstSessions = new Set(firstHandshakes.map(row => row.sessionId));
  const unmatchedFirst = new Set(handshakeRows.filter(row => !row.reused && row.attemptNumber > 1 &&
    !firstSessions.has(row.sessionId)).map(row => row.sessionId)).size;
  const handshake = rateEvidence(firstHandshakes, handshakePassed, excellent['wss-first-handshake']);
  const started = attempts.filter(row => row.firstEventMs != null && row.outcome !== 'cancelled');
  const stream = TRANSPORTS.map(transport => ({
    transport, ...rateEvidence(started.filter(row => row.transport === transport), completed, excellent['model-stream-completion']),
  }));
  const firstTextResult = firstText(allAttempts, run.comparisonId);
  const connections = candidate('connection');
  const disconnects = TRANSPORTS.map(transport => ({
    transport, ...disconnectEvidence(connections.filter(row => row.transport === transport)),
  }));
  const faults = candidate('fault');
  const windows = candidate('switchWindow');
  const switches = candidate('switch');
  const switchResult = switchEvidence(windows, switches);
  const cost = Object.fromEntries(ARMS.map(arm => {
    const rows = byArm.get(arm) ?? [];
    return [arm, costEvidence(rows.filter(row => row.type === 'attempt'), rows.filter(row => row.type === 'task'), run.comparisonId)];
  }));
  const baseline = cost.control.weightedMiBPerCompletedTask;
  const measured = cost.candidate.weightedMiBPerCompletedTask;
  const ratio = baseline > 0 && measured != null ? measured / baseline : null;
  const checks = [
    durationResult('startup-ready', startup, startup.map(row => row.controllerReady && row.expectedEgress ? row.durationMs : Infinity), true),
    matrixResult('state-restore', restore, row => row.stateMatches && row.ownerPreserved && row.durationMs <= excellent['state-restore'].withinMs),
    matrixResult('route-fidelity', routes, row => !row.forbiddenDirect && row.expected === row.actual,
      row => row.forbiddenDirect || (row.actual != null && row.proofId != null)),
    result('wss-first-handshake', handshakeRows, { ...handshake, unmatchedFirst }, {
      breach: handshake.breach, enough: handshake.enough && unmatchedFirst === 0,
    }),
    result('model-stream-completion', attempts, {
      transports: stream, cancelled: attempts.filter(row => row.outcome === 'cancelled').length,
      preStreamFailures: attempts.filter(row => row.firstEventMs == null && row.outcome !== 'cancelled').length,
      unattributedAttempts: attempts.filter(row => row.egressProof == null).length,
      verifiedEgress: egressCompletion(started),
    }, { breach: stream.some(row => row.breach), enough: stream.every(row => row.enough) }),
    result('paired-first-text', allAttempts, firstTextResult, {
      breach: firstTextResult.transports.some(row => row.p95Ms > excellent['paired-first-text'].p95Ms),
      enough: firstTextResult.unclassified === 0 && firstTextResult.transports.every(row => row.missing === 0 && row.pairs >= excellent['paired-first-text'].minPairs),
    }),
    result('unexpected-disconnects', connections, { transports: disconnects }, {
      breach: disconnects.some(row => row.breach), enough: disconnects.every(row => row.enough),
    }),
    durationResult('failover-recovery', faults, failoverTimes(faults, attempts), true),
    result('unnecessary-switches', [...windows, ...switches], switchResult, {
      breach: switchResult.unnecessary > excellent['unnecessary-switches'].count,
      enough: !switchResult.incompleteWindows && !switchResult.unclassified && switchResult.healthyHours >= excellent['unnecessary-switches'].minHealthyHours,
    }),
    result('weighted-task-traffic', allAttempts, { ratio, arms: cost }, {
      breach: ratio > excellent['weighted-task-traffic'].maxRatio,
      enough: ratio != null && ARMS.every(arm => cost[arm].completedTasks >= excellent['weighted-task-traffic'].minCompletedTasksPerArm),
    }),
  ];
  const qualification = run.source === 'fixture' ? 'fixture-only'
    : checks.every(check => check.status === 'meets') ? 'observed-excellent'
      : checks.some(check => check.status === 'fails') ? 'observed-fails' : 'observed-insufficient';
  return { schemaVersion: 1, run: run.id, source: run.source, networkMode: run.networkMode, qualification, checks };
}

async function main(args) {
  if (args.length === 1 && args[0] === '--fixture') {
    const { fixtureRows } = await import('./proxy-quality-fixture.mjs');
    return evaluate(fixtureRows(excellent));
  }
  if (args.length < 2 || args.length > 3 || args[0] !== 'evaluate' ||
      (args.length === 3 && args[2] !== '--require-excellent')) {
    throw new Error('用法为 node scripts/proxy-quality.mjs --fixture 或 evaluate <记录.ndjson> [--require-excellent]');
  }
  if ((await stat(args[1])).size > MAX_BYTES) throw new Error('数据文件不得超过 16 MiB');
  const summary = evaluate(parseNdjson(await readFile(args[1], 'utf8')));
  if (args.length === 3 && summary.qualification !== 'observed-excellent') process.exitCode = 1;
  return summary;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  try {
    console.log(JSON.stringify(await main(process.argv.slice(2)), null, 2));
  } catch (error) {
    console.error(error.code ? '读取测量记录失败' : error.message);
    process.exitCode = 2;
  }
}
