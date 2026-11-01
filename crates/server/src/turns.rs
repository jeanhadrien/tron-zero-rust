//! Server side of one-shot turn delivery.
//!
//! Each physical press arrives as a sequenced `TurnCommand` stamped with the
//! tick the client predicted it at. The client's timeline runs ahead of the
//! server's, so commands normally arrive early and wait in the per-link
//! [`TurnQueue`] until their tick:
//!
//! - execution tick = `max(intended, previous turn's tick + 1)`: in order, at
//!   most one turn per rider per tick;
//! - a turn that is late but within `TURN_LATE_WINDOW_TICKS` runs on the first
//!   tick it can; older ones are rejected (`TooLate`), never silently dropped;
//! - intended ticks beyond `TURN_MAX_LEAD_TICKS` ahead are rejected, and a
//!   client can never schedule earlier than receipt, so backdating gains nothing;
//! - `apply_turn_commands` overwrites the human riders' `ActionState` every
//!   tick, so Lightyear's native-input fallback (repeat the last input) can
//!   neither fire a phantom turn nor add a second one next to a command.
//!
//! The server never rewinds: late turns only ever execute on future ticks.

use std::collections::VecDeque;

use bevy::prelude::*;
use lightyear::prelude::*;
use shared::protocol::{
    TURN_LATE_WINDOW_TICKS, TURN_MAX_LEAD_TICKS, TURN_QUEUE_CAP, TurnAck, TurnChannel, TurnCommand,
    TurnDirection, TurnOutcome, TurnRejectReason,
};
use shared::{ActionState, IsAlive, LifeGeneration, PlayerInput};

use crate::systems::Human;

/// An accepted command waiting for its execution tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduledTurn {
    pub seq: u64,
    pub direction: TurnDirection,
    pub generation: u128,
    /// Execution tick: the intended tick, pushed past the previous turn's.
    pub due: u32,
}

/// What `TurnQueue` needs to know about the rider that owns a link.
#[derive(Clone, Copy, Debug)]
pub struct RiderView {
    pub alive: bool,
    pub generation: u128,
}

/// Per-link turn schedule. Lives on the link entity, so it dies with the
/// connection and cannot leak turns into a later session.
#[derive(Component, Default, Debug)]
pub struct TurnQueue {
    pending: VecDeque<ScheduledTurn>,
    /// Newest accepted command: `(generation, seq, due)`. Gives the duplicate
    /// watermark and the one-turn-per-tick floor.
    last: Option<(u128, u64, u32)>,
}

/// Signed tick distance `a - b`, safe against `u32` extremes from a client.
fn ticks_between(a: u32, b: u32) -> i64 {
    i64::from(a) - i64::from(b)
}

impl TurnQueue {
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Validate and schedule one command received while the server's last
    /// simulated tick is `now` (so the first tick it can run on is `now + 1`).
    pub fn admit(
        &mut self,
        cmd: TurnCommand,
        now: u32,
        rider: Option<RiderView>,
    ) -> Result<ScheduledTurn, TurnRejectReason> {
        let rider = rider.ok_or(TurnRejectReason::NotEligible)?;
        if cmd.generation != rider.generation {
            return Err(TurnRejectReason::StaleLife);
        }
        if !rider.alive {
            return Err(TurnRejectReason::NotAlive);
        }
        let previous = self
            .last
            .filter(|(generation, ..)| *generation == cmd.generation);
        if previous.is_some_and(|(_, seq, _)| cmd.seq <= seq) {
            return Err(TurnRejectReason::Duplicate);
        }
        if ticks_between(cmd.tick, now) > i64::from(TURN_MAX_LEAD_TICKS) {
            return Err(TurnRejectReason::TooFarAhead);
        }
        if self.pending.len() >= TURN_QUEUE_CAP {
            return Err(TurnRejectReason::QueueFull);
        }
        let due = previous.map_or(cmd.tick, |(.., due)| cmd.tick.max(due.saturating_add(1)));
        let turn = ScheduledTurn {
            seq: cmd.seq,
            direction: cmd.direction,
            generation: cmd.generation,
            due,
        };
        self.pending.push_back(turn);
        self.last = Some((cmd.generation, cmd.seq, due));
        Ok(turn)
    }

    /// Take what happens on tick `now`: turns that missed the late window
    /// (to be rejected) and at most one turn to execute.
    pub fn poll(&mut self, now: u32) -> (Vec<ScheduledTurn>, Option<ScheduledTurn>) {
        let mut expired = Vec::new();
        while let Some(head) = self.pending.front().copied() {
            let lateness = ticks_between(now, head.due);
            if lateness < 0 {
                break;
            }
            self.pending.pop_front();
            if lateness > i64::from(TURN_LATE_WINDOW_TICKS) {
                expired.push(head);
            } else {
                return (expired, Some(head));
            }
        }
        (expired, None)
    }

