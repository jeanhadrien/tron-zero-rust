//! Redundant turn delivery, client side.
//!
//! Every turn command stays in the [`TurnOutbox`] until the server reports it
//! decided, and *all* of the outbox goes out in every `TurnBatch` (one per
//! client tick while it is non-empty). A lost packet is therefore repaired by
//! the next tick's packet, 8 ms later, instead of by a reliable-channel resend
//! a round trip and a half later. The server drops the copies it has already
//! decided, so redundancy costs bandwidth only: a few dozen bytes per tick for
//! about one round trip after each press.
//!
//! The stamps are labels on Lightyear's input timeline, which a resync
//! (`SyncEvent`) relabels by a tick delta without re-simulating anything.
//! [`follow_timeline_resync`] moves every stamp in the outbox by that delta, so
//! the age-based prune and the stamps agree with the new labeling, and Lightyear's own
//! prediction/input histories (which it shifts by the same delta) still line
//! up with what the server is asked to do. Commands the server decided before
//! the resync keep their old stamp there (the shifted copies are dropped as
//! already decided): such a turn runs once, at the old label, and the client
//! corrects it by rollback like any late turn. Never lost, never doubled.

use std::collections::VecDeque;

use bevy::prelude::*;
use lightyear::prelude::client::InputTimelineConfig;
use lightyear::prelude::{Client, Connected, LocalTimeline, MessageSender, SyncEvent, Tick};
use shared::protocol::{TURN_BATCH_MAX, TURN_RESEND_TICKS, TurnBatch, TurnChannel, TurnCommand};

/// Commands the server has not yet reported as decided, oldest first. Because
/// the server acknowledges a prefix ("everything below N"), this is always a
/// contiguous run of sequence numbers.
#[derive(Resource, Default, Debug)]
pub struct TurnOutbox {
    unacked: VecDeque<TurnCommand>,
    last_sent_tick: Option<u32>,
}

impl TurnOutbox {
    pub fn len(&self) -> usize {
        self.unacked.len()
    }

    pub fn is_empty(&self) -> bool {
        self.unacked.is_empty()
    }

    /// Intended ticks of the unacknowledged commands, oldest first.
    pub fn stamps(&self) -> impl Iterator<Item = u32> + '_ {
        self.unacked.iter().map(|command| command.tick)
    }

    /// Add a freshly pressed command. A full outbox (the server has been silent
    /// for a whole batch worth of presses) gives up on its oldest command.
    pub fn push(&mut self, command: TurnCommand) {
        if self.unacked.len() >= TURN_BATCH_MAX {
            self.unacked.pop_front();
        }
        self.unacked.push_back(command);
    }

    /// The server has decided every command below `received_below`.
    pub fn acknowledge(&mut self, received_below: u64) {
        while self
            .unacked
            .front()
            .is_some_and(|command| command.seq < received_below)
        {
            self.unacked.pop_front();
        }
    }

    /// The timeline was relabelled by `delta` ticks: renumber every stamp the
    /// same way, so the outbox keeps meaning "the tick the client predicted
    /// this turn at" in the new labeling.
    pub fn shift_ticks(&mut self, delta: i32) {
        let shift = |tick: u32| (Tick(tick) + delta).0;
        for command in &mut self.unacked {
            command.tick = shift(command.tick);
        }
        // A batch already went out under the old labeling; the new tick that
        // happens to equal the old one is a different tick.
        self.last_sent_tick = None;
    }

    /// The batch to send at client tick `now`: everything still unacked, at
    /// most once per tick. Commands stamped more than `TURN_RESEND_TICKS` ago
    /// are dropped first; the server would reject them as too late anyway.
    pub fn batch(&mut self, now: u32) -> Option<TurnBatch> {
        self.unacked
            .retain(|command| Tick(now) - Tick(command.tick) <= TURN_RESEND_TICKS as i32);
        if self.unacked.is_empty() || self.last_sent_tick == Some(now) {
            return None;
        }
        self.last_sent_tick = Some(now);
        Some(TurnBatch {
            commands: self.unacked.iter().copied().collect(),
        })
    }
}

