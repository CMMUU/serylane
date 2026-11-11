const HOUR = 3_600_000;
const comparisonId = 'isolated-fixture';
const candidate = 'candidate';

function modelAttempt(arm, name, transport, startMs, extra) {
  const responseId = `response-${arm}-${name}`;
  return {
    type: 'attempt', id: name, taskId: name, arm, transport, comparisonId,
    startMs, endMs: startMs + 240, firstEventMs: startMs + 20, firstTextMs: startMs + 80,
    outcome: 'completed', responseId, terminalResponseId: responseId, terminalEvent: 'response.completed',
    egress: arm === candidate ? 'candidate-node' : 'control-node', egressProof: `egress-${arm}-${name}`,
    independentId: `trial-${name}`, txBytes: 200, rxBytes: 800, multiplier: 1,
    ...extra,
  };
}

export function fixtureRows(excellent, rateTrials = excellent['wss-first-handshake'].minIndependentTrials) {
  const rows = [{ type: 'run', id: 'deterministic-fixture', schemaVersion: 1, source: 'fixture', networkMode: 'system-proxy', comparisonId }];
  for (let i = 0; i < excellent['startup-ready'].minTrials; i += 1) {
    rows.push({ type: 'startup', id: `startup-${i}`, arm: candidate, durationMs: 1500, controllerReady: true, expectedEgress: true });
  }
  for (const scenario of excellent['state-restore'].cases) {
    rows.push({ type: 'restore', id: scenario, arm: candidate, scenario, durationMs: 1300, stateMatches: true, ownerPreserved: true });
  }
  for (const scenario of excellent['route-fidelity'].cases) {
    const expected = scenario === 'block' ? 'blocked' : scenario === 'direct' ? 'direct' : 'fixture-egress';
    rows.push({ type: 'route', id: scenario, arm: candidate, scenario, expected, actual: expected, proofId: `route-${scenario}`, forbiddenDirect: false });
  }
  for (let i = 0; i < rateTrials; i += 1) {
    const protocol = i % 2 ? 'http2' : 'http1';
    rows.push({
      type: 'wss', id: `handshake-${i}`, arm: candidate, sessionId: `wss-${i}`, attemptNumber: 1, reused: false,
      durationMs: 600, protocol, status: protocol === 'http1' ? 101 : 200, negotiated: true, independentId: `wss-trial-${i}`,
    });
    for (const transport of ['sse', 'wss']) {
      const name = `${transport}-${i}`;
      for (const arm of [candidate, 'control']) {
        const startMs = i * 1000 + (arm === candidate ? 0 : 300);
        rows.push(
          modelAttempt(arm, name, transport, startMs, { pairId: name }),
          { type: 'task', id: name, arm, comparisonId, outcome: 'completed' },
        );
      }
    }
  }
  for (const transport of ['sse', 'wss']) {
    for (let i = 0; i < excellent['unexpected-disconnects'].minConnectionHours; i += 1) {
      rows.push({ type: 'connection', id: `${transport}-open-${i}`, arm: candidate, transport, durationMs: HOUR, outcome: 'planned' });
    }
  }
  for (let i = 0; i < excellent['failover-recovery'].minTrials; i += 1) {
    const name = `fault-${i}`;
    const atMs = 10_000_000 + i * 200_000;
    rows.push({
      type: 'fault', id: name, arm: candidate, atMs, observedUntilMs: atMs + 180000,
      failedEgress: 'old-node', standbyEgress: 'standby-node', injectionProof: `injection-${i}`,
    });
    for (const arm of [candidate, 'control']) {
      const extra = arm === candidate ? { egress: 'standby-node', faultId: name } : {};
      rows.push(
        modelAttempt(arm, name, 'sse', atMs + 1000, extra),
        { type: 'task', id: name, arm, comparisonId, outcome: 'completed' },
      );
    }
  }
  rows.push({
    type: 'switchWindow', id: 'healthy-window', arm: candidate, startMs: 0,
    endMs: excellent['unnecessary-switches'].minHealthyHours * HOUR, complete: true,
  });
  return rows;
}
