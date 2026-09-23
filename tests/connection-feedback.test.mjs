import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import ts from 'typescript';
const read = path => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const js = ts.transpileModule(read('src/connection-feedback.ts'), { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText;
const { feedbackPresentation: present, friendlyError, escapeFeedbackText } = await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
const state = overrides => ({ revision: 1, operation: 1, phase: 'enabled', mode: 'system_proxy', health: 'checking', retrying: false, elapsedMs: 500, checks: [], issue: null, ...overrides });
const check = (target, success, failureKind = null) => ({ target, success, failureKind, latencyMs: 500, detail: 'fixture' });

test('local enablement is explicit while network health is pending or fails', () => {
  for (const health of ['checking', 'unavailable', 'partial']) {
    const p = present(state({ health }));
    assert.equal(p.title, '系统代理已开启');
    assert.doesNotMatch(p.description, /已关闭|开启失败|所有网站.*正常/);
  }
  assert.equal(present(state({ health: 'unavailable' })).action, 'recheck');
});
test('progress follows real phase and never guesses a percentage or completion time', () => {
  for (const phase of ['validating', 'starting', 'applying']) {
    const p = present(state({ phase }));
    assert.equal(p.busy, true);
    assert.doesNotMatch(p.title, /已开启|成功|\d+%|即将/);
    assert.equal(p.action, '', 'no decorative cancellation button');
  }
});
test('only successful basic probes permit the basic-network success message', () => {
  const p = present(state({ health: 'partial', checks: [check('google', true), check('cloudflare', true), check('openai', false)] }));
  assert.match(p.description, /基础连接正常，OpenAI 检查未通过/);
  const aiOnly = present(state({ health: 'partial', checks: [check('google', false), check('cloudflare', false), check('openai', true)] }));
  assert.doesNotMatch(aiOnly.description, /基础连接正常/);
});
test('401 reachability is not described as a verified model session', () => {
  const p = present(state({ health: 'healthy' }));
  assert.match(p.description, /模型会话尚未验证/);
});
test('certificate validation and connection EOF get different advice', () => {
  const cert = present(state({ health: 'unavailable', checks: [check('google', false, 'certificate')] }));
  const eof = present(state({ health: 'unavailable', checks: [check('google', false, 'tls')] }));
  assert.match(cert.description, /校验未通过/);
  assert.match(eof.description, /连接建立时中断/);
  assert.doesNotMatch(eof.description, /证书|系统日期/);
});
test('known structured reason wins; unknown English text does not invent a cause', () => {
  assert.equal(friendlyError({ code: 'STATE_CONFLICT', message: 'tls EOF occupied' }).action, 'refresh');
  assert.doesNotMatch(friendlyError(new Error('certificate')).title, /证书/);
  const supplied = { title: '代理端口正在被使用', description: '打开设置', action: 'settings', details: 'port fixture' };
  assert.deepEqual(friendlyError({ userMessage: supplied }), supplied);
});
test('diagnostic details remain text rather than executable markup', () => {
  assert.equal(escapeFeedbackText('<img src=x onerror=run()>'), '&lt;img src=x onerror=run()&gt;');
  assert.match(read('src/connection-feedback.ts'), /find\("detail-text"\)\.textContent/);
});
test('System Proxy acceptance is independent of the external probe and helper UI readback', () => {
  const backend = read('src-tauri/src/lib.rs');
  const finish = backend.slice(backend.indexOf('async fn finish_runtime_start'), backend.indexOf('fn active_effective_config'));
  const system = finish.slice(finish.indexOf('if settings.network_mode == NetworkMode::SystemProxy'), finish.indexOf('} else if settings.network_mode == NetworkMode::Tun'));
  assert.doesNotMatch(system, /verify_local_proxy|inspect_local_proxy|verify_tun_route/);
  assert.match(system, /verify_system_proxy/);
  const main = read('src/main.ts');
  const refresh = main.slice(main.indexOf('async function refreshRuntimeOnly'), main.indexOf('async function startRuntime('));
  assert.doesNotMatch(refresh, /await.*tunHelperStatus/);
  assert.match(main, /if \(wasRunning && !keepCore\) \{\s*await api\.stop\(\);\s*stoppedPrevious = true;/);
  assert.match(main, /if \(!keepCore && store\.activeProfile/);
});
test('network health has cancellation and config/core attribution guards, no mutating work', () => {
  const module = read('src-tauri/src/connection_feedback.rs');
  assert.match(module, /receiver\.changed\(\)/);
  assert.match(module, /Context::read\(&app\).*Some\(&context\)/);
  assert.doesNotMatch(module, /enable_system_proxy\(|restore_system_proxy\(|runtime\.stop\(/);
});
