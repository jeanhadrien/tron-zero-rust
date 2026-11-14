//! The territory race: who gets to each cell of the arena first.
//!
//! Every rider rides a predicted line (its maneuver, then straight on) and
//! spreads over the coarse grid sideways from each point of that line at the
//! tick it passes it, at its own pace. A cell goes to the first rider to reach
//! it. Spreading stops at what is closed to a rider when it arrives: walls,
//! trails that have not yet faded, and the lines of the riders (laid behind
//! them as they ride): a rider that reaches a cell before it was laid may
//! cross it, one that arrives after may not. Every rider is treated alike, so
//! speed, heading and head start all count, and the cell count per rider says
//! who owns the arena and how little room is left to one that is shut in.

use glam::Vec2;
use shared::ARENA_WIDTH;

use crate::space::{CELL, COARSE, M, Space};

/// Cells of the race grid.
const CELLS: usize = M * M;
const CELL_C: f32 = CELL * COARSE as f32;
/// Most riders in one race; the rest of the field is ignored.
pub const MAX_RACERS: usize = 8;
/// Ticks the race looks ahead; later arrivals are dropped.
const MAX_TICKS: usize = 2048;

/// A stretch of a racer's line and the tick it is ridden at each end.
#[derive(Clone, Copy, Debug)]
pub struct Laid {
    /// Index of the racer that rides it: it is closed to the others from then
    /// on, never to the racer itself.
    pub owner: usize,
    pub from: Vec2,
    pub to: Vec2,
    pub tick_from: f32,
    pub tick_to: f32,
}

/// Cells won by each racer.
pub type Cells = [u32; MAX_RACERS];

/// Reusable buffers for [`run`].
#[derive(Clone, Debug)]
pub struct RaceScratch {
    stamp: u32,
    claimed: Vec<u32>,
    laid: Vec<u32>,
    laid_at: Vec<u16>,
    laid_by: Vec<u8>,
    /// Cells to claim, by tick: `cell | racer << 16`.
    buckets: Vec<Vec<u32>>,
}

impl Default for RaceScratch {
    fn default() -> Self {
        Self {
            stamp: 0,
            claimed: vec![0; CELLS],
            laid: vec![0; CELLS],
            laid_at: vec![0; CELLS],
            laid_by: vec![0; CELLS],
            buckets: vec![Vec::new(); MAX_TICKS],
        }
    }
}

fn cell_of(p: Vec2) -> usize {
    let local = (p + Vec2::splat(ARENA_WIDTH * 0.5)) / CELL_C;
    let x = (local.x.max(0.0) as usize).min(M - 1);
    let y = (local.y.max(0.0) as usize).min(M - 1);
    y * M + x
}

/// The in-bounds 4-neighbours of a coarse cell.
fn around(cell: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (cell % M, cell / M);
    [
        (x > 0).then(|| cell - 1),
        (x + 1 < M).then(|| cell + 1),
        (y > 0).then(|| cell - M),
        (y + 1 < M).then(|| cell + M),
    ]
    .into_iter()
    .flatten()
}

/// The cells a line passes through, with the tick (clamped to the race) it
/// passes each, sampled twice per cell.
fn samples(leg: &Laid, mut each: impl FnMut(usize, u16)) {
    let length = leg.from.distance(leg.to);
    let count = ((length / (CELL_C * 0.5)).ceil() as usize).max(1);
    for i in 0..=count {
        let t = i as f32 / count as f32;
        let tick =
            (leg.tick_from + (leg.tick_to - leg.tick_from) * t).clamp(0.0, MAX_TICKS as f32 - 1.0);
        each(cell_of(leg.from.lerp(leg.to, t)), tick as u16);
    }
}

