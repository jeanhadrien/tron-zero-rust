//! Voronoi territory + chamber (block-cut) evaluation for Territorialist.
//!
//! Pure, deterministic, std-only helpers over the bot [`GridCache`]. No `rand`,
//! no wall-clock, no physics: inputs are grid cells, outputs are scores/counts.
//! Both passes are unit-testable in isolation; out-of-range or blocked start
//! cells degrade gracefully (empty region / zero surplus) instead of panicking.

use std::collections::VecDeque;

use super::world::{GRID_CELLS, GRID_N, GridCache};

/// Owner tags for the simultaneous Voronoi flood.
const UNCLAIMED: u8 = 0;
const MINE: u8 = 1;
const THEIRS: u8 = 2;
/// Reached at the same distance by both riders: owned by neither.
const TIED: u8 = 3;

/// Simultaneous 2-source BFS over free cells, capped at `budget` visited nodes.
///
/// Each free cell is owned by whichever rider reaches it first; cells reached
/// at equal distance are tied and count for neither. Returns
/// `mine - theirs` in raw cells (negative when the enemy owns more).
///
/// A blocked start cell simply seeds nothing, so a boxed-in rider scores the
/// (negative) enemy region instead of panicking. Same-cell starts tie at zero.
pub fn voronoi_surplus(
    grid: &GridCache,
    self_cell: (usize, usize),
    enemy_cell: (usize, usize),
    budget: u32,
) -> i32 {
    if !in_bounds(self_cell) || !in_bounds(enemy_cell) {
        return 0;
    }
    let self_free = !grid.is_blocked(self_cell.0, self_cell.1);
    let enemy_free = !grid.is_blocked(enemy_cell.0, enemy_cell.1);
    if !self_free && !enemy_free {
        return 0;
    }
    if self_cell == enemy_cell {
        return 0;
    }

    let mut owner = vec![UNCLAIMED; GRID_CELLS];
    let mut dist = vec![-1i32; GRID_CELLS];
    let mut queue = VecDeque::new();
    if self_free {
        let idx = self_cell.1 * GRID_N + self_cell.0;
        owner[idx] = MINE;
        dist[idx] = 0;
        queue.push_back(idx);
    }
    if enemy_free {
        let idx = enemy_cell.1 * GRID_N + enemy_cell.0;
        // Distinct cells (checked above), so no seed collision here.
        owner[idx] = THEIRS;
        dist[idx] = 0;
        queue.push_back(idx);
    }

    let mut mine = 0i32;
    let mut theirs = 0i32;
    let cap = budget.max(1) as usize;
    let mut visited = 0usize;
    while let Some(cell) = queue.pop_front() {
        if visited >= cap {
            break;
        }
        visited += 1;
        match owner[cell] {
            MINE => mine += 1,
            THEIRS => theirs += 1,
            _ => {}
        }
        let (cx, cy) = (cell % GRID_N, cell / GRID_N);
        for (nx, ny) in neighbors(cx, cy) {
            let next = ny * GRID_N + nx;
            if grid.is_blocked(nx, ny) {
                continue;
            }
            if owner[next] == UNCLAIMED {
                // First wave to arrive owns the cell; ties are resolved below
                // when a same-distance wave from the other side arrives.
                owner[next] = owner[cell];
                dist[next] = dist[cell] + 1;
                queue.push_back(next);
            } else if owner[next] != owner[cell]
                && owner[next] != TIED
                && owner[cell] != TIED
                && dist[next] == dist[cell] + 1
            {
                // Same-distance arrival from the other rider: contested.
                match owner[next] {
                    MINE => mine -= 1,
                    THEIRS => theirs -= 1,
                    _ => {}
                }
                owner[next] = TIED;
            }
        }
    }
    mine - theirs
}

/// Farmable-region summary for the connected free-space block holding `self_cell`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChamberStats {
    /// Free cells reachable from the start without crossing a blocked cell.
    pub size: usize,
    /// Articulation points inside the chamber (iterative Tarjan). Cutting any
    /// of these splits the chamber; many cuts mean an open battlefront, zero
    /// means a sealed pocket.
    pub cuts: usize,
    /// Battlefront rule (a1k0n-lite): heavily jointed or map-scale chambers
    /// are shared space and farm nothing.
    pub contested: bool,
}

