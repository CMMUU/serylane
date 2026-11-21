import { readFileSync, writeFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const read = path => readFileSync(resolve(root, path), 'utf8');
const capture = (source, pattern, name) => {
  const found = source.match(pattern)?.[1];
  if (found === undefined) throw new Error(`Product source contract changed: ${name}; review docs and generator`);
  return found;
};

export function productTable(language = 'zh') {
  const version = JSON.parse(read('package.json')).version;
  const capabilities = read('src-tauri/src/capabilities.rs');
  const policy = read('src-tauri/src/openai_policy.rs');
  const updates = read('src-tauri/src/app_update.rs');
  const sources = capture(updates, /pub enum UpdateSource \{([\s\S]*?)\n\}/, 'UpdateSource');
  const variants = sources.split('\n').map(line => line.trim()).filter(line => /^[A-Z]\w*,$/.test(line));
  const expected = ['Github', 'GithubAccelerator', 'GithubProxy', 'Hk'];
  const order = capture(updates, /const SOURCE_ORDER:[^=]+=\s*\[([\s\S]*?)\];/, 'update source priority')
    .split(',').map(item => item.trim().replace('UpdateSource::', '')).filter(Boolean);
  if (variants.map(item => item.slice(0, -1)).join(',') !== expected.join(',') || order.join(',') !== expected.join(',')) {
    throw new Error('Update source changed; review the documented priority');
  }
  const [enabledLabel, disabledLabel] = language === 'en'
    ? ['Enabled', 'Temporarily disabled']
    : ['已启用', '暂时停用'];
  const enabled = name => capture(capabilities, new RegExp(`pub const ${name}: bool = (true|false);`), name) === 'true'
    ? enabledLabel
    : disabledLabel;
  const min = capture(policy, /const MIN_HEALTHY_NODES: usize = (\d+);/, 'minimum AI nodes');
  const max = capture(policy, /const SELECTED_NODES: usize = (\d+);/, 'maximum AI nodes');
  if (language === 'en') {
    return `| Source fact | Current value |
| --- | --- |
| Source version | ${version} (see Releases for published builds) |
| Website installers | Download center |
| In-app updates | Direct GitHub → GitHub accelerator → GitHub through the running Serylane proxy → Download center |
| Eligible AI nodes | ${min}–${max}. A single node has no fallback. |
| Per-app proxy | ${enabled('PROGRAM_PROXY')} |
| Enhanced failover | ${enabled('ENHANCED_STABILITY')} |
| New Codex integration | ${enabled('CODEX_ATTACH')} |`;
  }
  return `| 源码事实 | 当前值 |\n| --- | --- |\n| 源码版本 | ${version}（不代表已经发行） |\n| 官网安装包 | 下载中心 |\n| 应用内更新 | GitHub 直连 → GitHub 加速代理 → 经 Serylane 本地代理访问 GitHub → 下载中心 |\n| AI 合格节点范围 | ${min}–${max} 个；单节点没有备用 |\n| 程序代理 | ${enabled('PROGRAM_PROXY')} |\n| 增强稳定灾备 | ${enabled('ENHANCED_STABILITY')} |\n| 新增 Codex 接入 | ${enabled('CODEX_ATTACH')} |`;
}

export function verifyProductDocs(write = false) {
  for (const [path, language] of [['README.md', 'en'], ['docs/当前实现与业务契约.md', 'zh']]) {
    const block = `<!-- product-contract:start -->\n${productTable(language)}\n<!-- product-contract:end -->`;
    const original = read(path).replace(/\r\n/g, '\n');
    const marker = /<!-- product-contract:start -->[\s\S]*?<!-- product-contract:end -->/g;
    if ([...original.matchAll(marker)].length !== 1) throw new Error(`Expected one product contract: ${path}`);
    const expected = original.replace(marker, block);
    if (original !== expected) {
      if (write) writeFileSync(resolve(root, path), expected);
      else throw new Error(`Product docs are stale: ${path}; review source and run node scripts/product-docs.mjs --write`);
    }
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.slice(2).some(arg => arg !== '--write')) throw new Error('Usage: node scripts/product-docs.mjs [--write]');
  verifyProductDocs(process.argv.includes('--write'));
  console.log('Current product documentation matches source facts');
}
