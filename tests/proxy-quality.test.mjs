import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, rmdir, unlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { evaluate, excellent, parseNdjson } from '../scripts/proxy-quality.mjs';
import { fixtureRows } from '../scripts/proxy-quality-fixture.mjs';

const script = fileURLToPath(new URL('../scripts/proxy-quality.mjs', import.meta.url));
const run = (rows, name) => evaluate(rows).checks.find(check => check.id === name);
const smallFixture = () => fixtureRows(excellent, 24);
const first = (rows, type, arm = 'candidate') => rows.find(row => row.type === type && row.arm === arm);

test('the fixture exercises all ten excellent gates but cannot qualify real observations', () => {
  const result = evaluate(fixtureRows(excellent));
  assert.equal(result.qualification, 'fixture-only');
  assert.deepEqual(result.checks.map(check => check.id), Object.keys(excellent));
  assert.ok(result.checks.every(check => check.status === 'meets'));
  assert.equal(result.checks.find(check => check.id === 'failover-recovery').p95Ms, 1020);
  const artificialObserved = fixtureRows(excellent);
  artificialObserved[0].source = 'observed';
  assert.equal(evaluate(artificialObserved).qualification, 'observed-excellent');
});

test('tiny or correlated samples cannot certify a 99.9% rate', () => {
  const rows = smallFixture();
  assert.equal(run(rows, 'wss-first-handshake').status, 'insufficient');
  assert.equal(run(rows, 'model-stream-completion').status, 'insufficient');
  for (const row of rows.filter(row => row.type === 'wss')) row.independentId = 'same-session-window';
  assert.equal(run(rows, 'wss-first-handshake').independentTrials, 1);
  const oneShort = fixtureRows(excellent, 2994);
  assert.ok(run(oneShort, 'wss-first-handshake').confidenceTail > 0.05);
  assert.equal(run(oneShort, 'wss-first-handshake').status, 'insufficient');
});

test('startup and restore require the actual data plane, complete scenario coverage and ownership', () => {
  const rows = smallFixture();
  first(rows, 'startup').expectedEgress = false;
  assert.equal(run(rows, 'startup-ready').status, 'fails');
  first(rows, 'restore').ownerPreserved = false;
  assert.equal(run(rows, 'state-restore').status, 'fails');
  const partial = smallFixture().filter(row => row.type !== 'restore' || row.scenario !== 'third-party-owner');
  assert.deepEqual(run(partial, 'state-restore').missingCases, ['third-party-owner']);
  assert.equal(run(partial, 'state-restore').status, 'insufficient');
});

test('matching routing metadata alone is insufficient and forbidden direct egress fails the matrix', () => {
  const rows = smallFixture();
  const route = first(rows, 'route');
  delete route.proofId;
  assert.equal(run(rows, 'route-fidelity').status, 'insufficient');
  route.forbiddenDirect = true;
  assert.equal(run(rows, 'route-fidelity').status, 'fails');
});

test('a successful WSS retry cannot cover a failed first handshake', () => {
  const rows = smallFixture();
  const handshake = first(rows, 'wss');
  handshake.status = 200;
  rows.push({ ...handshake, id: 'retry', attemptNumber: 2, status: 101 });
  const result = run(rows, 'wss-first-handshake');
  assert.equal(result.trials, 24);
  assert.equal(result.failures, 1);
  assert.equal(result.status, 'fails');
  const firstMissing = rows.filter(row => row !== handshake);
  assert.equal(run(firstMissing, 'wss-first-handshake').unmatchedFirst, 1);
  assert.equal(run(firstMissing, 'wss-first-handshake').status, 'insufficient');
});

test('an SSE body without its successful matching terminal event is not a completed stream', () => {
  const rows = smallFixture();
  const attempt = first(rows, 'attempt');
  delete attempt.terminalEvent;
  const second = rows.find(row => row.type === 'attempt' && row.arm === 'candidate' && row.transport === 'sse' && row !== attempt);
  second.terminalResponseId = 'unrelated';
  const result = run(rows, 'model-stream-completion');
  assert.equal(result.transports.find(row => row.transport === 'sse').failures, 2);
  assert.equal(result.transports.find(row => row.transport === 'wss').failures, 0);
  assert.equal(result.status, 'fails');
});

test('first-text timing uses matched first attempts and exposes missing measurements', () => {
  const rows = smallFixture();
  const initial = first(rows, 'attempt');
  delete initial.firstTextMs;
  rows.push({
    ...initial, id: 'later-retry', responseId: 'retry-response', terminalResponseId: 'retry-response',
    startMs: 100, firstEventMs: 120, firstTextMs: 125,
  });
  const result = run(rows, 'paired-first-text');
  assert.equal(result.status, 'insufficient');
  assert.equal(result.transports.find(row => row.transport === 'sse').missing, 1);
  const slow = smallFixture();
  for (const row of slow.filter(row => row.type === 'attempt' && row.arm === 'candidate')) row.firstTextMs += 101;
  assert.equal(run(slow, 'paired-first-text').status, 'fails');
});

