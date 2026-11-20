import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

export const origin = 'https://serylane.cmmuu.com';
export const output = new URL('../.site/', import.meta.url);
export const outputPath = fileURLToPath(output);
const manifest = JSON.parse(await readFile(new URL('../../locales/manifest.json', import.meta.url), 'utf8'));
const routes = {
  'zh-Hans': ['zh', 'zh-CN', 'zh_CN'],
  'zh-Hant': ['zh-hant', 'zh-Hant', 'zh_TW'],
  en: ['', 'en', 'en_US'],
  ja: ['ja', 'ja', 'ja_JP'], ko: ['ko', 'ko', 'ko_KR'],
  de: ['de', 'de', 'de_DE'], fr: ['fr', 'fr', 'fr_FR'],
  es: ['es', 'es', 'es_ES'], 'pt-BR': ['pt-br', 'pt-BR', 'pt_BR'],
  ru: ['ru', 'ru', 'ru_RU'], ar: ['ar', 'ar', 'ar_SA'],
};
assert.deepEqual(new Set(manifest.map(({ id }) => id)), new Set(Object.keys(routes)), 'Provide a website route and translations for each desktop locale');
export const locales = manifest.map(locale => {
  const [prefix, lang, social] = routes[locale.id];
  return { ...locale, prefix: prefix ? '/' + prefix : '', lang, social };
});
export const english = locales.find(({ id }) => id === 'en');
export const pages = {
  home: ['/', 'index.html'],
  docs: ['/docs/', 'docs/index.html'],
  guide: ['/docs/guide/', 'docs/guide/index.html'],
  routing: ['/docs/routing', 'docs/routing.html'],
  verification: ['/docs/verification', 'docs/verification.html'],
  privacy: ['/docs/privacy', 'docs/privacy.html'],
};
export const pathFor = (page, locale) => locale.prefix + pages[page][0];
export const fileFor = (page, locale) => (locale.prefix ? locale.prefix.slice(1) + '/' : '') + pages[page][1];
export const contentRoutes = new Map(locales.flatMap(locale =>
  Object.keys(pages).map(page => [pathFor(page, locale), { page, locale, file: fileFor(page, locale) }])));
export function equivalents(page) {
  return [...locales.map(locale => [locale.lang, pathFor(page, locale)]), ['x-default', pathFor(page, english)]];
}

export const uiKeys = [
  'language', 'skip', 'home', 'nav', 'capabilities', 'docs', 'guide', 'routing', 'verification', 'privacy',
  'download', 'toc', 'related', 'appearance', 'dark', 'light', 'system', 'open', 'close', 'source',
  'pause', 'resume', 'reduced', 'loading', 'latest', 'unavailable', 'checking', 'started', 'failed', 'retry', 'package',
];
export async function readUI(id) {
  const values = (await readFile(new URL('../content/home/' + id + '.ui', import.meta.url), 'utf8')).trim().split(/\r?\n/);
  assert.equal(values.length, uiKeys.length, id + ' UI string count');
  assert.ok(values.every(value => value.trim()), id + ' UI strings cannot be blank');
  const ui = Object.fromEntries(uiKeys.map((key, i) => [key, values[i]]));
  for (const [key, tokens] of Object.entries({ latest: ['version'], checking: ['target'], started: ['version', 'target'], package: ['target'] })) {
    assert.deepEqual([...new Set([...ui[key].matchAll(/\{(\w+)\}/g)].map(match => match[1]))].sort(), [...tokens].sort(), id + ':' + key);
  }
  return ui;
}
