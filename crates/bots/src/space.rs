//! Room to ride: a fine occupancy grid and a capped flood fill over it.
//!
//! Cells are about three ticks of riding wide, so the corridors this game
//! produces (a few lanes of trail apart) are visible. Each cell remembers when
//! it stops being blocked: arena walls never, a trail when its tail has passed
//! (a trail is capped at `TRAIL_MAX_LENGTH`, so the tail retreats as fast as
//! the head advances). The flood is timed the same way: a rider reaching a cell
//! after the tail left it may use it, which is what makes a pocket closed by
//! one's own trail open again.

use glam::Vec2;
use shared::{ARENA_WIDTH, BASE_SPEED, TICK_SECS, TRAIL_MAX_LENGTH};

/// Cell side in world units.
pub const CELL: f32 = 20.0;
/// Cells per axis. The arena is square (bots assume it).
pub const N: usize = (ARENA_WIDTH / CELL) as usize;
const CELLS: usize = N * N;
/// Distance covered in one tick at base speed.
pub const STEP: f32 = BASE_SPEED * TICK_SECS;
/// Ticks to cross one cell at base speed.
const CELL_TICKS: f32 = CELL / STEP;
/// A cell that never frees up.
const FOREVER: u16 = u16::MAX;

/// Blocked cells and the tick (from now) each one frees up.
#[derive(Clone, Debug)]
pub struct Space {
    free_at: Vec<u16>,
    /// The same grid at `COARSE` times the cell size, a cell freeing up when
    /// the last of its fine cells does: the board of the territory race.
    coarse: Vec<u16>,
}

/// Fine cells per coarse cell side.
pub const COARSE: usize = 2;
/// Coarse cells per axis.
pub const M: usize = N / COARSE;
const _: () = assert!(N.is_multiple_of(COARSE));

fn cell_index(p: Vec2) -> usize {
    let local = (p + Vec2::splat(ARENA_WIDTH * 0.5)) / CELL;
    let x = (local.x.max(0.0) as usize).min(N - 1);
    let y = (local.y.max(0.0) as usize).min(N - 1);
    y * N + x
}

/// Cells the segment `a..b` passes through, sampled twice per cell.
fn cells_along(a: Vec2, b: Vec2) -> impl Iterator<Item = (usize, f32)> {
    let length = a.distance(b);
    let samples = ((length / (CELL * 0.5)).ceil() as usize).max(1);
    (0..=samples).map(move |i| {
        let t = i as f32 / samples as f32;
        (cell_index(a.lerp(b, t)), t)
    })
}

impl Space {
    /// Rasterize the arena `walls` and the live riders' trails. Each trail is
    /// the polyline of `Trail::points`, tail first.
    pub fn build(walls: &[[f32; 4]], trails: &[Vec<Vec2>]) -> Self {
        let mut space = Self {
            free_at: vec![0; CELLS],
            coarse: Vec::new(),
        };
        for w in walls {
            let (a, b) = (Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3]));
            for (cell, _) in cells_along(a, b) {
                space.free_at[cell] = FOREVER;
            }
        }
        for points in trails {
            let total: f32 = points.windows(2).map(|p| p[0].distance(p[1])).sum();
            // A trail shorter than the cap does not shrink until it is full.
            let mut arc = (TRAIL_MAX_LENGTH - total).max(0.0);
            for pair in points.windows(2) {
                let length = pair[0].distance(pair[1]);
                for (cell, t) in cells_along(pair[0], pair[1]) {
                    let frees = ((arc + length * t) / STEP)
                        .ceil()
                        .min(f32::from(FOREVER - 1));
                    space.free_at[cell] = space.free_at[cell].max(frees as u16);
                }
                arc += length;
            }
        }
        let mut coarse = vec![0; M * M];
        for (cell, &frees) in space.free_at.iter().enumerate() {
            let at = (cell / N / COARSE) * M + (cell % N) / COARSE;
            coarse[at] = coarse[at].max(frees);
        }
        space.coarse = coarse;
        space
    }

    /// Per coarse cell, the tick it frees up (see `race`).
    pub fn coarse(&self) -> &[u16] {
        &self.coarse
    }
}

/// Reusable buffers for [`Space::measure`]; stamps avoid clearing them.
#[derive(Clone, Debug)]
pub struct Scratch {
    stamp: u32,
    seen: Vec<u32>,
    laid: Vec<u32>,
    dist: Vec<u16>,
    queue: Vec<u32>,
    path_cells: Vec<u32>,
    pub race: crate::race::RaceScratch,
}

impl Default for Scratch {
    fn default() -> Self {
        Self {
            stamp: 0,
            seen: vec![0; CELLS],
            laid: vec![0; CELLS],
            dist: vec![0; CELLS],
            queue: Vec::new(),
            path_cells: Vec::new(),
            race: crate::race::RaceScratch::default(),
        }
    }
}

/// Room found by a flood.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    /// Cells reached (at most the cap).
    pub area: u32,
    /// Share of the laid path's cells that run along something closed (a
    /// wall, a trail, the path itself), `0..=1`: riding tight leaves no gaps
    /// to waste.
    pub hug: f32,
}

