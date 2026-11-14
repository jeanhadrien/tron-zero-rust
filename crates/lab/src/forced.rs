//! Forced deaths: a rider that dies on its own trail or a wall, but only after
//! an opponent took its room away.
//!
//! The sim credits a death to what the rider faced (`death_cause`), so a rider
//! that was boxed in or cut off and then hit its own trail inside the pocket
//! counts as a suicide. This module looks at the situation just before such a
//! death and decides whether an opponent caused it. It is a lab measurement
//! only: the sim, the ranked rating and the trace do not use it.
//!
//! Definition. Take the rider's reachable room just before it died: a flood
//! fill from its head over the bots' room grid (`bots::space`: 20-unit cells over
//! the default arena, so like the bots it assumes a 2000-unit arena), blocked by
//! walls and every living trail (its own too), counted up to a cap. Every
//! segment is laid as a line of its own, so none frees up within the flood
//! (the bots' flood lets a trail's tail pass; that takes longer than any pocket).
//! The death is forced when
//! - that room was below what a rider needs to live (`NEED_CELLS`, the same
//!   pocket size the bots use), and
//! - the trail an opponent laid during the last `WINDOW` ticks is part of what
//!   closed it: without those segments the room would have been at least
//!   `NEED_CELLS` and at least twice as large.
//!
//! The opponent whose recent trail gives back the most room is credited. A
//! rider that coils into itself far from everyone has no opponent trail in its
//! pocket, so it stays a pure self-kill; a pocket whose opponent walls are
//! older than the window was entered knowingly and stays one too.

use bots::space::{NEED_CELLS, STEP, Scratch, Space};
use glam::Vec2;

use crate::sim::{Event, EventKind, RiderView};

/// Ticks (2 s) an opponent's trail counts as recent.
const WINDOW: u32 = 240;
/// The flood stops counting here.
const CAP: u32 = 3000;

/// A trail segment, and whether it was laid during the last `WINDOW` ticks.
struct Seg {
    a: Vec2,
    b: Vec2,
    recent: bool,
}

/// The opponent (rider index) that forced the death of `victim` at `tick`, if
/// any. `pre` is every rider as it was just before the deadly tick.
pub fn forced_by(
    pre: &[RiderView],
    walls: &[[f32; 4]],
    events: &[Event],
    tick: u32,
    victim: usize,
) -> Option<usize> {
    let trails: Vec<Vec<Seg>> = pre
        .iter()
        .map(|r| {
            if r.alive {
                segments(r, events, tick)
            } else {
                Vec::new()
            }
        })
        .collect();
    let head = pre[victim].position;
    let mut scratch = Scratch::default();
    let mut room = |skip: Option<usize>| {
        let lines: Vec<Vec<Vec2>> = trails
            .iter()
            .enumerate()
            .flat_map(|(owner, trail)| {
                trail
                    .iter()
                    .filter(move |seg| !(skip == Some(owner) && seg.recent))
                    .map(|seg| vec![seg.a, seg.b])
            })
            .collect();
        let space = Space::build(walls, &lines);
        space.measure(&mut scratch, &[], head, 0, CAP).area
    };
    let closed = room(None);
    if closed as f32 >= NEED_CELLS {
        return None;
    }
    let (best, gain) = (0..pre.len())
        .filter(|&k| k != victim && !trails[k].is_empty())
        .map(|k| (k, room(Some(k))))
        .max_by_key(|&(_, open)| open)?;
    (gain as f32 >= NEED_CELLS && gain >= 2 * closed).then_some(best)
}

