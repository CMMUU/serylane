import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(new URL("../src/program-proxy.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText;
const { parseProgramArguments, suggestedProgramName, launchBlockReason, programManagerMarkup } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);

test("arguments are one literal argument per line, not shell syntax", () => {
  assert.deepEqual(parseProgramArguments("--option\r\nC:\\folder with spaces\n\n& echo not-a-command"), ["--option", "C:\\folder with spaces", "& echo not-a-command"]);
  assert.deepEqual(parseProgramArguments("  kept spaces  \n\t"), ["  kept spaces  "]);
  assert.deepEqual(parseProgramArguments(""), []);
  assert.throws(() => parseProgramArguments("x\0y"));
  assert.throws(() => parseProgramArguments("x\ry"));
  assert.throws(() => parseProgramArguments("中".repeat(3000)));
  assert.throws(() => parseProgramArguments(Array(65).fill("x").join("\n")));
});

test("name suggestions only remove exe extension and retain Unicode", () => {
  assert.equal(suggestedProgramName("C:\\应用 文件夹\\开发工具.EXE"), "开发工具");
  assert.equal(suggestedProgramName("C:/apps/demo.exe"), "demo");
});

test("launch is unavailable for unsupported, missing, running or stopped cases", () => {
  const state = { supported: true, coreRunning: true };
  const program = { available: true, runningPid: null };
  assert.equal(launchBlockReason(program, state), null);
  assert.match(launchBlockReason(program, { ...state, supported: false }), /Windows/);
  assert.match(launchBlockReason({ ...program, available: false }, state), /找不到/);
  assert.match(launchBlockReason({ ...program, runningPid: 4567 }, state), /退出/);
  assert.match(launchBlockReason(program, { ...state, coreRunning: false }), /核心/);
});

test("program UI uses scoped API and explicit confirmation, not global proxy mutations", () => {
  assert.match(programManagerMarkup, /不强制接管已运行的程序/);
  assert.match(programManagerMarkup, /保存不会启动程序/);
  assert.match(source, /services\.confirm\(\{ title: `代理启动/);
  assert.match(source, /saveProxyProgram\(input, draftRevision!/);
  assert.match(source, /if \(path === null\) return/);
  assert.doesNotMatch(source, /setNetworkMode|startActive|updateSettings|\.kill\(|setx|localStorage/);
});

test("program page shares the single accessible navigation styling contract", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const css = readFileSync(new URL("../src/desktop-theme.css", import.meta.url), "utf8");
  const ui = readFileSync(new URL("../src/ui.ts", import.meta.url), "utf8");
  assert.match(main, /id="programs-view"/);
  assert.match(main, /if \(view === "programs"\) void programManager\.refresh\(\)/);
  assert.match(ui, /id: "programs", label: "程序代理"/);
  assert.match(css, /\.sidebar \.nav-item\[aria-current="page"\]/);
  assert.match(main, /button\.setAttribute\("aria-current", "page"\)/);
  assert.match(main, /button\.removeAttribute\("aria-current"\)/);
  assert.doesNotMatch(css, /body:has\(#programs-view/);
});

test("compatibility diagnostics never enable system routing for unrelated clients", () => {
  const cargo = readFileSync(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8");
  const lib = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
  const controller = readFileSync(new URL("../src-tauri/src/mihomo_api.rs", import.meta.url), "utf8");
  assert.doesNotMatch(cargo, /client-proxy-system/);
  assert.match(controller, /reqwest::Client::builder\(\)\s*\.no_proxy\(\)/);
  assert.match(lib, /if result\.updated && result\.profile\.openai_policy\.auto_maintain/);
});

test("bound application states explain installation failures rather than a generic missing file", () => {
  for (const availability of ["not_installed", "updating", "maintenance", "needs_relink", "read_error", "unsupported_launch"]) {
    const program = { available: false, runningPid: null, resolution: { availability, detail: `说明：${availability}` } };
    assert.equal(launchBlockReason(program, { supported: true, coreRunning: true }), `说明：${availability}`);
  }
});

test("installed-app binding and portable files keep independent directory and identity settings", () => {
  assert.match(programManagerMarkup, /从已安装应用选择/);
  assert.match(programManagerMarkup, /选择 \.exe 文件/);
  assert.match(programManagerMarkup, /包内相对目录/);
  assert.match(source, /readOnly = binding !== null/);
  assert.match(source, /installedProxyApplications\(\)/);
  assert.match(source, /workingDirectoryRelative:/);
  assert.match(source, /editProgram\(program\)/);
  assert.match(source, /program-path-details/);
});

test("README uses token-free theme-aware Star History without adding a chart scheduler", () => {
  const readme = readFileSync(new URL("../README.md", import.meta.url), "utf8");
  assert.ok(readme.indexOf("## Star 增长趋势") > readme.indexOf("## 快速开始"));
  assert.match(readme, /prefers-color-scheme: dark/);
  assert.match(readme, /https:\/\/api\.star-history\.com\/chart\?repos=CMMUU\/serylane/);
  assert.doesNotMatch(readme, /sealed_token|[?&]token=/);
});
