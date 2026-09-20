import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { readFileSync, mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import ts from "typescript";
import { verifyUpdateSignature } from "../scripts/verify-update-signature.mjs";
import { root, verifyVersions, setVersion, verifyReleaseReady } from "../scripts/version.mjs";

const source = readFileSync(new URL("../src/app-update.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText;
const { describeAppUpdate } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);
const status = (phase, available = true) => ({ phase, info: { available, source: "gitee", latestVersion: "v1.0.0" }, downloadedBytes: 50, totalBytes: 100, error: "HTTP 403" });

test("installation only becomes available after native verification of a newer release", () => {
  for (const phase of ["idle", "checking", "current", "ahead", "available", "downloading", "installing", "cancelled", "failed"]) assert.equal(describeAppUpdate(status(phase)).canInstall, false, phase);
  assert.equal(describeAppUpdate(status("ready")).canInstall, true);
  assert.equal(describeAppUpdate(status("ready", false)).canInstall, false);
  assert.equal(describeAppUpdate({ ...status("ready"), info: null }).canInstall, false);
});
test("failure and cancel can retry, but active operations cannot start another download", () => {
  for (const phase of ["available", "failed", "cancelled"]) assert.equal(describeAppUpdate(status(phase)).canDownload, true);
  for (const phase of ["checking", "downloading", "installing", "ready"]) assert.equal(describeAppUpdate(status(phase)).canDownload, false);
  assert.match(describeAppUpdate(status("available")).detail, /Gitee/);
  assert.match(describeAppUpdate(status("ahead", false)).detail, /不会自动降级/);
  assert.equal(describeAppUpdate(status("failed")).detail, "HTTP 403");
});
test("download progress is bounded and never NaN", () => {
  assert.equal(describeAppUpdate(status("downloading")).progress, 50);
  assert.equal(describeAppUpdate({ ...status("downloading"), downloadedBytes: 500 }).progress, 100);
  assert.equal(describeAppUpdate({ ...status("downloading"), totalBytes: 0 }).progress, 0);
});
test("automatic update preferences clearly describe the domestic-first fallback policy", () => {
  const settings = readFileSync(new URL("../src/settings-view.ts", import.meta.url), "utf8");
  assert.match(settings, /value="auto">自动（国内优先）/);
  assert.match(settings, /自动：Gitee → GitHub/);
  assert.match(settings, /Gitee 尚未同步新版时使用 GitHub/);
  assert.match(describeAppUpdate(status("idle")).detail, /优先 Gitee，GitHub 备用/);
});
test("updater uses confirmed native installation and separately saved non-network preferences", () => {
  const api = readFileSync(new URL("../src/api.ts", import.meta.url), "utf8");
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const rust = readFileSync(new URL("../src-tauri/src/app_update.rs", import.meta.url), "utf8");
  assert.match(api, /invoke<void>\("install_app_update", \{ versionTag, confirmed \}\)/);
  assert.match(main, /if \(!confirmed\) return;[\s\S]*api\.installAppUpdate\(version, true\)/);
  assert.match(main, /6 \* 60 \* 60 \* 1_000/);
  assert.match(main, /saveUpdatePreferences/);
  assert.match(rust, /candidate\.update\.install\(bytes\)/);
  assert.doesNotMatch(api, /downloadAndInstall|install_app_update[^\n]*(?:url|path|bytes)/);
});

function signedFixture(algorithm = "ED") {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const rawKey = publicKey.export({ type: "spki", format: "der" }).subarray(-32);
  const keyId = Buffer.from("fixture1");
  const bytes = Buffer.from("synthetic updater bytes; never an executable");
  const signature = sign(null, algorithm === "ED" ? createHash("blake2b512").update(bytes).digest() : bytes, privateKey);
  const comment = "timestamp:1000\tfile:fixture.bin";
  const global = sign(null, Buffer.concat([signature, Buffer.from(comment)]), privateKey);
  const encode = (value) => Buffer.from(value).toString("base64");
  const publicText = encode(`untrusted comment: test public key\n${encode(Buffer.concat([Buffer.from("Ed"), keyId, rawKey]))}\n`);
  const signatureText = encode(`untrusted comment: test signature\n${encode(Buffer.concat([Buffer.from(algorithm), keyId, signature]))}\ntrusted comment: ${comment}\n${encode(global)}\n`);
  return { bytes, publicText, signatureText };
}
test("publication verifies both Minisign file and comment signatures with native crypto", () => {
  for (const algorithm of ["Ed", "ED"]) {
    const fixture = signedFixture(algorithm);
    assert.equal(verifyUpdateSignature(fixture.bytes, fixture.signatureText, fixture.publicText), true);
    assert.throws(() => verifyUpdateSignature(Buffer.from("tampered"), fixture.signatureText, fixture.publicText), /verification failed/);
    const changedComment = Buffer.from(Buffer.from(fixture.signatureText, "base64").toString().replace("timestamp:1000", "timestamp:2000")).toString("base64");
    assert.throws(() => verifyUpdateSignature(fixture.bytes, changedComment, fixture.publicText), /verification failed/);
    assert.throws(() => verifyUpdateSignature(fixture.bytes, fixture.signatureText, signedFixture().publicText), /verification failed/);
    assert.throws(() => verifyUpdateSignature(fixture.bytes, "invalid!", fixture.publicText));
  }
});
test("all version fields and tag are gated; a single command updates every version", () => {
  const version = verifyVersions();
  assert.equal(verifyVersions(root, `v${version}`), version);
  assert.throws(() => verifyVersions(root, "v999.0.0"), /does not match/);
  const fixture = mkdtempSync(join(tmpdir(), "routedeck-version-test-"));
  try {
    mkdirSync(join(fixture, "src-tauri"));
    for (const path of ["package.json", "package-lock.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/tauri.conf.json"]) writeFileSync(join(fixture, path), readFileSync(join(root, path)));
    assert.equal(setVersion("1.23.4", fixture), "1.23.4");
    assert.throws(() => verifyReleaseReady(fixture), /Reviewed release notes are required/);
    mkdirSync(join(fixture, "docs"));
    writeFileSync(join(fixture, "docs/发布说明-v1.23.4.md"), " \n");
    assert.throws(() => verifyReleaseReady(fixture), /Reviewed release notes are required/);
    writeFileSync(join(fixture, "docs/发布说明-v1.23.4.md"), "# Serylane v1.23.4\n\nSynthetic release notes.\n");
    assert.equal(verifyReleaseReady(fixture), "1.23.4");
    const lock = JSON.parse(readFileSync(join(fixture, "package-lock.json")));
    lock.version = "0.0.1";
    writeFileSync(join(fixture, "package-lock.json"), JSON.stringify(lock));
    assert.throws(() => verifyVersions(fixture), /Version mismatch/);
    assert.throws(() => setVersion("v1.2.3", fixture), /Expected stable/);
  } finally {
    assert.ok(fixture.startsWith(join(tmpdir(), "routedeck-version-test-")));
    rmSync(fixture, { recursive: true });
  }
});

test("release notes are present and checked before the bundle/tag pipeline", () => {
  assert.equal(verifyReleaseReady(), verifyVersions());
  const workflow = readFileSync(join(root, ".github/workflows/release.yml"), "utf8");
  const sourceJob = workflow.slice(workflow.indexOf("  source:"), workflow.indexOf("  bundle:"));
  assert.match(sourceJob, /node scripts\/version\.mjs --release-ready/);
});