impl ChamberStats {
    /// Empty chamber: blocked start cell.
    pub fn empty() -> Self {
        Self {
            size: 0,
            cuts: 0,
            contested: true,
        }
    }

    /// Farmable value in `[0, 1]`: sealed pockets score their map share,
    /// contested battlefronts score zero.
    pub fn value(self) -> f32 {
        if self.contested || self.size == 0 {
            0.0
        } else {
            self.size as f32 / GRID_CELLS as f32
        }
    }
}

/// Flood the free-space block holding `self_cell`, then run an iterative
/// Tarjan articulation pass over it. Blocked/out-of-range starts yield an
/// empty (zero-value) chamber.
pub fn chamber_stats(grid: &GridCache, self_cell: (usize, usize)) -> ChamberStats {
    if !in_bounds(self_cell) || grid.is_blocked(self_cell.0, self_cell.1) {
        return ChamberStats::empty();
    }
    // Flood the chamber.
    let mut member = vec![false; GRID_CELLS];
    let mut queue = VecDeque::new();
    member[self_cell.1 * GRID_N + self_cell.0] = true;
    queue.push_back(self_cell);
    while let Some((x, y)) = queue.pop_front() {
        for (nx, ny) in neighbors(x, y) {
            let idx = ny * GRID_N + nx;
            if !member[idx] && !grid.is_blocked(nx, ny) {
                member[idx] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    let size = member.iter().filter(|&&m| m).count();
    let cuts = articulation_count(grid, &member, size);
    // Two or more joints (or half the map) means shared battlefront space,
    // not a farmable pocket.
    let contested = cuts >= 2 || size * 2 >= GRID_CELLS;
    ChamberStats {
        size,
        cuts,
        contested,
    }
}

/// Iterative Tarjan articulation-point count over the `member` subgraph.
/// Linear in chamber size; explicit stack, no recursion.
fn articulation_count(grid: &GridCache, member: &[bool], size: usize) -> usize {
    if size <= 2 {
        return 0;
    }
    // Compact cell -> node numbering for dense disc/low arrays.
    let mut node_of = vec![-1i32; GRID_CELLS];
    let mut cell_of_node: Vec<usize> = Vec::with_capacity(size);
    for (cell, &inside) in member.iter().enumerate() {
        if inside {
            node_of[cell] = cell_of_node.len() as i32;
            cell_of_node.push(cell);
        }
    }
    let n = cell_of_node.len();
    // Adjacency over 4-neighbour free member cells.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (node, &cell) in cell_of_node.iter().enumerate() {
        let (cx, cy) = (cell % GRID_N, cell / GRID_N);
        for (nx, ny) in neighbors(cx, cy) {
            let next = ny * GRID_N + nx;
            if member[next] && !grid.is_blocked(nx, ny) {
                adj[node].push(node_of[next] as usize);
            }
        }
    }

    let mut disc = vec![-1i32; n];
    let mut low = vec![0i32; n];
    let mut parent = vec![-1i32; n];
    let mut is_cut = vec![false; n];
    let mut timer = 0i32;

    for root in 0..n {
        if disc[root] != -1 {
            continue;
        }
        disc[root] = timer;
        low[root] = timer;
        timer += 1;
        let mut root_children = 0u32;
        // Stack of (node, next-neighbour-index).
        let mut stack = vec![(root, 0usize)];
        while let Some((u, child)) = stack.pop() {
            if child < adj[u].len() {
                stack.push((u, child + 1));
                let v = adj[u][child];
                if disc[v] == -1 {
                    parent[v] = u as i32;
                    disc[v] = timer;
                    low[v] = timer;
                    timer += 1;
                    if u == root {
                        root_children += 1;
                    }
                    stack.push((v, 0));
                } else if v as i32 != parent[u] {
                    low[u] = low[u].min(disc[v]);
                }
            } else if parent[u] != -1 {
                let p = parent[u] as usize;
                low[p] = low[p].min(low[u]);
                if p != root && low[u] >= disc[p] {
                    is_cut[p] = true;
                }
            }
        }
        if root_children > 1 {
            is_cut[root] = true;
        }
    }
    is_cut.iter().filter(|&&c| c).count()
}

fn in_bounds(cell: (usize, usize)) -> bool {
    cell.0 < GRID_N && cell.1 < GRID_N
}

fn neighbors(gx: usize, gy: usize) -> impl Iterator<Item = (usize, usize)> {
    [
        gx.checked_sub(1).map(|x| (x, gy)),
        (gx + 1 < GRID_N).then_some((gx + 1, gy)),
        gy.checked_sub(1).map(|y| (gx, y)),
        (gy + 1 < GRID_N).then_some((gx, gy + 1)),
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Vec2;
    use shared::ARENA_WIDTH;

    // Cell-centre helper without touching world internals: mirrors
    // `GridCache::cell_of` (cell = ARENA_WIDTH / GRID_N, origin at -half).
    fn center(gx: usize, gy: usize) -> Vec2 {
        let cell = ARENA_WIDTH / GRID_N as f32;
        let half = ARENA_WIDTH * 0.5;
        Vec2::new(
            (gx as f32 + 0.5) * cell - half,
            (gy as f32 + 0.5) * cell - half,
        )
    }

    #[test]
    fn open_arena_surplus_is_zero_for_symmetric_starts() {
        let grid = GridCache::build(&[]);
        let surplus = voronoi_surplus(&grid, (8, 16), (23, 16), 800);
        assert!(
            surplus.abs() <= 2,
            "symmetric open arena should tie, got {surplus}"
        );
    }

    #[test]
    fn pocketed_enemy_gives_positive_surplus() {
        // Seal the enemy in a 5x5 ring of blocked cells; self roams the rest.
        let (ex, ey) = (24usize, 16usize);
        let ring: Vec<(Vec2, Vec2)> = vec![
            (center(ex - 3, ey - 3), center(ex + 3, ey - 3)),
            (center(ex + 3, ey - 3), center(ex + 3, ey + 3)),
            (center(ex + 3, ey + 3), center(ex - 3, ey + 3)),
            (center(ex - 3, ey + 3), center(ex - 3, ey - 3)),
        ];
        let grid = GridCache::build(&ring);
        assert!(grid.is_blocked(ex - 3, ey));
        let surplus = voronoi_surplus(&grid, (6, 16), (ex, ey), 1024);
        assert!(
            surplus > 100,
            "expected big positive surplus, got {surplus}"
        );
    }

    #[test]
    fn blocked_starts_degrade_gracefully() {
        let grid = GridCache::build(&[]);
        assert_eq!(voronoi_surplus(&grid, (32, 0), (5, 5), 800), 0);
        assert_eq!(voronoi_surplus(&grid, (5, 5), (5, 5), 800), 0);
        assert_eq!(chamber_stats(&grid, (99, 99)), ChamberStats::empty());
        assert_eq!(ChamberStats::empty().value(), 0.0);
    }

    #[test]
    fn corridor_chamber_has_cuts_and_open_arena_is_contested() {
        // Vertical wall at gx=16 through cell centres, one-cell gap at gy=16.
        let wall: Vec<(Vec2, Vec2)> = vec![
            (center(16, 0), center(16, 15)),
            (center(16, 17), center(16, 31)),
        ];
        let grid = GridCache::build(&wall);
        let stats = chamber_stats(&grid, (8, 16));
        assert!(stats.size > GRID_CELLS / 2, "gap keeps halves joined");
        assert!(stats.cuts >= 1, "gap cells are joints, got {:?}", stats);
        assert!(stats.contested);
        assert_eq!(stats.value(), 0.0);
    }

    #[test]
    fn sealed_pocket_has_value_and_no_cuts() {
        // 7x7 sealed ring around (8, 8): interior 5x5 pocket.
        let (px, py) = (8usize, 8usize);
        let ring: Vec<(Vec2, Vec2)> = vec![
            (center(px - 3, py - 3), center(px + 3, py - 3)),
            (center(px + 3, py - 3), center(px + 3, py + 3)),
            (center(px + 3, py + 3), center(px - 3, py + 3)),
            (center(px - 3, py + 3), center(px - 3, py - 3)),
        ];
        let grid = GridCache::build(&ring);
        let stats = chamber_stats(&grid, (px, py));
        assert_eq!(stats.size, 25, "5x5 interior, got {:?}", stats);
        assert_eq!(stats.cuts, 0);
        assert!(!stats.contested);
        assert!(stats.value() > 0.0);
    }
}
