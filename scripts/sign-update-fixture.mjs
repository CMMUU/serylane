// Ephemeral test keys only; never touches release secrets or production artifacts.
import { generateKeyPairSync, createHash, sign } from 'node:crypto';
import { readFileSync } from 'node:fs';
const bytes = readFileSync(process.argv[2]);
const { publicKey, privateKey } = generateKeyPairSync('ed25519');
const key = publicKey.export({ type: 'spki', format: 'der' }).subarray(-32);
const keyId = Buffer.from('fixture1');
const sig = sign(null, createHash('blake2b512').update(bytes).digest(), privateKey);
const comment = 'timestamp:1000\tfile:fixture.app.tar.gz';
const global = sign(null, Buffer.concat([sig, Buffer.from(comment)]), privateKey);
const b64 = b => Buffer.from(b).toString('base64');
console.log(JSON.stringify({
  pubkey: b64(`untrusted comment: disposable test key\n${b64(Buffer.concat([Buffer.from('Ed'), keyId, key]))}\n`),
  signature: b64(`untrusted comment: disposable signature\n${b64(Buffer.concat([Buffer.from('ED'), keyId, sig]))}\ntrusted comment: ${comment}\n${b64(global)}\n`)
}));