/// A living rider's trail as segments. A leg is recent when the turn that ended
/// it (read from the rider's turn events this life) is; the last leg is still
/// being laid, so only the stretch ridden in the window counts, at the
/// rider's current speed.
fn segments(rider: &RiderView, events: &[Event], tick: u32) -> Vec<Seg> {
    let points = &rider.trail;
    if points.len() < 2 {
        return Vec::new();
    }
    // Turn ticks of this life, newest first, until the life or round began.
    let mut turns: Vec<u32> = Vec::new();
    for event in events.iter().rev() {
        match &event.kind {
            EventKind::RoundStart { .. } => break,
            _ if event.rider != rider.name => {}
            EventKind::Respawn { .. } | EventKind::Death { .. } => break,
            EventKind::Turn { .. } => turns.push(event.tick),
            _ => {}
        }
    }
    turns.reverse();
    let corners = points.len() - 2;
    // The newest `corners` turns made the interior corners, oldest first.
    let skip = turns.len().saturating_sub(corners);
    let ticks = &turns[skip..];
    let last = points.len() - 2;
    let mut segs = Vec::new();
    for (i, pair) in points.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        if i < last {
            let end = ticks.get(i).copied().unwrap_or(0);
            segs.push(Seg {
                a,
                b,
                recent: end + WINDOW >= tick,
            });
            continue;
        }
        let reach = WINDOW as f32 * STEP * rider.speed.max(1.0);
        let cut = a.lerp(b, (1.0 - reach / a.distance(b).max(0.001)).clamp(0.0, 1.0));
        segs.push(Seg {
            a,
            b: cut,
            recent: false,
        });
        segs.push(Seg {
            a: cut,
            b,
            recent: true,
        });
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rider(name: &str, head: Vec2, trail: Vec<Vec2>) -> RiderView {
        RiderView {
            name: name.into(),
            index: 0,
            alive: true,
            position: head,
            direction: Vec2::X,
            speed: 1.0,
            target: 1.0,
            rubber: 120.0,
            sliding: false,
            colliding: false,
            trail,
            deaths: Vec::new(),
            is_bot: true,
        }
    }

    fn turn(rider: &str, tick: u32) -> Event {
        Event {
            tick,
            rider: rider.into(),
            kind: EventKind::Turn {
                at: [0.0, 0.0],
                heading: "+x",
            },
        }
    }

    const WALLS: [[f32; 4]; 4] = [
        [-300.0, -300.0, 300.0, -300.0],
        [300.0, -300.0, 300.0, 300.0],
        [300.0, 300.0, -300.0, 300.0],
        [-300.0, 300.0, -300.0, -300.0],
    ];

    /// A rider in the west part of a 600-unit room, shut in by a vertical line
    /// at x = -100 (a pocket of about 4 x 30 cells), laid by `s1` and finished
    /// by its turn `age` ticks before the death (at tick 2000).
    fn pocket(age: u32) -> (Vec<RiderView>, Vec<Event>) {
        let victim = rider(
            "s0",
            Vec2::new(-200.0, 100.0),
            vec![Vec2::new(-250.0, 100.0), Vec2::new(-200.0, 100.0)],
        );
        let foe = rider(
            "s1",
            Vec2::new(0.0, 300.0),
            vec![
                Vec2::new(-100.0, -300.0),
                Vec2::new(-100.0, 300.0),
                Vec2::new(0.0, 300.0),
            ],
        );
        (vec![victim, foe], vec![turn("s1", 2000 - age)])
    }

    #[test]
    fn a_pocket_closed_by_a_fresh_foe_trail_is_forced() {
        let (pre, events) = pocket(30);
        assert_eq!(forced_by(&pre, &WALLS, &events, 2000, 0), Some(1));
    }

    #[test]
    fn an_old_foe_trail_or_a_pocket_of_ones_own_making_is_not() {
        let (pre, events) = pocket(30);
        // The same pocket, but the foe's line has stood for ten seconds.
        let (_, old) = pocket(1200);
        assert_eq!(forced_by(&pre, &WALLS, &old, 2000, 0), None);
        // Nobody else about: the victim coiled into itself.
        let alone = vec![pre[0].clone()];
        assert_eq!(forced_by(&alone, &WALLS, &events, 2000, 0), None);
    }
}