/// Run the race over `space`: `costs[r]` is the ticks racer `r` needs to cross
/// a coarse cell, `lines` the lines ridden (each leg tagged with its racer).
/// Returns the cells each racer reached first; on a tie the lower index wins.
pub fn run(space: &Space, scratch: &mut RaceScratch, costs: &[u16], lines: &[Laid]) -> Cells {
    debug_assert!(costs.len() <= MAX_RACERS);
    scratch.stamp += 1;
    let stamp = scratch.stamp;
    let free_at = space.coarse();
    let mut pending = 0usize;
    // Lay every line first (its cells close behind the rider), then seed the
    // claims, so the order of the lines never matters to the result.
    for leg in lines {
        samples(leg, |cell, tick| {
            if scratch.laid[cell] != stamp || tick < scratch.laid_at[cell] {
                scratch.laid[cell] = stamp;
                scratch.laid_at[cell] = tick;
                scratch.laid_by[cell] = leg.owner as u8;
            }
        });
    }
    for bucket in &mut scratch.buckets {
        bucket.clear();
    }
    for leg in lines {
        samples(leg, |cell, tick| {
            scratch.buckets[usize::from(tick)].push(cell as u32 | (leg.owner as u32) << 16);
            pending += 1;
        });
    }
    let mut cells = [0u32; MAX_RACERS];
    let mut tick = 0;
    while pending > 0 && tick < MAX_TICKS {
        let batch = std::mem::take(&mut scratch.buckets[tick]);
        pending -= batch.len();
        for &entry in &batch {
            let cell = (entry & 0xffff) as usize;
            let r = (entry >> 16) as usize;
            if scratch.claimed[cell] == stamp {
                continue;
            }
            scratch.claimed[cell] = stamp;
            cells[r] += 1;
            let arrive = tick + usize::from(costs[r]);
            if arrive >= MAX_TICKS {
                continue;
            }
            for next in around(cell) {
                if scratch.claimed[next] == stamp || usize::from(free_at[next]) > arrive {
                    continue;
                }
                let closed = scratch.laid[next] == stamp
                    && (usize::from(scratch.laid_by[next]) == r
                        || arrive >= usize::from(scratch.laid_at[next]));
                if !closed {
                    scratch.buckets[arrive].push(next as u32 | (r as u32) << 16);
                    pending += 1;
                }
            }
        }
        let mut batch = batch;
        batch.clear();
        scratch.buckets[tick] = batch;
        tick += 1;
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walls(half: f32) -> Vec<[f32; 4]> {
        vec![
            [-half, -half, half, -half],
            [half, -half, half, half],
            [half, half, -half, half],
            [-half, half, -half, -half],
        ]
    }

    /// A rider standing still at `(x, y)`: a line of no length.
    fn stand(owner: usize, x: f32, y: f32) -> Laid {
        let at = Vec2::new(x, y);
        Laid {
            owner,
            from: at,
            to: at,
            tick_from: 0.0,
            tick_to: 0.0,
        }
    }

    #[test]
    fn the_nearer_rider_owns_more_of_the_room() {
        let space = Space::build(&walls(400.0), &[]);
        let mut scratch = RaceScratch::default();
        let lines = [stand(0, -300.0, 0.0), stand(1, 100.0, 0.0)];
        let cells = run(&space, &mut scratch, &[13, 13], &lines);
        assert!(cells[1] > cells[0], "{cells:?}");
        assert!(cells[0] > 0);
    }

    #[test]
    fn a_line_walls_off_riders_that_arrive_late() {
        let space = Space::build(&walls(400.0), &[]);
        let mut scratch = RaceScratch::default();
        let foe = stand(1, 300.0, 0.0);
        let open = run(
            &space,
            &mut scratch,
            &[13, 13],
            &[stand(0, 0.0, -300.0), foe],
        );
        // A line from the bot's side across to the north wall, ridden at once,
        // seals the foe in the east half.
        let line = Laid {
            owner: 0,
            from: Vec2::new(0.0, -300.0),
            to: Vec2::new(0.0, 400.0),
            tick_from: 0.0,
            tick_to: 1.0,
        };
        let cut = run(&space, &mut scratch, &[13, 13], &[line, foe]);
        assert!(cut[1] < open[1], "{cut:?} vs {open:?}");
    }
}
