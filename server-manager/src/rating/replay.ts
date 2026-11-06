// Replays a lab elimination log through the rating core and reports the
// measured order and how the ratings converge.
//
//   bun src/rating/replay.ts ratings.jsonl [--checkpoints N]
//
// Produce the log with the lab's batch mode:
//   cargo run --release -p tron-zero-lab -- crates/lab/validation/ratings.toml \
//     --seeds 200 --eliminations ratings.jsonl

import { readFileSync } from 'node:fs';
import { RATING_CONFIG } from './config.js';
import { applyKill, applyMutual, applySuicide, isEligible, type Change, type Rated } from './elo.js';

type Elimination =
  | { type: 'kill'; victim: string; killer: string }
  | { type: 'mutual'; a: string; b: string }
  | { type: 'suicide'; victim: string; alive: string[] };

const args = process.argv.slice(2);
const path = args.find((a) => !a.startsWith('--'));
if (!path) {
  console.error('usage: bun src/rating/replay.ts <eliminations.jsonl> [--checkpoints N]');
  process.exit(1);
}
const checkpointFlag = args.indexOf('--checkpoints');
const checkpoints = checkpointFlag >= 0 ? Number(args[checkpointFlag + 1]) : 5;

const ratings = new Map<string, Rated>();
const get = (id: string): Rated => {
  let r = ratings.get(id);
  if (!r) {
    r = { id, rating: RATING_CONFIG.startRating, ratedEvents: 0 };
    ratings.set(id, r);
  }
  return r;
};

const events: Elimination[] = readFileSync(path, 'utf8')
  .split('\n')
  .filter(Boolean)
  .map((line) => JSON.parse(line));

/** Apply every change of one elimination, all computed from pre-event ratings. */
function apply(changes: Change[]): void {
  for (const c of changes) {
    const r = get(c.id);
    r.rating += c.delta;
    if (c.countsAsEvent) r.ratedEvents += 1;
  }
}

const moved = { kill: 0, mutual: 0, suicide: 0 };
const snapshots: { at: number; ratings: [string, number][] }[] = [];
const every = Math.max(1, Math.floor(events.length / Math.max(1, checkpoints)));

events.forEach((e, i) => {
  let changes: Change[];
  switch (e.type) {
    case 'kill':
      changes = applyKill(get(e.killer), get(e.victim));
      break;
    case 'mutual':
      changes = applyMutual(get(e.a), get(e.b));
      break;
    case 'suicide':
      changes = applySuicide(get(e.victim), e.alive.map(get));
      break;
  }
  moved[e.type] += changes.reduce((sum, c) => sum + Math.abs(c.delta), 0);
  apply(changes);
  if ((i + 1) % every === 0 || i + 1 === events.length) {
    snapshots.push({ at: i + 1, ratings: [...ratings].map(([id, r]) => [id, Math.round(r.rating)]) });
  }
});


const rows = [...ratings.values()].sort((a, b) => b.rating - a.rating);


const total = moved.kill + moved.mutual + moved.suicide;
const count = (t: Elimination['type']) => events.filter((e) => e.type === t).length;

console.log(`${events.length} eliminations: ${count('kill')} kill, ${count('mutual')} mutual, ${count('suicide')} suicide`);
console.log(
  `share of rating movement: kill ${pct(moved.kill, total)}, mutual ${pct(moved.mutual, total)}, suicide ${pct(moved.suicide, total)}`,
);
console.log('\nrank  rating  events  eligible  id');
rows.forEach((r, i) =>
  console.log(
    `${String(i + 1).padStart(4)}  ${r.rating.toFixed(0).padStart(6)}  ${String(r.ratedEvents).padStart(6)}  ${String(isEligible(r.ratedEvents)).padStart(8)}  ${r.id}`,
  ),
);

console.log('\nconvergence (rating by checkpoint):');
for (const s of snapshots) {
  const line = s.ratings
    .sort((a, b) => a[0].localeCompare(b[0]))
    .map(([id, rating]) => `${id.split(':').slice(1).join(':')}=${rating}`)
    .join('  ');
  console.log(`  @${String(s.at).padStart(6)}  ${line}`);
}

function pct(part: number, whole: number): string {
  return whole === 0 ? 'n/a' : `${((100 * part) / whole).toFixed(0)}%`;
}
