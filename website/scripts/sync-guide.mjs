import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { locales } from './site-locales.mjs';

const [mode, checkout, ...extra] = process.argv.slice(2);
assert.ok(checkout && ['--check', '--write'].includes(mode) && !extra.length,
  'Usage: node scripts/sync-guide.mjs --check|--write /absolute/path/to/project-docs');
const source = join(resolve(checkout), 'serylane', '官网使用指南');
const names = {
  'zh-Hans': '简体中文', 'zh-Hant': '繁體中文', en: '英文', ja: '日文', ko: '韩文',
  de: '德文', fr: '法文', es: '西班牙文', 'pt-BR': '葡萄牙文（巴西）', ru: '俄文', ar: '阿拉伯文',
};
assert.deepEqual(new Set(locales.map(item => item.id)), new Set(Object.keys(names)));
const pairs = await Promise.all(locales.map(async ({ id }) => ({
  id,
  bytes: await readFile(join(source, names[id] + '.md')),
  target: new URL('../content/guide/' + id + '.md', import.meta.url),
})));
for (const { id, bytes, target } of pairs) {
  if (mode === '--write') await writeFile(target, bytes);
  else assert.ok(bytes.equals(await readFile(target)), id + ': site guide differs from project-docs');
}
console.log((mode === '--write' ? 'Synced ' : 'Verified ') + pairs.length + ' guides from project-docs');
