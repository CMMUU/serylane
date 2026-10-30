import { TARGETS, HK_FILES, filenames, boundedText, ReleaseError } from './releases.ts';
import type { Fetcher, Target, Asset } from './releases.ts';
export const CENTER = 'https://downloads.cmmuu.com';
export const CENTER_API = `${CENTER}/api/projects/serylane/releases/latest`;
export const CENTER_PROJECT = `${CENTER}/projects/serylane`;
type CenterAsset = Asset & { fileUrl: string; channel: 'hk'; hkAvailable: boolean; domesticAvailable: boolean; giteeAvailable: false };
type CenterCatalog = { version: string; checkedAt: string; assets: Record<Target, CenterAsset> };
function ensure(v: unknown): asserts v { if (!v) throw new ReleaseError('CENTER_METADATA_INVALID'); }
function record(v: unknown): Record<string, unknown> { ensure(v && typeof v === 'object' && !Array.isArray(v)); return v as Record<string, unknown>; }
export async function centerCatalog(fetcher: Fetcher): Promise<CenterCatalog> {
  const response = await fetcher(CENTER_API, {
    method: 'GET', redirect: 'manual', cache: 'no-store', signal: AbortSignal.timeout(8000),
    headers: { Accept: 'application/json', 'Cache-Control': 'no-cache' },
  });
  const data = record(JSON.parse(await boundedText(response)));
  ensure(data.schemaVersion === 1 && data.project === 'serylane' && typeof data.version === 'string'
    && /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(data.version)
    && typeof data.checkedAt === 'string' && Math.abs(Date.now() - Date.parse(data.checkedAt)) <= 60_000);
  const rows = record(data.assets);
  ensure(Object.keys(rows).length === TARGETS.length);
  const names = filenames(data.version);
  const assets = {} as Record<Target, CenterAsset>;
  for (const target of TARGETS) {
    const row = record(rows[target]);
    ensure(row.filename === names[target] && typeof row.size === 'number' && Number.isSafeInteger(row.size) && row.size > 0 && row.size <= 2 * 1024 ** 3
      && typeof row.sha256 === 'string' && /^[a-f0-9]{64}$/.test(row.sha256) && row.channel === 'hk');
    const expected = `${HK_FILES}/${data.version}/${row.filename}`;
    ensure(row.fileUrl === expected && row.downloadUrl === `${CENTER}/download/serylane/latest/${target}`
      && row.hkAvailable === true && row.domesticAvailable === true && row.giteeAvailable === false);
    assets[target] = { filename: names[target], size: row.size, sha256: row.sha256, fileUrl: expected, channel: row.channel,
      hkAvailable: true, domesticAvailable: true, giteeAvailable: false };
  }
  return { version: data.version, checkedAt: data.checkedAt, assets };
}

/** Validate the selected immutable archive without downloading installer bodies.
 * Redirects are deliberately rejected, even to another center path: the exact
 * version, architecture and source must remain the identity just verified.
 */
export async function verifyCenterAsset(fetcher: Fetcher, asset: CenterAsset): Promise<void> {
  const reply = await fetcher(asset.fileUrl, {
    method: 'HEAD', redirect: 'manual', cache: 'no-store', signal: AbortSignal.timeout(5000),
    headers: { Accept: '*/*', 'Cache-Control': 'no-cache' },
  });
  await reply.body?.cancel();
  const size = Number(reply.headers.get('content-length'));
  const type = reply.headers.get('content-type') ?? '';
  if (reply.status !== 200 || size !== asset.size || !type || /text\/|json|html/i.test(type)
    || reply.headers.has('location') || reply.redirected) throw new ReleaseError('CENTER_PACKAGE_UNAVAILABLE');
}
