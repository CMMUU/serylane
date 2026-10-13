import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
const read = path => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const source = read("src/log-view.ts");
const js = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText;
const { filterLogs, applicationLogHelp } = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);

test("log help follows saved retention, independently of storage size", () => {
  for (const days of [1, 3, 7, 90]) {
    const help = applicationLogHelp(days * 24, 262144);
    assert.ok(help.includes(`保留 ${days} 天`));
    assert.ok(help.includes("256 KiB"));
  }
  assert.match(read("src/settings-view.ts"), /id="settings-app-log-retention"[^>]*min="1"[^>]*max="90"/);
});

test("log filtering handles translated sources, both warning spellings and empty results", () => {
  const entries = [
    { timestamp: 0, level: "warning", source: "网络预检", message: "TLS 证书校验失败" },
    { timestamp: 1, level: "info", source: "应用", message: "Serylane 已启动" },
  ];
  assert.equal(filterLogs(entries, "warn", "tls").length, 1);
  assert.equal(filterLogs(entries, "all", " 网络预检 ").length, 1);
  assert.equal(filterLogs(entries, "error", "").length, 0);
  assert.equal(filterLogs(entries, "all", "").length, 2);
});

test("log content is text-only and refresh preserves source, filters and reading position", () => {
  assert.match(source, /message\.textContent = entry\.message/);
  assert.match(source, /requested !== revision/);
  assert.match(source, /list\.scrollTop = scroll/);
  assert.match(source, /if \(!confirmed \|\| source\.value !== selected\) return/);
  assert.doesNotMatch(source, /innerHTML\s*=\s*(?:entry|entries)/);
  assert.match(read("src/main.ts"), /if \(store\.view === "logs"\) void refreshLogs\(\);\s*if \(store\.runtime\?\.phase === "running"\)/);
});

test("startup mode composes existing preferences without coupling Codex routing", () => {
  assert.match(read("src/settings-view.ts"), /settings-startup-mode/);
  assert.match(read("src/session-resume.ts"), /silentStartup: mode === "background", restoreLastSession: true/);
  assert.match(read("src/types.ts"), /silentStartup: boolean/);
  assert.doesNotMatch(read("src/main.ts"), /data-openai-action="stability"/);
  assert.match(read("src/main.ts"), /无需开启本地路由或接入 Codex/);
});
