// Fixed download-center archive only; never accept visitor-supplied upstreams.
export const HK_FILES = 'https://files.cmmuu.com/releases/serylane';
export const TARGETS = ['windows-x64', 'windows-arm64', 'macos-x64', 'macos-arm64', 'linux-x64', 'linux-arm64'] as const;
export type Target = typeof TARGETS[number];
export type Fetcher = (url: string, init: RequestInit) => Promise<Response>;
export type Asset = { filename: string; size: number; sha256: string };
const LIMIT = 256 * 1024;
export class ReleaseError extends Error {
  code: string;
  constructor(code: string) { super(code); this.code = code; }
}
function requireValid(value: unknown): asserts value {
  if (!value) throw new ReleaseError('INVALID_RELEASE_METADATA');
}
async function dispose(response: Response) { await response.body?.cancel(); }
export async function boundedText(response: Response): Promise<string> {
  if (response.status !== 200) {
    await dispose(response);
    throw new ReleaseError(`METADATA_HTTP_${response.status}`);
  }
  if (Number(response.headers.get('content-length')) > LIMIT) {
    await dispose(response);
    throw new ReleaseError('RELEASE_METADATA_TOO_LARGE');
  }
  requireValid(response.body);
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      length += value.length;
      if (length > LIMIT) throw new ReleaseError('RELEASE_METADATA_TOO_LARGE');
      chunks.push(value);
    }
  } finally { await reader.cancel(); reader.releaseLock(); }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
  return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
}
export function filenames(version: string): Record<Target, string> {
  const [major, minor, patch] = version.slice(1).split('.').map(Number);
  const brand = major === 0 && (minor < 7 || (minor === 7 && patch < 7)) ? 'RouteDeck' : 'Serylane';
  const prefix = `${brand}_${version.slice(1)}`;
  return {
    'windows-x64': `${prefix}_x64-setup.exe`, 'windows-arm64': `${prefix}_arm64-setup.exe`,
    'macos-x64': `${prefix}_x64.dmg`, 'macos-arm64': `${prefix}_aarch64.dmg`,
    'linux-x64': `${prefix}_amd64.AppImage`, 'linux-arm64': `${prefix}_aarch64.AppImage`,
  };
}
