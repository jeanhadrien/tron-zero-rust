//! Server side of one-shot turn delivery.
//!
//! Each physical press is a sequenced `TurnCommand` stamped with the tick the
//! client predicted it at. The client repeats every command the server has not
//! yet acknowledged in each of its per-tick `TurnBatch` packets, so a lost
//! packet is made up by the next one. Per link, the [`TurnQueue`]:
//!
//! - **decides each seq once.** Everything below `next_seq` is decided (queued
//!   or rejected); resent copies of those are dropped silently, without a log
//!   entry, a reply or any cost to the sender's budget. `next_seq` is also the
//!   cumulative acknowledgment the client prunes its outbox with.
//! - **tolerates gaps by skipping them.** A batch always holds every command
//!   its sender still considers unacknowledged, and acknowledgments are a
//!   prefix, so a batch that contains seq `n+1` contains `n` too unless the
//!   client deliberately gave `n` up (it was far too old to run). Waiting for
//!   such a seq would only delay the later ones, so it is skipped. Commands in
//!   a batch are processed in ascending seq regardless of wire order.
//! - **schedules.** Execution tick = `max(intended, previous turn's tick + 1)`:
//!   in order, at most one turn per rider per tick, early commands wait.
//! - **bounds lateness.** A turn that is late but within
//!   `TURN_LATE_WINDOW_TICKS` runs on the first tick it can; older ones are
//!   rejected (`TooLate`), never silently dropped. Intended ticks beyond
//!   `TURN_MAX_LEAD_TICKS` ahead are rejected, and a client can never schedule
//!   earlier than receipt, so backdating gains nothing.
//!
//! `apply_turn_commands` overwrites the human riders' `ActionState` every tick,
//! so Lightyear's native-input fallback (repeat the last input) can neither
//! fire a phantom turn nor add a second one next to a command. The server never
//! rewinds: late turns only ever execute on future ticks.
//!
//! Replies are one coalesced, unreliable `TurnAck` per link per frame
//! (`send_turn_acks`): the cumulative `received_below` plus the newest verdict.
//! Nothing is buffered per command, so a client that floods the server cannot
//! make it queue acknowledgments; a link that misbehaves is closed
//! (`budget.rs`).

use std::collections::VecDeque;

use bevy::prelude::*;
use lightyear::prelude::*;
use shared::protocol::{
    TURN_BATCH_MAX, TURN_LATE_WINDOW_TICKS, TURN_MAX_LEAD_TICKS, TURN_QUEUE_CAP, TurnAck,
    TurnAckChannel, TurnBatch, TurnCommand, TurnDirection, TurnOutcome, TurnRejectReason,
    TurnVerdict,
};
use shared::{ActionState, IsAlive, LifeGeneration, PlayerInput};

use crate::budget::{LinkBudget, Meter};
use crate::session::SessionLease;
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

/// What became of a newly decided command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Accepted; `intended` is the tick the client stamped.
    Queued {
        turn: ScheduledTurn,
        intended: u32,
    },
    Rejected {
        seq: u64,
        reason: TurnRejectReason,
    },
}

/// Per-link turn schedule. Lives on the link entity, so it dies with the
/// connection and cannot leak turns into a later session.
#[derive(Component, Default, Debug)]
pub struct TurnQueue {
    pending: VecDeque<ScheduledTurn>,
    /// Every seq below this is decided. Seqs are monotonic for a whole
    /// connection (they do not restart with a new life).
    next_seq: u64,
    /// Generation and execution tick of the newest queued turn: the
    /// one-turn-per-tick floor, which a new life starts afresh.
    floor: Option<(u128, u32)>,
    /// A batch arrived (or a verdict is new) since the last ack went out.
    ack_due: bool,
    /// Newest verdict not yet reported to the client.
    verdict: Option<TurnVerdict>,
}

/// Signed tick distance `a - b`; wrap-safe for any two ticks less than 2^31
/// apart (Lightyear's `Tick` subtraction truncates to `i32`).
fn ticks_between(a: u32, b: u32) -> i64 {
    i64::from(Tick(a) - Tick(b))
}

impl TurnQueue {
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Every seq below this has been decided; the client may forget them.
    pub fn received_below(&self) -> u64 {
        self.next_seq
    }

    /// Decide one received command while the server's last simulated tick is
    /// `now` (so the first tick it can run on is `now + 1`). `None` means the
    /// seq was decided before: a redundant copy, ignored without a trace.
    pub fn receive(
        &mut self,
        cmd: TurnCommand,
        now: u32,
        rider: Option<RiderView>,
    ) -> Option<Decision> {
        if cmd.seq < self.next_seq {
            return None;
        }
        // Rejections are final too, or the client would resend them forever.
        self.next_seq = cmd.seq.saturating_add(1);
        let decision = match self.schedule(cmd, now, rider) {
            Ok(turn) => Decision::Queued {
                turn,
                intended: cmd.tick,
            },
            Err(reason) => {
                self.note_verdict(cmd.seq, TurnOutcome::Rejected { reason });
                Decision::Rejected {
                    seq: cmd.seq,
                    reason,
                }
            }
        };
        Some(decision)
    }