    /// Remove turns the current rider state can no longer execute: all of them
    /// when it is dead, otherwise those from another life.
    pub fn remove_invalid(&mut self, rider: RiderView) -> Vec<(ScheduledTurn, TurnRejectReason)> {
        let mut removed = Vec::new();
        self.pending.retain(|turn| {
            let reason = if !rider.alive {
                TurnRejectReason::NotAlive
            } else if turn.generation != rider.generation {
                TurnRejectReason::StaleLife
            } else {
                return true;
            };
            removed.push((*turn, reason));
            false
        });
        removed
    }
}

/// Opt-in diagnostics: insert this resource to record every command's fate
/// (the headless network harness does; the production server does not).
#[derive(Resource, Default, Debug)]
pub struct TurnLog(pub Vec<TurnEvent>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnEvent {
    pub link: Entity,
    pub seq: u64,
    /// Server `LocalTimeline` tick when this happened.
    pub server_tick: u32,
    pub kind: TurnEventKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnEventKind {
    /// Accepted at receipt; `intended` is the tick the client stamped.
    Queued {
        intended: u32,
        due: u32,
    },
    /// Written to the rider's `ActionState` for `server_tick`.
    Applied {
        due: u32,
    },
    Rejected(TurnRejectReason),
}

/// Record the event and ack a rejection to the client.
fn reject(
    log: &mut Option<ResMut<TurnLog>>,
    sender: &mut MessageSender<TurnAck>,
    link: Entity,
    seq: u64,
    server_tick: u32,
    reason: TurnRejectReason,
) {
    debug!(?link, seq, server_tick, ?reason, "Turn rejected");
    note(log, link, seq, server_tick, TurnEventKind::Rejected(reason));
    sender.send::<TurnChannel>(TurnAck {
        seq,
        outcome: TurnOutcome::Rejected { reason },
    });
}

fn note(
    log: &mut Option<ResMut<TurnLog>>,
    link: Entity,
    seq: u64,
    server_tick: u32,
    kind: TurnEventKind,
) {
    if let Some(log) = log {
        log.0.push(TurnEvent {
            link,
            seq,
            server_tick,
            kind,
        });
    }
}

fn rider_view(alive: &IsAlive, generation: &LifeGeneration) -> RiderView {
    RiderView {
        alive: alive.0,
        generation: generation.0,
    }
}

/// Admit received `TurnCommand`s into the owning link's queue.
///
/// PreUpdate, after `MessageSystems::Receive`: Lightyear clears receivers in
/// `Last`, even on frames without a fixed tick, so drain every frame. Ownership
/// comes from the link (no entity id in the message). Rejections are acked
/// immediately; accepted commands are acked when they execute.
#[allow(clippy::type_complexity)]
pub fn collect_turn_commands(
    timeline: Res<LocalTimeline>,
    mut links: Query<
        (
            Entity,
            &mut MessageReceiver<TurnCommand>,
            &mut MessageSender<TurnAck>,
            &mut TurnQueue,
        ),
        With<Connected>,
    >,
    riders: Query<(&ControlledBy, &IsAlive, &LifeGeneration), With<Human>>,
    mut log: Option<ResMut<TurnLog>>,
) {
    let now = timeline.tick().0;
    for (link, mut receiver, mut sender, mut queue) in &mut links {
        let rider = riders
            .iter()
            .find(|(controlled, ..)| controlled.owner == link)
            .map(|(_, alive, generation)| rider_view(alive, generation));
        for cmd in receiver.receive() {
            match queue.admit(cmd, now, rider) {
                Ok(turn) => {
                    debug!(
                        ?link,
                        seq = cmd.seq,
                        intended = cmd.tick,
                        due = turn.due,
                        now,
                        "Turn queued"
                    );
                    let kind = TurnEventKind::Queued {
                        intended: cmd.tick,
                        due: turn.due,
                    };
                    note(&mut log, link, cmd.seq, now, kind);
                }
                Err(reason) => reject(&mut log, &mut sender, link, cmd.seq, now, reason),
            }
        }
    }
}

/// Write each human rider's `ActionState` for this tick: `None`, or the one
/// turn scheduled for it.
///
/// FixedUpdate, before `simulate_players` and therefore after Lightyear's
/// `UpdateActionState` (FixedPreUpdate), which this overrides. Bots are not
/// `Human` and keep writing their own input.
pub fn apply_turn_commands(
    timeline: Res<LocalTimeline>,
    mut links: Query<(&mut TurnQueue, &mut MessageSender<TurnAck>)>,
    mut riders: Query<
        (
            &ControlledBy,
            &IsAlive,
            &LifeGeneration,
            &mut ActionState<PlayerInput>,
        ),
        With<Human>,
    >,
    mut log: Option<ResMut<TurnLog>>,
) {
    let now = timeline.tick().0;
    for (controlled, alive, generation, mut action) in &mut riders {
        action.0 = PlayerInput::None;
        let link = controlled.owner;
        let Ok((mut queue, mut sender)) = links.get_mut(link) else {
            continue;
        };
        let view = rider_view(alive, generation);
        for (turn, reason) in queue.remove_invalid(view) {
            reject(&mut log, &mut sender, link, turn.seq, now, reason);
        }
        let (expired, due) = queue.poll(now);
        for turn in expired {
            reject(
                &mut log,
                &mut sender,
                link,
                turn.seq,
                now,
                TurnRejectReason::TooLate,
            );
        }
        if let Some(turn) = due {
            action.0 = turn.direction.to_input(turn.generation);
            debug!(?link, seq = turn.seq, due = turn.due, now, "Turn applied");
            note(
                &mut log,
                link,
                turn.seq,
                now,
                TurnEventKind::Applied { due: turn.due },
            );
            sender.send::<TurnChannel>(TurnAck {
                seq: turn.seq,
                outcome: TurnOutcome::Applied { server_tick: now },
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEN: u128 = 7;
    const ALIVE: RiderView = RiderView {
        alive: true,
        generation: GEN,
    };

    fn cmd(seq: u64, tick: u32) -> TurnCommand {
        TurnCommand {
            seq,
            direction: TurnDirection::Left,
            generation: GEN,
            tick,
        }
    }

    fn admit(
        queue: &mut TurnQueue,
        seq: u64,
        tick: u32,
        now: u32,
    ) -> Result<u32, TurnRejectReason> {
        queue.admit(cmd(seq, tick), now, Some(ALIVE)).map(|t| t.due)
    }

    #[test]
    fn early_command_waits_for_its_tick() {
        let mut queue = TurnQueue::default();
        assert_eq!(admit(&mut queue, 0, 110, 100), Ok(110));
        for now in 101..110 {
            assert_eq!(queue.poll(now), (vec![], None));
        }
        let (expired, due) = queue.poll(110);
        assert!(expired.is_empty());
        assert_eq!(due.map(|t| t.seq), Some(0));
        assert!(queue.is_empty());
    }

    #[test]
    fn same_tick_burst_runs_in_order_one_per_tick() {
        let mut queue = TurnQueue::default();
        for seq in 0..3 {
            assert_eq!(admit(&mut queue, seq, 110, 100), Ok(110 + seq as u32));
        }
        for (i, now) in (110..113).enumerate() {
            let (_, due) = queue.poll(now);
            assert_eq!(due.map(|t| t.seq), Some(i as u64));
        }
    }

    #[test]
    fn late_within_window_runs_on_first_free_tick_beyond_is_rejected() {
        let mut queue = TurnQueue::default();
        // Intended 100, received while the server is at 105: runs at once.
        assert_eq!(admit(&mut queue, 0, 100, 105), Ok(100));
        assert_eq!(queue.poll(106).1.map(|t| t.seq), Some(0));
        // Intended 110, but the first chance is 110 + window + 1.
        assert_eq!(admit(&mut queue, 1, 110, 100), Ok(110));
        let now = 110 + TURN_LATE_WINDOW_TICKS + 1;
        let (expired, due) = queue.poll(now);
        assert_eq!(expired.iter().map(|t| t.seq).collect::<Vec<_>>(), vec![1]);
        assert!(due.is_none());
    }

    #[test]
    fn expired_head_does_not_block_a_runnable_follower() {
        let mut queue = TurnQueue::default();
        assert_eq!(admit(&mut queue, 0, 100, 99), Ok(100));
        assert_eq!(admit(&mut queue, 1, 105, 99), Ok(105));
        let (expired, due) = queue.poll(100 + TURN_LATE_WINDOW_TICKS + 1);
        assert_eq!(expired.len(), 1);
        assert_eq!(due.map(|t| t.seq), Some(1));
        assert!(queue.is_empty());
    }

    #[test]
    fn duplicates_and_reordered_seqs_never_execute_twice() {
        let mut queue = TurnQueue::default();
        assert_eq!(admit(&mut queue, 5, 110, 100), Ok(110));
        assert_eq!(
            admit(&mut queue, 5, 110, 100),
            Err(TurnRejectReason::Duplicate)
        );
        assert_eq!(
            admit(&mut queue, 4, 111, 100),
            Err(TurnRejectReason::Duplicate)
        );
        assert_eq!(admit(&mut queue, 6, 111, 100), Ok(111));
    }

    #[test]
    fn client_ticks_are_bounded_and_cannot_backdate_execution() {
        let mut queue = TurnQueue::default();
        let far = 100 + TURN_MAX_LEAD_TICKS as u32;
        assert_eq!(
            admit(&mut queue, 0, far + 1, 100),
            Err(TurnRejectReason::TooFarAhead)
        );
        assert_eq!(
            admit(&mut queue, 1, u32::MAX, 100),
            Err(TurnRejectReason::TooFarAhead)
        );
        assert_eq!(admit(&mut queue, 2, far, 100), Ok(far));
        // A backdated stamp is only "late": it still waits for the next tick
        // the server simulates, never an already-simulated one.
        let mut queue = TurnQueue::default();
        assert_eq!(admit(&mut queue, 0, 0, 100), Ok(0));
        let (expired, due) = queue.poll(101);
        assert_eq!(expired.len(), 1);
        assert!(due.is_none());
    }

    #[test]
    fn rider_state_gates_admission_and_execution() {
        let mut queue = TurnQueue::default();
        let dead = RiderView {
            alive: false,
            ..ALIVE
        };
        let other_life = RiderView {
            generation: GEN + 1,
            ..ALIVE
        };
        assert_eq!(
            queue.admit(cmd(0, 110), 100, None).err(),
            Some(TurnRejectReason::NotEligible)
        );
        assert_eq!(
            queue.admit(cmd(0, 110), 100, Some(dead)).err(),
            Some(TurnRejectReason::NotAlive)
        );
        assert_eq!(
            queue.admit(cmd(0, 110), 100, Some(other_life)).err(),
            Some(TurnRejectReason::StaleLife)
        );
        assert_eq!(admit(&mut queue, 0, 110, 100), Ok(110));
        let removed = queue.remove_invalid(other_life);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].1, TurnRejectReason::StaleLife);
        assert_eq!(admit(&mut queue, 1, 120, 100), Ok(120));
        assert_eq!(queue.remove_invalid(dead)[0].1, TurnRejectReason::NotAlive);
        assert!(queue.is_empty());
    }

    #[test]
    fn new_life_restarts_the_seq_watermark_but_keeps_nothing_else() {
        let mut queue = TurnQueue::default();
        assert_eq!(admit(&mut queue, 9, 110, 100), Ok(110));
        let next = RiderView {
            generation: GEN + 1,
            ..ALIVE
        };
        let first = TurnCommand {
            generation: GEN + 1,
            ..cmd(0, 100)
        };
        assert_eq!(queue.admit(first, 100, Some(next)).map(|t| t.due), Ok(100));
    }

    #[test]
    fn queue_cap_rejects_new_commands_without_dropping_accepted_ones() {
        let mut queue = TurnQueue::default();
        for seq in 0..TURN_QUEUE_CAP as u64 {
            assert!(admit(&mut queue, seq, 100 + seq as u32, 100).is_ok());
        }
        assert_eq!(
            admit(&mut queue, 99, 110, 100),
            Err(TurnRejectReason::QueueFull)
        );
        assert_eq!(queue.len(), TURN_QUEUE_CAP);
    }

    #[test]
    fn apply_overwrites_native_input_every_tick() {
        let mut app = App::new();
        app.init_resource::<LocalTimeline>();
        app.add_systems(Update, apply_turn_commands);
        let link = app
            .world_mut()
            .spawn((TurnQueue::default(), MessageSender::<TurnAck>::default()))
            .id();
        let rider = app
            .world_mut()
            .spawn((
                Human,
                ControlledBy {
                    owner: link,
                    lifetime: Lifetime::SessionBased,
                },
                IsAlive(true),
                LifeGeneration(GEN),
                // Lightyear's last-input fallback has just repeated a turn.
                ActionState(PlayerInput::TurnLeftFor(GEN)),
            ))
            .id();
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(110);
        app.update();
        assert_eq!(
            app.world()
                .get::<ActionState<PlayerInput>>(rider)
                .unwrap()
                .0,
            PlayerInput::None
        );
        let mut queue = app.world_mut().get_mut::<TurnQueue>(link).unwrap();
        queue.admit(cmd(0, 111), 110, Some(ALIVE)).unwrap();
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(1);
        app.update();
        assert_eq!(
            app.world()
                .get::<ActionState<PlayerInput>>(rider)
                .unwrap()
                .0,
            PlayerInput::TurnLeftFor(GEN)
        );
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(1);
        app.update();
        assert_eq!(
            app.world()
                .get::<ActionState<PlayerInput>>(rider)
                .unwrap()
                .0,
            PlayerInput::None
        );
    }
}
