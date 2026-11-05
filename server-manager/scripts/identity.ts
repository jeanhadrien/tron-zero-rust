// Checks platform identity verification against locally signed tokens (no network).
// Run with `bun scripts/identity.ts`; exits non-zero on a failure.
import { SignJWT, createLocalJWKSet, exportJWK, generateKeyPair } from 'jose';

process.env.TRUSTED_IDENTITIES = 'game@p.iam.gserviceaccount.com:1,community@p.iam.gserviceaccount.com:0';
const { checkIdentityToken } = await import('../src/identity');

const AUDIENCE = 'https://manager.test';
const { publicKey, privateKey } = await generateKeyPair('RS256');
const keys = createLocalJWKSet({ keys: [{ ...(await exportJWK(publicKey)), alg: 'RS256', kid: 'k' }] });
const options = { keys, audience: AUDIENCE };

const mint = (claims: Record<string, unknown>, aud = AUDIENCE, ttl = '1h') =>
  new SignJWT({ email_verified: true, ...claims })
    .setProtectedHeader({ alg: 'RS256', kid: 'k' })
    .setIssuer('https://accounts.google.com')
    .setAudience(aud)
    .setExpirationTime(ttl)
    .sign(privateKey);

const check = (name: string, ok: boolean, extra?: unknown) => {
  console.log(ok ? 'ok  ' : 'FAIL', name, ok ? '' : JSON.stringify(extra));
  if (!ok) process.exitCode = 1;
};

const vm = { compute_engine: { project_id: 'p', instance_id: '42' } };
let r = await checkIdentityToken(await mint({ email: 'game@p.iam.gserviceaccount.com', google: vm }), options);
check('trusted official account', r.ok && r.official && r.keyId === 'gce:p:42', r);

r = await checkIdentityToken(await mint({ email: 'community@p.iam.gserviceaccount.com' }), options);
check('community account, no VM claims', r.ok && !r.official && r.keyId === 'sa:community@p.iam.gserviceaccount.com', r);

r = await checkIdentityToken(await mint({ email: 'stranger@p.iam.gserviceaccount.com' }), options);
check('unlisted account refused', !r.ok && !r.transient, r);

r = await checkIdentityToken(await mint({ email: 'game@p.iam.gserviceaccount.com' }, 'https://other.test'), options);
check('wrong audience refused', !r.ok && !r.transient, r);

r = await checkIdentityToken(await mint({ email: 'game@p.iam.gserviceaccount.com', email_verified: false }), options);
check('unverified email refused', !r.ok && !r.transient, r);

r = await checkIdentityToken(await mint({ email: 'game@p.iam.gserviceaccount.com' }, AUDIENCE, '-1m'), options);
check('expired token refused', !r.ok && !r.transient, r);

const other = await generateKeyPair('RS256');
const forged = await new SignJWT({ email: 'game@p.iam.gserviceaccount.com', email_verified: true })
  .setProtectedHeader({ alg: 'RS256', kid: 'k' })
  .setIssuer('https://accounts.google.com')
  .setAudience(AUDIENCE)
  .setExpirationTime('1h')
  .sign(other.privateKey);
r = await checkIdentityToken(forged, options);
check('forged signature refused', !r.ok && !r.transient, r);

r = await checkIdentityToken('not.a.jwt', options);
check('garbage refused', !r.ok && !r.transient, r);

const down = async () => { throw new Error('network down'); };
r = await checkIdentityToken(await mint({ email: 'game@p.iam.gserviceaccount.com' }), { keys: down as never, audience: AUDIENCE });
check('unreachable keys are transient', !r.ok && r.transient === true, r);

r = await checkIdentityToken(await mint({ email: 'game@p.iam.gserviceaccount.com' }), { keys, audience: '' });
check('disabled without an audience', !r.ok && !r.transient, r);