    fn schedule(
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
        if ticks_between(cmd.tick, now) > i64::from(TURN_MAX_LEAD_TICKS) {
            return Err(TurnRejectReason::TooFarAhead);
        }
        if self.pending.len() >= TURN_QUEUE_CAP {
            return Err(TurnRejectReason::QueueFull);
        }
        let earliest = self
            .floor
            .filter(|(generation, _)| *generation == cmd.generation)
            .map(|(_, due)| due.wrapping_add(1));
        let due = match earliest {
            Some(earliest) if ticks_between(cmd.tick, earliest) < 0 => earliest,
            _ => cmd.tick,
        };
        let turn = ScheduledTurn {
            seq: cmd.seq,
            direction: cmd.direction,
            generation: cmd.generation,
            due,
        };
        self.pending.push_back(turn);
        self.floor = Some((cmd.generation, due));
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

    /// A batch arrived: answer it, even when every command in it was already
    /// decided, so a lost ack is repaired by the next copy.
    pub fn mark_received(&mut self) {
        self.ack_due = true;
    }

    /// Remember the newest outcome for the next ack (older ones are
    /// superseded: acks are diagnostics, not a delivery channel).
    pub fn note_verdict(&mut self, seq: u64, outcome: TurnOutcome) {
        self.verdict = Some(TurnVerdict { seq, outcome });
        self.ack_due = true;
    }

    /// The ack to send now, if any; clears the pending state.
    pub fn take_ack(&mut self) -> Option<TurnAck> {
        std::mem::take(&mut self.ack_due).then(|| TurnAck {
            received_below: self.next_seq,
            newest: self.verdict.take(),
        })
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

/// Record a rejection that happened after receipt (expiry, death) and report
/// it in the next ack.
fn reject_queued(
    log: &mut Option<ResMut<TurnLog>>,
    queue: &mut TurnQueue,
    link: Entity,
    seq: u64,
    server_tick: u32,
    reason: TurnRejectReason,
) {
    debug!(?link, seq, server_tick, ?reason, "Turn rejected");
    note(log, link, seq, server_tick, TurnEventKind::Rejected(reason));
    queue.note_verdict(seq, TurnOutcome::Rejected { reason });
}

fn rider_view(alive: &IsAlive, generation: &LifeGeneration) -> RiderView {
    RiderView {
        alive: alive.0,
        generation: generation.0,
    }
}

/// Decide the commands of received `TurnBatch`es and queue the accepted ones
/// on the owning link.
///
/// PreUpdate, after `MessageSystems::Receive`: Lightyear clears receivers in
/// `Last`, even on frames without a fixed tick, so drain every frame. Ownership
/// comes from the link (no entity id in the message). A link that exceeds its
/// message or rejection budget is closed.
pub fn collect_turn_commands(
    timeline: Res<LocalTimeline>,
    time: Res<Time<Real>>,
    mut links: Query<
        (
            Entity,
            &mut MessageReceiver<TurnBatch>,
            &mut TurnQueue,
            &mut LinkBudget,
            &mut SessionLease,
        ),
        With<Connected>,
    >,
    riders: Query<(&ControlledBy, &IsAlive, &LifeGeneration), With<Human>>,
    mut log: Option<ResMut<TurnLog>>,
) {
    let now = timeline.tick().0;
    let clock = time.elapsed_secs_f64();
    for (link, mut receiver, mut queue, mut budget, mut lease) in &mut links {
        let rider = riders
            .iter()
            .find(|(controlled, ..)| controlled.owner == link)
            .map(|(_, alive, generation)| rider_view(alive, generation));
        for mut batch in receiver.receive() {
            let oversized = batch.commands.len() > TURN_BATCH_MAX;
            if oversized || !budget.spend(Meter::TurnBatches, 1, clock) {
                warn!(?link, oversized, "Turn message flood; closing the link");
                lease.close();
                break;
            }
            queue.mark_received();
            batch.commands.sort_by_key(|cmd| cmd.seq);
            for cmd in batch.commands {
                match queue.receive(cmd, now, rider) {
                    None => {}
                    Some(Decision::Queued { turn, intended }) => {
                        debug!(
                            ?link,
                            seq = cmd.seq,
                            intended,
                            due = turn.due,
                            now,
                            "Turn queued"
                        );
                        let kind = TurnEventKind::Queued {
                            intended,
                            due: turn.due,
                        };
                        note(&mut log, link, cmd.seq, now, kind);
                    }
                    Some(Decision::Rejected { seq, reason }) => {
                        debug!(?link, seq, now, ?reason, "Turn rejected");
                        note(&mut log, link, seq, now, TurnEventKind::Rejected(reason));
                        if !budget.spend(Meter::TurnRejections, 1, clock) {
                            warn!(?link, "Too many rejected turns; closing the link");
                            lease.close();
                        }
                    }
                }
            }
            if lease.is_closing() {
                break;
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
    mut links: Query<&mut TurnQueue>,
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
        let Ok(mut queue) = links.get_mut(link) else {
            continue;
        };
        let view = rider_view(alive, generation);
        for (turn, reason) in queue.remove_invalid(view) {
            reject_queued(&mut log, &mut queue, link, turn.seq, now, reason);
        }
        let (expired, due) = queue.poll(now);
        for turn in expired {
            let reason = TurnRejectReason::TooLate;
            reject_queued(&mut log, &mut queue, link, turn.seq, now, reason);
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
            queue.note_verdict(turn.seq, TurnOutcome::Applied { server_tick: now });
        }
    }
}

/// Send each link's pending ack: at most one small unreliable message per link
/// per frame, whatever the client sent.
pub fn send_turn_acks(mut links: Query<(&mut TurnQueue, &mut MessageSender<TurnAck>)>) {
    for (mut queue, mut sender) in &mut links {
        if let Some(ack) = queue.take_ack() {
            sender.send::<TurnAckChannel>(ack);
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

    /// Receive one command; `Ok(due)` if queued, `Err(None)` if it was a
    /// redundant copy, `Err(Some(reason))` if rejected.
    fn admit(
        queue: &mut TurnQueue,
        seq: u64,
        tick: u32,
        now: u32,
    ) -> Result<u32, Option<TurnRejectReason>> {
        match queue.receive(cmd(seq, tick), now, Some(ALIVE)) {
            Some(Decision::Queued { turn, .. }) => Ok(turn.due),
            Some(Decision::Rejected { reason, .. }) => Err(Some(reason)),
            None => Err(None),
        }
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
    fn redundant_copies_are_silent_and_never_execute_twice() {
        let mut queue = TurnQueue::default();
        assert_eq!(admit(&mut queue, 5, 110, 100), Ok(110));
        // The same command in every following batch: no decision, no effect.
        for _ in 0..100 {
            assert_eq!(admit(&mut queue, 5, 110, 100), Err(None));
        }
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.received_below(), 6);
        // A rejected command is decided as well: its copies stay silent.
        let too_far = 100 + TURN_MAX_LEAD_TICKS as u32 + 1;
        assert_eq!(
            admit(&mut queue, 6, too_far, 100),
            Err(Some(TurnRejectReason::TooFarAhead))
        );
        assert_eq!(admit(&mut queue, 6, too_far, 100), Err(None));
        assert_eq!(queue.received_below(), 7);
    }

    #[test]
    fn a_batch_is_processed_in_seq_order_and_gaps_are_skipped() {
        // The client repeats everything unacked, so seq 1 normally rides with
        // seq 2; the server decides in ascending order whatever the wire order.
        let mut queue = TurnQueue::default();
        assert_eq!(admit(&mut queue, 0, 110, 100), Ok(110));
        assert_eq!(admit(&mut queue, 1, 110, 100), Ok(111));
        assert_eq!(admit(&mut queue, 2, 110, 100), Ok(112));
        // Seq 3 and 4 were given up by the client; 5 still runs normally.
        assert_eq!(admit(&mut queue, 5, 120, 100), Ok(120));
        assert_eq!(queue.received_below(), 6);
        // An older batch arriving late only holds decided commands.
        assert_eq!(admit(&mut queue, 2, 110, 100), Err(None));
        assert_eq!(admit(&mut queue, 1, 110, 100), Err(None));
    }

    #[test]
    fn ack_is_cumulative_coalesced_and_sent_only_when_something_arrived() {
        let mut queue = TurnQueue::default();
        assert!(queue.take_ack().is_none());
        queue.mark_received();
        assert_eq!(
            queue.take_ack(),
            Some(TurnAck {
                received_below: 0,
                newest: None
            })
        );
        assert!(queue.take_ack().is_none(), "one ack per arrival");
        assert_eq!(admit(&mut queue, 0, 110, 100), Ok(110));
        queue.note_verdict(0, TurnOutcome::Applied { server_tick: 110 });
        queue.note_verdict(
            1,
            TurnOutcome::Rejected {
                reason: TurnRejectReason::TooLate,
            },
        );
        let ack = queue.take_ack().unwrap();
        assert_eq!(ack.received_below, 1);
        assert_eq!(ack.newest.map(|v| v.seq), Some(1), "newest verdict wins");
        // A redundant batch re-triggers the ack so a lost one is repaired.
        queue.mark_received();
        assert_eq!(queue.take_ack().unwrap().newest, None);
    }

    #[test]
    fn client_ticks_are_bounded_and_cannot_backdate_execution() {
        let mut queue = TurnQueue::default();
        let far = 100 + TURN_MAX_LEAD_TICKS as u32;
        assert_eq!(
            admit(&mut queue, 0, far + 1, 100),
            Err(Some(TurnRejectReason::TooFarAhead))
        );
        assert_eq!(
            admit(&mut queue, 1, 100 + i32::MAX as u32, 100),
            Err(Some(TurnRejectReason::TooFarAhead))
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
    fn tick_arithmetic_survives_wraparound() {
        assert_eq!(ticks_between(5, u32::MAX - 4), 10);
        assert_eq!(ticks_between(u32::MAX - 4, 5), -10);
        let mut queue = TurnQueue::default();
        let now = u32::MAX - 2;
        let at = |ahead: u32| now.wrapping_add(ahead);
        // Intended 3 ticks ahead, past the wrap: not "far in the past".
        assert_eq!(admit(&mut queue, 0, at(3), now), Ok(at(3)));
        // The one-turn-per-tick floor also works across the wrap.
        assert_eq!(admit(&mut queue, 1, at(3), now), Ok(at(4)));
        assert_eq!(queue.poll(at(2)), (vec![], None));
        assert_eq!(queue.poll(at(3)).1.map(|t| t.seq), Some(0));
        assert_eq!(queue.poll(at(4)).1.map(|t| t.seq), Some(1));
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
        let rejected =
            |queue: &mut TurnQueue, seq, rider| match queue.receive(cmd(seq, 110), 100, rider) {
                Some(Decision::Rejected { reason, .. }) => Some(reason),
                _ => None,
            };
        assert_eq!(
            rejected(&mut queue, 0, None),
            Some(TurnRejectReason::NotEligible)
        );
        assert_eq!(
            rejected(&mut queue, 1, Some(dead)),
            Some(TurnRejectReason::NotAlive)
        );
        assert_eq!(
            rejected(&mut queue, 2, Some(other_life)),
            Some(TurnRejectReason::StaleLife)
        );
        assert_eq!(admit(&mut queue, 3, 110, 100), Ok(110));
        let removed = queue.remove_invalid(other_life);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].1, TurnRejectReason::StaleLife);
        assert_eq!(admit(&mut queue, 4, 120, 100), Ok(120));
        assert_eq!(queue.remove_invalid(dead)[0].1, TurnRejectReason::NotAlive);
        assert!(queue.is_empty());
    }

    #[test]
    fn new_life_restarts_the_floor_but_seqs_keep_counting() {
        let mut queue = TurnQueue::default();
        assert_eq!(admit(&mut queue, 9, 110, 100), Ok(110));
        let next = RiderView {
            generation: GEN + 1,
            ..ALIVE
        };
        let first = TurnCommand {
            generation: GEN + 1,
            ..cmd(10, 100)
        };
        let due = match queue.receive(first, 100, Some(next)) {
            Some(Decision::Queued { turn, .. }) => turn.due,
            other => panic!("expected the first turn of a new life to queue: {other:?}"),
        };
        assert_eq!(due, 100, "not pushed behind the old life's last turn");
    }

    #[test]
    fn queue_cap_rejects_new_commands_without_dropping_accepted_ones() {
        let mut queue = TurnQueue::default();
        for seq in 0..TURN_QUEUE_CAP as u64 {
            assert!(admit(&mut queue, seq, 100 + seq as u32, 100).is_ok());
        }
        assert_eq!(
            admit(&mut queue, 99, 110, 100),
            Err(Some(TurnRejectReason::QueueFull))
        );
        assert_eq!(queue.len(), TURN_QUEUE_CAP);
    }

    #[test]
    fn apply_overwrites_native_input_every_tick() {
        let mut app = App::new();
        app.init_resource::<LocalTimeline>();
        app.add_systems(Update, apply_turn_commands);
        let link = app.world_mut().spawn(TurnQueue::default()).id();
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
        queue.receive(cmd(0, 111), 110, Some(ALIVE)).unwrap();
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
        // The execution is reported in the next ack.
        let ack = app
            .world_mut()
            .get_mut::<TurnQueue>(link)
            .unwrap()
            .take_ack()
            .unwrap();
        assert_eq!(
            ack.newest,
            Some(TurnVerdict {
                seq: 0,
                outcome: TurnOutcome::Applied { server_tick: 111 }
            })
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