/// Keep the outbox stamps in step with the input timeline (see module docs).
pub fn follow_timeline_resync(
    event: On<SyncEvent<InputTimelineConfig>>,
    mut outbox: ResMut<TurnOutbox>,
) {
    outbox.shift_ticks(event.tick_delta);
}

/// Send the outbox to the server. PostUpdate, before Lightyear serializes
/// messages, so a command popped this frame leaves in this frame's packet.
pub fn send_turn_batch(
    timeline: Option<Res<LocalTimeline>>,
    mut outbox: ResMut<TurnOutbox>,
    mut senders: Query<&mut MessageSender<TurnBatch>, (With<Client>, With<Connected>)>,
) {
    let Some(timeline) = timeline else {
        return;
    };
    let Some(batch) = outbox.batch(timeline.tick().0) else {
        return;
    };
    for mut sender in &mut senders {
        sender.send::<TurnChannel>(batch.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::protocol::TurnDirection;

    fn command(seq: u64, tick: u32) -> TurnCommand {
        TurnCommand {
            seq,
            direction: TurnDirection::Left,
            generation: 1,
            tick,
        }
    }

    fn seqs(batch: &TurnBatch) -> Vec<u64> {
        batch.commands.iter().map(|c| c.seq).collect()
    }

    #[test]
    fn every_tick_repeats_all_unacked_commands_until_acknowledged() {
        let mut outbox = TurnOutbox::default();
        assert!(outbox.batch(100).is_none(), "nothing to send");
        outbox.push(command(0, 100));
        outbox.push(command(1, 103));
        assert_eq!(seqs(&outbox.batch(103).unwrap()), vec![0, 1]);
        assert!(outbox.batch(103).is_none(), "at most one batch per tick");
        assert_eq!(seqs(&outbox.batch(104).unwrap()), vec![0, 1]);
        outbox.acknowledge(1);
        assert_eq!(seqs(&outbox.batch(105).unwrap()), vec![1]);
        outbox.acknowledge(2);
        assert!(outbox.is_empty());
        assert!(outbox.batch(106).is_none());
    }

    #[test]
    fn stale_and_surplus_commands_are_dropped_so_the_batch_stays_small() {
        let mut outbox = TurnOutbox::default();
        outbox.push(command(0, 100));
        outbox.push(command(1, 100 + TURN_RESEND_TICKS));
        let now = 100 + TURN_RESEND_TICKS + 1;
        assert_eq!(seqs(&outbox.batch(now).unwrap()), vec![1]);
        for seq in 2..2 + 2 * TURN_BATCH_MAX as u64 {
            outbox.push(command(seq, now));
        }
        let batch = outbox.batch(now + 1).unwrap();
        assert_eq!(batch.commands.len(), TURN_BATCH_MAX);
        assert_eq!(
            batch.commands.last().unwrap().seq,
            1 + 2 * TURN_BATCH_MAX as u64,
            "the newest commands survive"
        );
    }

    #[test]
    fn a_resync_relabels_stamps_and_keeps_the_age_prune_honest() {
        let mut outbox = TurnOutbox::default();
        outbox.push(command(0, 100));
        outbox.push(command(1, 110));
        assert!(outbox.batch(112).is_some());
        // Backward jump: tick 112 is now labelled 101; the same batch tick
        // must not be skipped as "already sent".
        outbox.shift_ticks(-11);
        let batch = outbox.batch(101).expect("sent again under the new labels");
        assert_eq!(
            batch.commands.iter().map(|c| c.tick).collect::<Vec<_>>(),
            vec![89, 99]
        );
        // Forward jump: without the shift these would look 60 ticks old.
        outbox.shift_ticks(60);
        let batch = outbox.batch(161).expect("still within the resend window");
        assert_eq!(
            batch
                .commands
                .iter()
                .map(|c| (c.seq, c.tick))
                .collect::<Vec<_>>(),
            vec![(0, 149), (1, 159)]
        );
    }
}
