# Matchmaking and Elo — core design

Status: implemented in `server-manager/src/rating/` (rules, constants in
`config.ts`), `results.ts` (result intake and trust checks) and `placement.ts`;
the game server reports eliminations from `crates/server/src/ranked.rs`. Still
open: the community-server question, attribution beyond trail owner, and the
validation run. This spec describes the **algorithm and the rules**: what is
rated, how ratings move, who may report results, where players are placed.

## Goals

- One **individual rating per player**, chess-style Elo, meant to stay
  comparable across modes (arena today; 2v2 / 3v3 later feed the same number).
- The rating **updates continuously**, on every elimination, not at match end.
- Humans and bots share **one rating pool**; a bot's rating is measured, not
  assigned.
- A **matchmaker** owns identities and ratings and decides where players are
  placed. Game servers are **not trusted**: the matchmaker accepts or rejects
  what they report.
- A **leaderboard** (rank 1, 2, 3, ...) derived from the ratings.
- **Nothing but ratings is kept.** No match history, no event log.

## Identity

- A player is an **anonymous account**, created on first launch. It is the
  identity the rating belongs to; losing it loses the rating.
- A display name is chosen by the player and is not unique; the account id is
  the identity.
- Servers learn who a player is only through the matchmaker, never from what
  the client claims.
- A real login can replace anonymous accounts later without changing the
  account's identity.

## Rating model

Standard Elo, chess scale.

- Start rating **1500**. Expected score `E(a, b) = 1 / (1 + 10^((Rb - Ra) / 400))`.
- The unit of play is the **elimination**. Each one is decided and applied
  immediately.
- `K` is per rider. Humans: **40** for the first 20 rated events, **16**
  afterwards (deliberately high for the MVP, so a little play moves far). Bots:
  **16** for the first 100 events, **6** afterwards, so they stay a stable
  reference. All are tunable (see *Validation*).
- Bots and humans use the same formulas.
- The rating gap matters through `E`: beating a stronger player gains more,
  losing to a weaker one costs more. A server never restricts who can join
  based on skill.

### Elimination outcomes

Let `K_x` be a player's `K`.

| Event | Rule |
| --- | --- |
| **Kill**: the victim dies on the trail of another rider A | `S_A = 1`, `S_victim = 0`; each side moves by `K * (S - E)` against the other |
| **Mutual kill**: A and B each die on the other's trail within a few ticks of each other | `S = 0.5` for both |
| **Suicide**: the victim dies on an arena wall or its own trail | see below |

A bot facing another instance of the **same bot id** produces no change (equal
ratings, no information).

**Suicide.** There is no opponent, so the victim is compared against the
*field*: `F` = mean rating of the other riders alive at that moment.

- `loss = 0.33 * K_victim * E(victim, F)`. The 0.33 reflects that a suicide is
  weaker evidence than a loss to an opponent.
- `E` grows with the victim's rating, so **a high-rated player loses more than a
  low-rated one** for the same mistake. This is intended.
- The `loss` is shared equally among the riders alive, which keeps the exchange
  roughly zero-sum and gives surviving a small reward.
- If nobody else is alive, nothing changes.

### Attribution (v1)

**Trail owner only.** The rider who owns the trail the victim hit gets the
kill; a wall or the victim's own trail is a suicide.

A later version may add shared credit for riders who were grinding or closing in
during the moments before the death (the "how much did each player contribute"
question). The rules above do not change if it is added.

### Bots

- A bot is its personality plus a set of named policy knobs (reaction,
  look-ahead, mistake rate, aggression, caution, ...). Its rating identity is
  `bot:<personality>:<knob fingerprint>`: **one shared rating per identity**,
  however many instances run, and a retuned knob is a new bot with a new rating.
- Bots carry no declared skill. A new bot identity starts at the same rating as
  a newcomer (1500); the pool measures where it really belongs. Bots are chosen
  for a lobby by measured rating.
- A bot's display name is drawn at random from a list and is unique among the
  riders in the room; it carries no information about the build.
- Only results from **official** servers move bot ratings.

## Trust

A server reports eliminations (who died, who owned the trail, who was alive).
The matchmaker accepts a report only if it is consistent:

- the server is registered as **ranked** (a server needs an operator-issued key
  to report at all);
- every human named is currently connected to **that server** through the
  matchmaker;
- the numbers are plausible: nobody kills themselves as a "kill", no impossible
  number of deaths per player or per server in a short time, no repeated kills
  between the same pair beyond a cap;
- bot results come from official servers only.

This catches mistakes and crude abuse, not a determined host; stronger
protection (verifying results by replaying the game) is out of scope.

**Open question — community servers.** A community server may be ranked, but
it cannot be trusted to move the global ratings. Either only official servers'
results count in v1 (a community "ranked" flag then means little), or
community results count with the same checks and the risk that a key holder
inflates a friend. To decide.

## Placement

Passive and instant: there is no queue. When a player asks to play, the
matchmaker picks a server immediately. Only **official ranked servers** are
candidates; community servers are reached only by browsing.

For each candidate with a free slot:

1. Take its **roster**: humans and bots currently on it, plus **reservations**
   (players placed there who have not connected yet, counted at their rating).
2. Compute the **unfairness** for this player:
   `u = mean over roster of | E(player, rider) - 0.5 |`.
   It is 0 when everyone is an even match and approaches 0.5 against much
   stronger or weaker riders. Because it looks at each rider rather than the
   mean rating, a 1500 player facing a 1000 and a 2000 scores badly where two
   1500s would score perfectly.
3. Add a small **population term** that prefers servers closer to a target
   number of riders, so players concentrate instead of scattering over empty
   servers.
4. Pick the lowest total.

Rules around it:

- **Always place someone.** The best available server is used even when it is a
  poor match; a thin population is handled by concentrating players, not by
  refusing or waiting.
- **Reservations.** A placed player counts in the chosen server's roster at
  once and until they connect or a short time passes. This stops simultaneous
  requests from piling onto the same server.
- **Failure.** If the chosen server turns out full or unreachable, the next best
  is tried.
- **Placement is evaluated only when a player asks to play.** Nobody is moved
  during a session.
- A server whose roster is empty has no unfairness data; it gets a neutral
  score so it is used when nothing better exists.
- Newcomers start at 1500 and are placed like anyone else; there is no special
  handling of provisional ratings.

## Leaderboard

- Ordered by rating, descending, including bots.
- A player appears once they have enough rated events (3 for the MVP, to tune), so
  provisional ratings never show.
- A player's rank is the number of eligible players above them plus one.
- v1 has no decay and no inactivity filter.

## Validation

Simulate bots of known `skill` against each other with the same rating rules and
check that (1) the resulting order matches the skill order and (2) ratings
converge at the chosen `K`. Use it to tune `K`, the ranked threshold, the
suicide factor and the bot remap, and to see how large a share suicides take in
the rating moves.

## Out of scope (v1)

- **Teams** (2v2, 3v3): the game has no team concept yet (no allied trails, no
  friendly fire). The per-elimination model carries over; only lobby balancing
  needs a team strength.
- Event history, replays, and replay-based verification of results.
- Attribution beyond trail owner; smurf and cheat protection; real login;
  rating decay and seasons.
- Matchmaking on community servers; choosing bot fill by rating.