test('disconnect density stays separate by transport and unknown closes cannot qualify', () => {
  const rows = smallFixture();
  const conn = first(rows, 'connection');
  conn.outcome = 'unknown';
  const result = run(rows, 'unexpected-disconnects');
  assert.equal(result.status, 'insufficient');
  assert.equal(result.transports.find(row => row.transport === 'sse').unclassified, 1);
  assert.equal(result.transports.find(row => row.transport === 'wss').connectionHours, 600);
});

test('failover ignores unverified or wrong exits and fails on a missing trial recovery', () => {
  const rows = smallFixture();
  const replacement = rows.find(row => row.faultId != null);
  delete replacement.egress;
  delete replacement.egressProof;
  const result = run(rows, 'failover-recovery');
  assert.equal(result.status, 'fails');
  assert.equal(result.failed, 1);
  assert.equal(result.p95Ms, 1020);
  const late = smallFixture();
  const lateReplacement = late.find(row => row.faultId != null);
  const fault = late.find(row => row.id === lateReplacement.faultId && row.type === 'fault');
  lateReplacement.endMs = fault.atMs + 180001;
  assert.equal(run(late, 'failover-recovery').status, 'fails');
});

test('separate healthy windows do not add up to a continuous 72-hour run', () => {
  const rows = smallFixture();
  const window = first(rows, 'switchWindow');
  const half = window.endMs / 2;
  window.endMs = half;
  rows.push({ ...window, id: 'after-gap', startMs: half + 100, endMs: half * 2 + 100 });
  assert.equal(run(rows, 'unnecessary-switches').healthyHours, 36);
  assert.equal(run(rows, 'unnecessary-switches').status, 'insufficient');
  rows.push({
    type: 'switch', arm: 'candidate', id: 'auto-switch', atMs: 1, windowId: window.id,
    cause: 'automatic', previousHealth: 'healthy', healthProof: 'independent-probe',
  });
  assert.equal(run(rows, 'unnecessary-switches').status, 'fails');
});

test('traffic accounting includes failed attempts and preserves unknown multipliers', () => {
  const rows = smallFixture();
  const attempt = first(rows, 'attempt');
  rows.push({
    ...attempt, id: 'failed-costly-attempt', outcome: 'interrupted', txBytes: 100000, multiplier: 2,
  });
  assert.equal(run(rows, 'weighted-task-traffic').status, 'fails');
  delete attempt.multiplier;
  const result = run(rows, 'weighted-task-traffic');
  assert.equal(result.status, 'insufficient');
  assert.equal(result.ratio, null);
  assert.equal(result.arms.candidate.attemptsMissingTrafficOrComparison, 1);
});

test('parsing rejects duplicate identities, mismatched evidence, and extra private fields', () => {
  const [header, ...data] = smallFixture();
  const attempt = first(data, 'attempt');
  assert.throws(() => evaluate([header, attempt, attempt]), /编号重复/);
  assert.throws(() => evaluate([header, attempt, { ...attempt, id: 'different', egress: 'other' }]), /同一证据/);
  assert.throws(() => evaluate([header, attempt, { ...attempt, id: 'different', independentId: 'new-unit' }]), /跨越独立试验单元/);
  assert.throws(() => evaluate([header, attempt, { ...attempt, id: 'different', egressProof: 'new-proof' }]), /成功响应编号重复/);
  assert.throws(() => evaluate([header, { ...attempt, credentials: 'do-not-print-this' }]), error =>
    error.message.includes('存在未声明的字段') && !error.message.includes('do-not-print-this'));
  assert.throws(() => parseNdjson('{oops'), /JSON 格式无效/);
  const report = evaluate(parseNdjson('\uFEFF' + JSON.stringify({ ...header, source: 'observed' }) + '\r\n'));
  assert.equal(report.qualification, 'observed-insufficient');
  assert.ok(report.checks.every(check => check.status === 'missing'));
});

test('the CLI reads NDJSON, returns 2 for invalid evidence, and 1 for unqualified evidence only on request', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'serylane-proxy-quality-'));
  const path = join(dir, 'run.ndjson');
  try {
    await writeFile(path, smallFixture().map(row => JSON.stringify(row)).join('\n'), 'utf8');
    const fixture = spawnSync(process.execPath, [script, '--fixture'], { encoding: 'utf8', windowsHide: true });
    assert.equal(fixture.status, 0, fixture.stderr);
    assert.equal(JSON.parse(fixture.stdout).qualification, 'fixture-only');
    const parsed = spawnSync(process.execPath, [script, 'evaluate', path], { encoding: 'utf8', windowsHide: true });
    assert.equal(parsed.status, 0, parsed.stderr);
    assert.equal(JSON.parse(parsed.stdout).source, 'fixture');
    const strict = spawnSync(process.execPath, [script, 'evaluate', path, '--require-excellent'], { encoding: 'utf8', windowsHide: true });
    assert.equal(strict.status, 1, strict.stderr);
    await writeFile(path, '{bad', 'utf8');
    const invalid = spawnSync(process.execPath, [script, 'evaluate', path], { encoding: 'utf8', windowsHide: true });
    assert.equal(invalid.status, 2);
    assert.match(invalid.stderr, /JSON 格式无效/);
  } finally {
    await unlink(path).catch(error => { if (error.code !== 'ENOENT') throw error; });
    await rmdir(dir);
  }
});
