import { readFileSync, writeFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

export const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const stable = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const read = (base, path) => readFileSync(resolve(base, path), "utf8");
export function versions(base = root) {
  const packageInfo = JSON.parse(read(base, "package.json"));
  const lock = JSON.parse(read(base, "package-lock.json"));
  const cargo = read(base, "src-tauri/Cargo.toml").match(/\[package\][\s\S]*?\nversion = "([^"]+)"/);
  const cargoLock = read(base, "src-tauri/Cargo.lock").match(/\[\[package\]\]\r?\nname = "serylane"\r?\nversion = "([^"]+)"/);
  return { "package.json": packageInfo.version, "package-lock.json": lock.version,
    "package-lock root": lock.packages?.[""]?.version, "Cargo.toml": cargo?.[1], "Cargo.lock": cargoLock?.[1],
    "tauri.conf.json": JSON.parse(read(base, "src-tauri/tauri.conf.json")).version };
}
export function verifyVersions(base = root, tag) {
  const found = versions(base);
  const expected = found["package.json"];
  if (!stable.test(expected) || Object.values(found).some((value) => value !== expected)) throw new Error(`Version mismatch: ${JSON.stringify(found)}`);
  if (tag && tag !== `v${expected}`) throw new Error(`Tag ${tag} does not match v${expected}`);
  return expected;
}
export function verifyReleaseReady(base = root) {
  const version = verifyVersions(base);
  const path = `docs/发布说明-v${version}.md`;
  let notes;
  try { notes = read(base, path); } catch { throw new Error(`Reviewed release notes are required: ${path}`); }
  if (!notes.trim()) throw new Error(`Reviewed release notes are required: ${path}`);
  return version;
}
export function setVersion(version, base = root) {
  if (!stable.test(version)) throw new Error("Expected stable X.Y.Z version");
  // Validate every input before writing any of them; no npm/cargo side effects.
  verifyVersions(base);
  const files = new Map();
  for (const path of ["package.json", "package-lock.json", "src-tauri/tauri.conf.json"]) {
    const object = JSON.parse(read(base, path)); object.version = version;
    if (path === "package-lock.json") object.packages[""].version = version;
    files.set(path, `${JSON.stringify(object, null, 2)}\n`);
  }
  files.set("src-tauri/Cargo.toml", read(base, "src-tauri/Cargo.toml").replace(/(\[package\][\s\S]*?\nversion = ")[^"]+("\r?\n)/, `$1${version}$2`));
  files.set("src-tauri/Cargo.lock", read(base, "src-tauri/Cargo.lock").replace(/(\[\[package\]\]\r?\nname = "serylane"\r?\nversion = ")[^"]+("\r?\n)/, `$1${version}$2`));
  for (const [path, content] of files) writeFileSync(resolve(base, path), content);
  return verifyVersions(base);
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args[0] === "--set" && args.length === 2) setVersion(args[1]);
  else if (args[0] === "--release-ready" && args.length === 1) verifyReleaseReady();
  else if (args.length && !(args[0] === "--tag" && args.length === 2)) throw new Error("Usage: node scripts/version.mjs [--set X.Y.Z | --tag vX.Y.Z | --release-ready]");
  console.log(`Serylane ${verifyVersions(root, args[0] === "--tag" ? args[1] : undefined)}: all six version fields agree`);
}