impl Space {
    /// Flood from the cell of `from`, reached `at` ticks from now, over cells
    /// that are free by the time the rider gets there. `laid` is the rider's
    /// own new trail (blocked for good). Stops after `cap` cells. The start
    /// cell is always free: the head sits on its own trail.
    pub fn measure(
        &self,
        scratch: &mut Scratch,
        laid: &[(Vec2, Vec2)],
        from: Vec2,
        at: u32,
        cap: u32,
    ) -> Room {
        scratch.stamp += 1;
        let stamp = scratch.stamp;
        scratch.path_cells.clear();
        for &(a, b) in laid {
            for (cell, _) in cells_along(a, b) {
                if scratch.laid[cell] != stamp {
                    scratch.laid[cell] = stamp;
                    scratch.path_cells.push(cell as u32);
                }
            }
        }
        let hug = self.hug(scratch, at);

        let start = cell_index(from);
        scratch.seen[start] = stamp;
        scratch.dist[start] = 0;
        scratch.queue.clear();
        scratch.queue.push(start as u32);
        let mut area = 1;
        let mut head = 0;
        while head < scratch.queue.len() && area < cap {
            let cell = scratch.queue[head] as usize;
            head += 1;
            let arrive = at + ((f32::from(scratch.dist[cell]) + 1.0) * CELL_TICKS) as u32;
            for next in side_cells(cell) {
                let open = scratch.laid[next] != stamp && u32::from(self.free_at[next]) <= arrive;
                if !open || scratch.seen[next] == stamp {
                    continue;
                }
                scratch.seen[next] = stamp;
                scratch.dist[next] = scratch.dist[cell] + 1;
                scratch.queue.push(next as u32);
                area += 1;
            }
        }
        Room { area, hug }
    }

    /// Share of the laid cells with a closed side neighbour at `at`: an
    /// obstacle, or another cell of the same laid path.
    fn hug(&self, scratch: &Scratch, at: u32) -> f32 {
        let cells = &scratch.path_cells;
        if cells.is_empty() {
            return 0.0;
        }
        let hugging = cells
            .iter()
            .filter(|&&cell| {
                side_cells(cell as usize).any(|next| {
                    u32::from(self.free_at[next]) > at
                        || (scratch.laid[next] == scratch.stamp && next != cell as usize)
                })
            })
            .count();
        hugging as f32 / cells.len() as f32
    }
}

/// The in-bounds 4-neighbours of a cell.
fn side_cells(cell: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (cell % N, cell / N);
    [
        (x > 0).then(|| cell - 1),
        (x + 1 < N).then(|| cell + 1),
        (y > 0).then(|| cell - N),
        (y + 1 < N).then(|| cell + N),
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walls(rect: [f32; 4]) -> Vec<[f32; 4]> {
        let [x0, y0, x1, y1] = rect;
        vec![
            [x0, y0, x1, y0],
            [x1, y0, x1, y1],
            [x1, y1, x0, y1],
            [x0, y1, x0, y0],
        ]
    }

    #[test]
    fn a_closed_box_holds_its_own_cells_and_nothing_more() {
        // A 200 x 200 box: 10 x 10 cells, minus the ring its walls occupy.
        let space = Space::build(&walls([-100.0, -100.0, 100.0, 100.0]), &[]);
        let mut scratch = Scratch::default();
        let room = space.measure(&mut scratch, &[], Vec2::ZERO, 0, 100_000);
        assert!((60..=100).contains(&room.area), "{}", room.area);
    }

    #[test]
    fn the_laid_trail_splits_a_region() {
        let space = Space::build(&walls([-100.0, -100.0, 100.0, 100.0]), &[]);
        let mut scratch = Scratch::default();
        let free = space.measure(&mut scratch, &[], Vec2::new(-60.0, 0.0), 0, 100_000);
        let wall = [(Vec2::new(0.0, -100.0), Vec2::new(0.0, 100.0))];
        let cut = space.measure(&mut scratch, &wall, Vec2::new(-60.0, 0.0), 0, 100_000);
        assert!(
            cut.area < free.area / 2 + 10,
            "{} vs {}",
            cut.area,
            free.area
        );
    }

    #[test]
    fn a_trail_blocks_until_its_tail_passes() {
        // A long straight trail walls off the left half of a box; its tail end
        // (the first point) frees up soonest, so a late arrival gets through
        // there while an immediate one does not.
        let box_walls = walls([-300.0, -300.0, 300.0, 300.0]);
        let trail = vec![Vec2::new(0.0, -290.0), Vec2::new(0.0, 290.0)];
        let space = Space::build(&box_walls, &[trail]);
        let mut scratch = Scratch::default();
        let now = space.measure(&mut scratch, &[], Vec2::new(-100.0, 0.0), 0, 100_000);
        let later = space.measure(&mut scratch, &[], Vec2::new(-100.0, 0.0), 700, 100_000);
        assert!(
            later.area > now.area * 3 / 2,
            "{} vs {}",
            later.area,
            now.area
        );
    }
}
