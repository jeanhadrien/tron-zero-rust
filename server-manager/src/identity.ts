import { createRemoteJWKSet, jwtVerify, type JWTPayload, type JWTVerifyGetKey } from 'jose';
import type { KeyCheck } from './ranked';
import { store } from './store';

// A game server proves who it is with an OIDC ID token minted by the platform it
// runs on (on GCE: the metadata server). The manager checks the signature, that
// the token was minted for this manager, and that the identity is on the allowlist
// (`trustedIdentities`). No shared secret is stored anywhere.
//
// Only Google is configured today; another issuer is one more entry here.

const GOOGLE_ISSUERS = ['https://accounts.google.com', 'accounts.google.com'];
const googleKeys = createRemoteJWKSet(new URL('https://www.googleapis.com/oauth2/v3/certs'));

/** Verification failures that mean "this token is bad", as opposed to "we could not check". */
const REJECTED_CODES = new Set([
  'ERR_JWT_CLAIM_VALIDATION_FAILED',
  'ERR_JWT_EXPIRED',
  'ERR_JWT_INVALID',
  'ERR_JWS_INVALID',
  'ERR_JWS_SIGNATURE_VERIFICATION_FAILED',
  'ERR_JOSE_ALG_NOT_ALLOWED',
  'ERR_JWKS_NO_MATCHING_KEY',
  'ERR_JWKS_MULTIPLE_MATCHING_KEYS',
]);

/** The extra claims Google adds with `format=full` on a Compute Engine VM. */
interface GoogleClaims extends JWTPayload {
  email?: string;
  email_verified?: boolean;
  google?: { compute_engine?: { project_id?: string; instance_id?: string } };
}

export interface IdentityOptions {
  /** Signing keys; defaults to Google's. Tests inject a local set. */
  keys?: JWTVerifyGetKey;
  /** The `aud` a token must carry; defaults to `IDENTITY_AUDIENCE`. */
  audience?: string;
}

/**
 * Validate a platform identity token. `ok: false` with `transient` set means the
 * verification itself failed (e.g. Google's keys were unreachable): the caller
 * should answer 503 so the server retries, not 403.
 */
export async function checkIdentityToken(token: string, options: IdentityOptions = {}): Promise<KeyCheck> {
  const audience = options.audience ?? process.env.IDENTITY_AUDIENCE;
  if (!audience) return { ok: false, reason: 'This manager does not accept identity tokens' };

  let claims: GoogleClaims;
  try {
    ({ payload: claims } = await jwtVerify<GoogleClaims>(token, options.keys ?? googleKeys, {
      issuer: GOOGLE_ISSUERS,
      audience,
      algorithms: ['RS256'],
    }));
  } catch (err) {
    const code = (err as { code?: string }).code ?? '';
    if (REJECTED_CODES.has(code)) return { ok: false, reason: `Identity token rejected (${code})` };
    return { ok: false, reason: 'Could not verify the identity token', transient: true };
  }

  const email = claims.email;
  if (!email || claims.email_verified !== true) return { ok: false, reason: 'Identity token has no verified email' };
  const trusted = await store.trustedIdentity(email);
  if (!trusted || trusted.revoked) return { ok: false, reason: `Identity ${email} is not trusted` };

  // Ticket binding and the result cursor are per key id. Servers sharing one service
  // account must not share them, so a VM is identified by its instance.
  const vm = claims.google?.compute_engine;
  const keyId = vm?.project_id && vm.instance_id ? `gce:${vm.project_id}:${vm.instance_id}` : `sa:${email}`;
  return { ok: true, keyId, official: trusted.official };
}
