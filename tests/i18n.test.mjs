import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { registerHooks } from "node:module";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { FluentBundle, FluentResource } from "@fluent/bundle";
import ts from "typescript";

const manifest = readFileSync(new URL("../locales/manifest.json", import.meta.url), "utf8");
const languages = JSON.parse(manifest);
const resourceFor = id => readFileSync(new URL(`../locales/${id}.ftl`, import.meta.url), "utf8");
const fixtureUrl = "serylane-test:i18n";

// The existing frontend test suites compile individual TS modules into data
// URLs. Supply the same fixture resources that Vite normally loads from disk.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier === fixtureUrl || specifier === "./i18n" && context.parentURL?.startsWith("data:")) {
      return { url: fixtureUrl, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url !== fixtureUrl) return nextLoad(url, context);
    const raw = Object.fromEntries(languages.map(({ id }) => [`../locales/${id}.ftl`, resourceFor(id)]));
    const source = readFileSync(new URL("../src/i18n.ts", import.meta.url), "utf8")
      .replace('"@fluent/bundle"', JSON.stringify(import.meta.resolve("@fluent/bundle")))
      .replace('import manifest from "../locales/manifest.json?raw";', `const manifest = ${JSON.stringify(manifest)};`)
      .replace(/const raw = import\.meta\.glob<string>\([\s\S]*?\n\}\);/, `const raw = ${JSON.stringify(raw)};`);
    assert.doesNotMatch(source, /import\.meta\.glob/);
    const compiled = ts.transpileModule(source, {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
    }).outputText;
    return { format: "module", source: compiled, shortCircuit: true };
  },
});

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  test("the 11 native language labels and order match the product contract", () => {
    assert.deepEqual(languages.map(({ id, name }) => [id, name]), [
      ["zh-Hans", "简体中文"], ["zh-Hant", "繁體中文"], ["en", "English"], ["ja", "日本語"],
      ["ko", "한국어"], ["de", "Deutsch"], ["fr", "Français"], ["es", "Español"],
      ["pt-BR", "Português (Brasil)"], ["ru", "Русский"], ["ar", "العربية"],
    ]);
    assert.deepEqual(languages.filter(({ direction }) => direction === "rtl").map(({ id }) => id), ["ar"]);
  });

  test("the Windows setup list matches the app's native names and localized controls", () => {
    const installer = readFileSync(new URL("../src-tauri/installer/installer.nsi", import.meta.url), "utf8");
    const options = [...installer.matchAll(/^\s*!insertmacro \$\{Apply\} (\d+) "([^"]+)" "([^"]+)"$/gm)];
    assert.deepEqual(options.map(([, index, id, label]) => [Number(index), id, label]), [
      ...languages.map(({ id, name }, index) => [index, id, name]),
      [11, "system", "$(appLanguageSystem)"],
    ]);
    const installerLanguages = [
      ["en", "English"], ["zh-Hans", "SimpChinese"], ["zh-Hant", "TradChinese"],
      ["ja", "Japanese"], ["ko", "Korean"], ["de", "German"], ["fr", "French"],
      ["es", "SpanishInternational"], ["pt-BR", "PortugueseBR"], ["ru", "Russian"], ["ar", "Arabic"],
    ];
    const config = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
    assert.deepEqual(config.bundle.windows.nsis.languages, installerLanguages.map(([, name]) => name));
    for (const [locale, name] of installerLanguages) {
      const resource = new FluentResource(resourceFor(locale));
      for (const [fluent, nsis] of [
        ["language-heading", "appLanguageHeading"],
        ["language-label", "appLanguageLabel"],
        ["language-system", "appLanguageSystem"],
      ]) {
        const label = resource.body.find(item => item.id === fluent).value;
        assert.ok(installer.includes(`LangString ${nsis} \${LANG_${name.toUpperCase()}} "${label}"`), locale);
      }
    }
  });

  test("all locale files expose matching phrases and named parameters", () => {
    const keys = resource => Object.fromEntries(resource.body.map(({ id, value }) => [
      id, Array.isArray(value) ? value.filter(part => part?.type === "var").map(part => part.name).sort() : [],
    ]));
    const sourceKeys = keys(new FluentResource(resourceFor("zh-Hans")));
    const available = readdirSync(new URL("../locales/", import.meta.url)).filter(name => name.endsWith(".ftl"));
    assert.deepEqual(available.sort(), languages.map(({ id }) => id + ".ftl").sort());
    for (const { id } of languages) {
      const resource = new FluentResource(resourceFor(id));
      const bundle = new FluentBundle(id);
      assert.deepEqual(bundle.addResource(resource), [], id);
      assert.deepEqual(keys(resource), sourceKeys, id);
      for (const [key, names] of Object.entries(sourceKeys)) {
        const errors = [];
        const args = Object.fromEntries(names.map(name => [name, name === "count" ? 3 : "token"]));
        const result = bundle.formatPattern(bundle.getMessage(key).value, args, errors);
        assert.ok(result.trim().length > 0, `${id} ${key}`);
        assert.deepEqual(errors, [], `${id} ${key}`);
      }
    }
  });

  test("switching locale repaints existing bindings and preserves opaque profile names", async () => {
    globalThis.document = { documentElement: {} };
    try {
      const { applyAppLocale, setMessage, setLocalizedText, setPlainText, localeOptions } = await import(fixtureUrl);
      const label = { isConnected: true, textContent: "" };
      const profile = { isConnected: true, textContent: "" };
      const notice = { isConnected: true, textContent: "" };
      setLocalizedText(label, "检查更新");
      setPlainText(profile, "检查更新", "未选择");
      setMessage(notice, "update-card-title", "Serylane v0.7.44 可用", { version: "v0.7.44" });
      applyAppLocale({ preference: "en", language: "en", formatLocale: "en-GB" });
      assert.equal(label.textContent, "Check for updates");
      assert.equal(profile.textContent, "检查更新");
      assert.match(notice.textContent, /v0\.7\.44.*is available/);
      applyAppLocale({ preference: "ar", language: "ar", formatLocale: "ar" });
      assert.equal(label.textContent, "التحقق من وجود تحديثات");
      assert.equal(document.documentElement.dir, "rtl");
      assert.equal([...localeOptions().matchAll(/value="/g)].length, 11);
    } finally {
      delete globalThis.document;
    }
  });
}
