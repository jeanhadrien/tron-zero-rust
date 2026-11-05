// End-to-end check of accounts, tickets, ranked rooms and result intake against
// the in-memory store. Run with `bun run smoke`; exits non-zero on a failure.
process.env.RANKED_KEYS = 'k1:s3cret:1,k2:other:0';
const { default: app, stopEviction } = await import('../src/main');
const server = app.listen(0);
const base = `http://127.0.0.1:${(server.address() as { port: number }).port}`;

const call = async (method: string, path: string, body?: unknown, auth?: string) => {
  const res = await fetch(base + path, {
    method,
    headers: { 'content-type': 'application/json', ...(auth ? { authorization: `Bearer ${auth}` } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return { status: res.status, json: await res.json() };
};
const check = (name: string, ok: boolean, extra?: unknown) => {
  console.log(ok ? 'ok  ' : 'FAIL', name, ok ? '' : JSON.stringify(extra));
  if (!ok) process.exitCode = 1;
};

const a = (await call('POST', '/api/accounts', { displayName: 'Alice' })).json;
const b = (await call('POST', '/api/accounts', { displayName: 'Bob' })).json;

const bad = await call('POST', '/api/rooms', { host: 'h', port: 1, displayName: 'x', maxPlayers: 8, rankedKey: 'k1.wrong' });
check('bad ranked key rejected', bad.status === 403, bad);

const junk = await call('POST', '/api/rooms', { host: 'h', port: 1, displayName: 'x', maxPlayers: 8, identityToken: 'not.a.jwt' });
check('unusable identity token rejected', junk.status === 403, junk);
const both = await call('POST', '/api/rooms', { host: 'h', port: 1, displayName: 'x', maxPlayers: 8, rankedKey: 'k1.s3cret', identityToken: 'a.b.c' });
check('key and token together rejected', both.status === 400, both);
const plain = (await call('POST', '/api/rooms', { host: 'hp', port: 9, displayName: 'plain', maxPlayers: 8 })).json;
check('plain room states its tier', plain.ranked === false && plain.official === false, plain);

const room = (await call('POST', '/api/rooms', { host: 'h', port: 1, displayName: 'official', maxPlayers: 8, rankedKey: 'k1.s3cret' })).json;
check('ranked register', room.ranked && room.official && room.keyId === 'k1' && room.publicKeys.length === 1, room);

const community = (await call('POST', '/api/rooms', { host: 'h2', port: 2, displayName: 'community', maxPlayers: 8, rankedKey: 'k2.other' })).json;

const ta = (await call('POST', '/api/tickets', { roomId: room.id }, a.credential)).json.ticket;
const tb = (await call('POST', '/api/tickets', { roomId: room.id }, b.credential)).json.ticket;
check('tickets issued', !!ta && !!tb);
check('ticket needs credential', (await call('POST', '/api/tickets', { roomId: room.id })).status === 401);

const idA = `acct:${a.id}`, idB = `acct:${b.id}`;
const kill = { session: 's1', seq: 1, events: [{ type: 'kill', killer: idA, victim: idB }] };

let r = await call('POST', `/api/rooms/${room.id}/results`, kill, room.token);
check('kill without presence is skipped', r.json.applied === 0 && r.json.skipped === 1, r);

await call('POST', `/api/rooms/${room.id}/heartbeat`, { playerCount: 2, roster: { humans: [a.id, b.id], tickets: [ta, tb] } }, room.token);
r = await call('POST', `/api/rooms/${room.id}/results`, { ...kill, seq: 2 }, room.token);
check('kill applied after presence proof', r.json.applied === 1, r);

r = await call('POST', `/api/rooms/${room.id}/results`, { ...kill, seq: 2 }, room.token);
check('duplicate seq is idempotent', r.json.duplicate === true, r);

const meA = (await call('GET', '/api/me', undefined, a.credential)).json;
const meB = (await call('GET', '/api/me', undefined, b.credential)).json;
check('winner +8, loser -8', meA.rating === 1508 && meB.rating === 1492, { meA, meB });

r = await call('POST', `/api/rooms/${room.id}/results`, { session: 's1', seq: 3, events: [{ type: 'kill', killer: 'bot:trapper:2500', victim: idA }, { type: 'suicide', victim: 'bot:duelist:2250', alive: [idA] }] }, room.token);
check('official room moves bots', r.json.applied === 2, r);

r = await call('POST', `/api/rooms/${community.id}/results`, { session: 's9', seq: 1, events: [{ type: 'kill', killer: 'bot:trapper:2500', victim: idA }] }, community.token);
check('community room cannot move bots', r.json.applied === 0 && r.json.skipped === 1, r);

r = await call('POST', `/api/rooms/${community.id}/results`, { session: 's9', seq: 2, events: [{ type: 'kill', killer: idA, victim: idB }] }, community.token);
check('community cannot use players it never admitted', r.json.applied === 0, r);

r = await call('POST', `/api/rooms/${room.id}/results`, kill, 'wrong-token');
check('results need room token', r.status === 401, r);

r = await call('POST', `/api/rooms/${room.id}/results`, { session: 's1', seq: 9, events: [{ type: 'kill', killer: idA, victim: idA }] }, room.token);
check('self kill skipped', r.json.applied === 0, r);

// Placement: an even match beats an empty server; exclusions and community rooms are respected.
const c = (await call('POST', '/api/accounts', { displayName: 'Carol' })).json;
const empty = (await call('POST', '/api/rooms', { host: 'h3', port: 3, displayName: 'official-empty', maxPlayers: 8, rankedKey: 'k1.s3cret' })).json;
check('play needs credential', (await call('POST', '/api/play', {})).status === 401);
r = await call('POST', '/api/play', {}, c.credential);
check('placed where players are an even match', r.json.room?.id === room.id && !!r.json.ticket, r);
r = await call('POST', '/api/play', { exclude: [room.id] }, c.credential);
check('exclude moves to the next room', r.json.room?.id === empty.id, r);
r = await call('POST', '/api/play', { exclude: [room.id, empty.id] }, c.credential);
check('community rooms are never placed into', r.status === 404, r);

stopEviction();
server.close();
process.exit(process.exitCode ?? 0);
