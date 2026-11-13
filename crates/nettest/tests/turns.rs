//! End-to-end turn delivery over real UDP loopback: a production server and
//! production client systems, scripted key presses, traces on both sides.

use std::time::Duration;

use shared::PlayerInput;
use shared::protocol::{TURN_LATE_WINDOW_TICKS, TurnRejectReason};
use tron_zero_nettest::analysis::{Delivery, Fate, deliveries, server_turns};
use tron_zero_nettest::client::turns_of;
use tron_zero_nettest::{Impairment, LEFT, Net, RIGHT, TestClient, serial, staircase};

const SPACING: u32 = 12;

/// Run until the client has popped its whole script, then give the network
/// time to deliver, execute and acknowledge everything.
fn finish_script(net: &mut Net, client: usize) {
    let done = net.run_until(Duration::from_secs(60), |net| {
        net.clients[client].scripted_remaining() == 0 && net.clients[client].pending_presses() == 0
    });
    assert!(done, "script did not finish");
    net.run_server_ticks(150);
}

/// Everything the server logged for one client's commands, plus the ids needed
/// to look at both simulations.
struct Run {
    id: String,
    log: Vec<Delivery>,
}

impl Run {
    fn collect(net: &mut Net, client: usize) -> Self {
        let id = net.clients[client].player_id().expect("player id");
        let (_, link) = net.server.rider(&id).expect("rider");
        Self {
            log: deliveries(&net.server, Some(link)),
            id,
        }
    }
}

/// Invariants that must hold on ANY link: exactly-once, in order, never early,
/// explicit fate for everything, no phantom turns, client replay consistent.
/// Assumes the client session started at seq 0 and sent `sent` commands.
fn check_delivery_invariants(net: &mut Net, client: usize, run: &Run) {
    check_delivery(net, client, run, false);
}

/// [`check_delivery_invariants`]; with `relabelled` the stamp a turn carried
/// may differ from the client trace by timeline resyncs that happened between
/// its press and its first receipt (the trace is renumbered by every resync;
/// a copy the server decided earlier keeps the stamp it had then).
fn check_delivery(net: &mut Net, client: usize, run: &Run, relabelled: bool) {
    let sent = net.clients[client].turns_sent() as usize;
    assert_eq!(run.log.len(), sent, "every sent command reaches the server");
    let client_turns = turns_of(&net.clients[client].trace().first_input);
    assert_eq!(client_turns.len(), sent, "one predicted turn per command");

    let mut last_tick = None;
    let mut applied = Vec::new();
    for (i, delivery) in run.log.iter().enumerate() {
        assert_eq!(delivery.seq, i as u64, "sequence has no gaps");
        let predicted_at = client_turns[i].0;
        if relabelled {
            let deltas = &net.clients[client].trace().resyncs;
            let possible = (0..=deltas.len()).any(|from| {
                let later: i32 = deltas[from..].iter().sum();
                delivery.intended == Some(predicted_at.wrapping_add_signed(-later))
            });
            assert!(
                possible,
                "seq {i}: stamp {:?} is not the predicted tick {predicted_at} under any                  resync history {deltas:?}",
                delivery.intended
            );
        } else {
            assert_eq!(
                delivery.intended,
                Some(predicted_at),
                "seq {i}: the stamp is the tick the client predicted the turn at"
            );
        }
        match delivery.fate {
            Fate::Waiting => panic!("seq {} never got a verdict", delivery.seq),
            Fate::Rejected(reason) => assert!(
                matches!(
                    reason,
                    TurnRejectReason::TooLate | TurnRejectReason::NotAlive
                ),
                "unexpected rejection {reason:?} for seq {}",
                delivery.seq
            ),
            Fate::Applied { tick } => {
                assert!(
                    last_tick.is_none_or(|last| tick > last),
                    "in order, one per tick"
                );
                last_tick = Some(tick);
                let lateness = delivery.lateness().expect("applied has intended tick");
                assert!(lateness >= 0, "never executed before its intended tick");
                assert!(
                    lateness <= i64::from(TURN_LATE_WINDOW_TICKS),
                    "within the late window"
                );
                if delivery.arrival_offset().is_some_and(|offset| offset < 0) {
                    assert_eq!(lateness, 0, "a command that arrives early runs on its tick");
                }
                // The server executed what the client predicted for this seq.
                applied.push((tick, client_turns[i].1));
            }
        }
    }
    assert_eq!(
        server_turns(&net.server, &run.id),
        applied,
        "the server simulated exactly the applied turns: no phantom, no double turn"
    );
    let trace = net.clients[client].trace();
    assert!(
        trace.replay_input_mismatches.is_empty(),
        "rollback replay re-applied different inputs at ticks {:?}",
        trace.replay_input_mismatches
    );
    assert!(
        trace.reexecuted_ticks.is_empty(),
        "no tick ran twice outside rollback: {:?}",
        trace.reexecuted_ticks
    );
}

/// Client prediction and server agree on rider pose at every tick both ran.
fn assert_same_timeline(net: &mut Net, client: usize, run: &Run, from_tick: u32) {
    let mut compared = 0;
    for (tick, predicted) in net.clients[client].trace().history.range(from_tick..) {
        let Some(authoritative) = net.server.trace().0.get(&(run.id.clone(), *tick)) else {
            continue;
        };
        assert!(
            predicted.position.distance(authoritative.position) < 1e-2,
            "tick {tick}: client {:?} vs server {:?}",
            predicted.position,
            authoritative.position
        );
        assert_eq!(predicted.direction, authoritative.direction, "tick {tick}");
        compared += 1;
    }
    assert!(compared > 100, "compared only {compared} ticks");
}

fn clean_session() -> (Net, String) {
    let mut net = Net::new(Impairment::NONE);
    net.add_client();
    let id = net.join_and_settle().remove(0);
    (net, id)
}

#[test]
fn clean_link_turns_run_on_their_intended_tick_with_no_correction() {
    let _guard = serial();
    let (mut net, id) = clean_session();
    let from_tick = net.clients[0].tick();
    let rollbacks = net.clients[0].rollbacks();

    // Three presses in one frame, then a steady staircase.
    let start = from_tick + 30;
    for key in [LEFT, RIGHT, LEFT] {
        net.clients[0].schedule(start, key);
    }
    staircase(&mut net.clients[0], start + 10, 24, SPACING);
    finish_script(&mut net, 0);

    let run = Run::collect(&mut net, 0);
    assert_eq!(run.log.len(), 27);
    check_delivery_invariants(&mut net, 0, &run);
    for delivery in &run.log {
        assert_eq!(
            delivery.lateness(),
            Some(0),
            "seq {} not on time",
            delivery.seq
        );
        assert!(
            delivery.arrival_offset().unwrap() < 0,
            "arrives before its tick"
        );
        // The native Lightyear input history holds the same turn at the same tick (shared timeline).
        let Fate::Applied { tick } = delivery.fate else {
            unreachable!()
        };
        let state = net.server.trace().0[&(id.clone(), tick)];
        assert_ne!(state.executed, PlayerInput::None);
        assert_eq!(
            state.native,
            Some(state.executed),
            "native input history disagrees with the command at tick {tick}"
        );
    }
    assert_eq!(
        net.clients[0].rollbacks(),
        rollbacks,
        "on-time turns need no correction"
    );
    assert_same_timeline(&mut net, 0, &run, from_tick);
    assert!(
        net.clients[0].last_ack().last_seq.is_some(),
        "acks reach the client"
    );
    assert!(net.clients[0].rider_alive());
}

#[test]
fn rollback_replay_reuses_recorded_turns_and_sends_nothing() {
    let _guard = serial();
    let (mut net, _) = clean_session();
    let start = net.clients[0].tick() + 30;
    staircase(&mut net.clients[0], start, 20, SPACING);
    // Mid-script, rewind the client 40 ticks and replay them.
    net.run_until(Duration::from_secs(10), |net| {
        net.clients[0].tick() >= start + 100
    });
    let (replayed_before, tick) = (net.clients[0].trace().replayed_ticks, net.clients[0].tick());
    net.clients[0].force_rollback(tick - 40);
    finish_script(&mut net, 0);

    let trace = net.clients[0].trace();
    assert!(
        trace.replayed_ticks > replayed_before + 30,
        "the rollback replayed ticks"
    );
    let run = Run::collect(&mut net, 0);
    assert_eq!(
        run.log.len(),
        20,
        "replay neither re-sent nor consumed presses"
    );
    check_delivery_invariants(&mut net, 0, &run);
}

#[test]
fn native_input_cannot_turn_a_human_and_menu_presses_are_swallowed() {
    let _guard = serial();
    let (mut net, id) = clean_session();
    let generation = net.clients[0].rider_generation().expect("life");

    // A modified client writes a native turn but sends no command.
    let planted_at = net.server.tick();
    net.clients[0].plant_native_input(PlayerInput::TurnLeftFor(generation));
    net.run_server_ticks(60);
    let seen_natively = net
        .server
        .trace()
        .0
        .iter()
        .filter(|((player, _), state)| {
            player == &id && state.native.is_some_and(|n| n != PlayerInput::None)
        })
        .count();
    assert!(
        seen_natively > 0,
        "the native turn did reach the server's input buffer"
    );
    assert!(
        server_turns(&net.server, &id).is_empty(),
        "but it never turned the rider"
    );
    assert!(
        net.server
            .trace()
            .0
            .iter()
            .all(|((player, tick), s)| player != &id
                || *tick < planted_at
                || s.direction == glam::Vec2::X)
    );

    // Menu: a press while the menu is open is swallowed; seq stays gapless.
    net.clients[0].press(LEFT);
    net.run_server_ticks(20);
    net.clients[0].set_menu_open(true);
    net.clients[0].press(RIGHT);
    net.run_server_ticks(20);
    net.clients[0].set_menu_open(false);
    net.run_server_ticks(5);
    net.clients[0].press(RIGHT);
    finish_script(&mut net, 0);
    let run = Run::collect(&mut net, 0);
    assert_eq!(run.log.len(), 2, "the menu press was never sent");
    assert!(
        run.log
            .iter()
            .all(|d| matches!(d.fate, Fate::Applied { .. }))
    );
    // The client predicted the planted turn too (it is its own cheat), but only
    // the two real presses were executed, on the ticks the client predicted.
    let predicted = turns_of(&net.clients[0].trace().first_input);
    assert_eq!(predicted.len(), 3);
    assert_eq!(server_turns(&net.server, &id), predicted[1..]);
}

#[test]
fn impaired_link_still_executes_every_press_exactly_once_in_order() {
    let _guard = serial();
    let mut net = Net::new(Impairment::new(100, 15, 0.03));
    net.add_client();
    net.join_and_settle();
    let start = net.clients[0].tick() + 30;
    staircase(&mut net.clients[0], start, 30, 16);
    finish_script(&mut net, 0);

    let run = Run::collect(&mut net, 0);
    check_delivery_invariants(&mut net, 0, &run);
    let on_time = run.log.iter().filter(|d| d.lateness() == Some(0)).count();
    println!("on time: {on_time} of {}", run.log.len());
    // A missed turn can end the run early (the rider crashes), so only judge the
    // ratio when enough commands were sent.
    if run.log.len() >= 20 {
        assert!(on_time * 10 >= run.log.len() * 8, "most turns are on time");
    }
}

/// Add `extra` one-way delay to everything the server receives from now on.
fn set_uplink_delay(net: &mut Net, extra: Duration) {
    net.set_uplink_latency(extra);
}

#[test]
fn late_turns_run_within_the_window_and_expire_beyond_it() {
    let _guard = serial();
    let mut net = Net::new(Impairment::new(2, 0, 0.0));
    net.add_client();
    net.join_and_settle();
    let t0 = net.clients[0].tick() + 30;
    for (i, key) in [LEFT, RIGHT, LEFT, RIGHT].into_iter().enumerate() {
        net.clients[0].schedule(t0 + 80 * i as u32, key);
    }
    // Press 1 sees ~6 ticks of extra delay, press 2 about 30, press 3 none.
    let phases = [(t0 + 70, 70), (t0 + 150, 250), (t0 + 230, 0)];
    for (tick, extra_ms) in phases {
        net.run_until(Duration::from_secs(20), |net| net.clients[0].tick() >= tick);
        set_uplink_delay(&mut net, Duration::from_millis(extra_ms));
    }
    finish_script(&mut net, 0);

    let run = Run::collect(&mut net, 0);
    let fates: Vec<_> = run.log.iter().map(|d| (d.fate, d.lateness())).collect();
    println!("{fates:?}");
    assert_eq!(run.log.len(), 4);
    assert_eq!(run.log[0].lateness(), Some(0));
    let late = run.log[1].lateness().expect("press 1 still executes");
    assert!(
        (1..=i64::from(TURN_LATE_WINDOW_TICKS)).contains(&late),
        "late by {late}"
    );
    assert_eq!(run.log[2].fate, Fate::Rejected(TurnRejectReason::TooLate));
    assert_eq!(run.log[3].lateness(), Some(0));
    check_delivery_invariants(&mut net, 0, &run);
    assert!(
        net.clients[0].rollbacks() > 0,
        "late execution is a visible correction"
    );
}

/// A rejected turn's predicted corner must not survive on the client: the
/// server never re-sends an unchanged `Trail`, so only Lightyear's completed
/// mutate tick (unchanged check and confirmed restore) can remove it. Both the
/// predicting client and an observer must end with the server's exact corners.
#[test]
fn a_rejected_turn_leaves_no_ghost_corner_on_any_client() {
    let _guard = serial();
    let mut net = Net::new(Impairment::new(2, 0, 0.0));
    net.add_client();
    net.add_client();
    let ids = net.join_and_settle();
    let t0 = net.clients[0].tick() + 30;
    for (i, key) in [LEFT, RIGHT, LEFT].into_iter().enumerate() {
        net.clients[0].schedule(t0 + 80 * i as u32, key);
    }
    // The middle press arrives about 30 ticks late: past the window.
    let phases = [(t0 + 70, 250), (t0 + 150, 0)];
    for (tick, extra_ms) in phases {
        net.run_until(Duration::from_secs(20), |net| net.clients[0].tick() >= tick);
        set_uplink_delay(&mut net, Duration::from_millis(extra_ms));
    }
    finish_script(&mut net, 0);

    let run = Run::collect(&mut net, 0);
    assert_eq!(run.log[1].fate, Fate::Rejected(TurnRejectReason::TooLate));
    let server = net.server.trail_of(&ids[0]).expect("server trail");
    assert_eq!(server.0.len(), 3, "anchor + two executed turns: {server:?}");
    // The sim is bit-identical: exact equality, no tolerance.
    for client in 0..2 {
        assert_eq!(
            net.clients[client].trail_of(&ids[0]),
            Some(server.clone()),
            "client {client} disagrees with the server"
        );
    }
}

#[test]
fn respawn_and_reconnect_never_leak_turns() {
    let _guard = serial();
    let (mut net, id) = clean_session();
    let old_generation = net.clients[0].rider_generation().expect("life");

    // A burst is in flight when the rider dies.
    let start = net.clients[0].tick() + 20;
    for _ in 0..6 {
        net.clients[0].schedule(start, LEFT);
    }
    net.run_until(Duration::from_secs(10), |net| {
        net.server.turn_log().0.len() >= 3
    });
    net.server.kill_rider(&id);
    net.run_server_ticks(60);
    let applied_at_death = server_turns(&net.server, &id).len();
    assert!(applied_at_death < 6, "the burst was cut off by the death");

    // Respawn: nothing from the old life may fire.
    let respawned = net.run_until(Duration::from_secs(10), |net| {
        let client = &mut net.clients[0];
        if !client.rider_alive() {
            client.press(RIGHT_RESPAWN);
        }
        client.rider_alive() && client.rider_generation() != Some(old_generation)
    });
    assert!(respawned, "respawn accepted");
    let respawn_tick = net.server.tick();
    net.run_server_ticks(120);
    let leaked: Vec<_> = server_turns(&net.server, &id)
        .into_iter()
        .filter(|(tick, _)| *tick > respawn_tick)
        .collect();
    assert!(
        leaked.is_empty(),
        "old-life turns fired in the new life: {leaked:?}"
    );

    // The new life still takes fresh presses.
    let before = deliveries(&net.server, None).len();
    net.clients[0].press(LEFT);
    net.run_server_ticks(60);
    let log = deliveries(&net.server, None);
    assert_eq!(log.len(), before + 1);
    assert!(matches!(log.last().unwrap().fate, Fate::Applied { .. }));

    // Reconnect: a fresh session, fresh life, fresh queue.
    net.clients[0].disconnect();
    let offline = net.run_until(Duration::from_secs(10), |net| {
        net.clients[0].phase() == tron_zero_client::connection::ConnectionPhase::Offline
    });
    assert!(offline, "client finished disconnecting");
    let dropped = net.run_until(Duration::from_secs(5), |net| {
        net.server.player_ids().is_empty()
    });
    assert!(dropped, "server dropped the rider");
    let port = net.port();
    net.clients[0].connect(port);
    let ids = net.join_and_settle();
    assert_eq!(ids.len(), 1);
    let new_generation = net.clients[0].rider_generation().expect("life");
    assert_ne!(
        new_generation >> 64,
        old_generation >> 64,
        "new session nonce"
    );
    net.run_server_ticks(60);
    let (_, link) = net.server.rider(&ids[0]).expect("rider");
    assert!(
        deliveries(&net.server, Some(link)).is_empty(),
        "nothing from the old connection is attributed to the new link"
    );
    net.clients[0].press(LEFT);
    net.run_server_ticks(60);
    let fresh = deliveries(&net.server, Some(link));
    assert_eq!(fresh.len(), 1);
    assert!(matches!(fresh[0].fate, Fate::Applied { .. }));
    let _: &TestClient = &net.clients[0];
}

const RIGHT_RESPAWN: bevy::input::keyboard::KeyCode = bevy::input::keyboard::KeyCode::Space;

/// Run until `presses_left` or fewer scripted presses remain and one of the
/// popped ones is on the wire but not yet acknowledged, then relabel the client
/// timeline by `delta`.
fn resync_with_turns_in_flight(net: &mut Net, presses_left: usize, delta: i32) {
    let in_flight = net.run_until(Duration::from_secs(20), |net| {
        net.clients[0].scripted_remaining() <= presses_left && net.clients[0].turns_unacked() > 0
    });
    assert!(in_flight, "no press was in flight");
    let before = net.clients[0].outbox_stamps();
    net.clients[0].force_resync(delta);
    let expected: Vec<u32> = before
        .iter()
        .map(|t| t.wrapping_add_signed(delta))
        .collect();
    assert_eq!(
        net.clients[0].outbox_stamps(),
        expected,
        "unacknowledged commands follow the relabelled timeline"
    );
}

#[test]
fn timeline_resyncs_mid_script_lose_and_duplicate_no_turn() {
    let _guard = serial();
    // ~12 ticks round trip: every press is unacked for a while after it is stamped.
    let mut net = Net::new(Impairment::new(100, 0, 0.0));
    net.add_client();
    net.join_and_settle();
    let start = net.clients[0].tick() + 30;
    staircase(&mut net.clients[0], start, 24, 16);
    for (presses_left, delta) in [(18, 7), (10, -10), (4, 4)] {
        resync_with_turns_in_flight(&mut net, presses_left, delta);
    }
    finish_script(&mut net, 0);

    let run = Run::collect(&mut net, 0);
    assert!(net.clients[0].trace().resyncs.len() >= 3);
    check_delivery(&mut net, 0, &run, true);
    let rejected: Vec<_> = run
        .log
        .iter()
        .filter(|d| matches!(d.fate, Fate::Rejected(_)))
        .collect();
    assert!(rejected.is_empty(), "no turn rejected: {rejected:?}");
    assert_eq!(run.log.len(), 24, "the rider survived the whole staircase");
    assert_eq!(net.clients[0].turns_unacked(), 0);
}

#[test]
fn redundant_turns_keep_every_press_on_time_at_150ms_and_3_percent_loss() {
    let _guard = serial();
    let mut net = Net::new(Impairment::new(150, 20, 0.03));
    net.add_client();
    net.join_and_settle();
    let start = net.clients[0].tick() + 30;
    staircase(&mut net.clients[0], start, 40, 16);
    finish_script(&mut net, 0);

    let run = Run::collect(&mut net, 0);
    check_delivery_invariants(&mut net, 0, &run);
    let rejected: Vec<_> = run
        .log
        .iter()
        .filter(|d| matches!(d.fate, Fate::Rejected(_)))
        .collect();
    assert!(
        rejected.is_empty(),
        "nothing is rejected as too late: {rejected:?}"
    );
    assert_eq!(run.log.len(), 40, "the rider survived the whole staircase");
    let late = run.log.iter().filter(|d| d.lateness() != Some(0)).count();
    println!("late (executed after the intended tick): {late} of 40");
    // The 4-9 tick margin absorbs jitter and a lost packet (next one is 1 tick behind).
    assert!(late <= 2, "{late} of 40 turns ran late");
    assert_eq!(
        net.clients[0].turns_unacked(),
        0,
        "acks reach the client and empty its outbox"
    );
}

#[test]
fn a_flooding_client_is_disconnected_and_others_are_unaffected() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);
    let honest = net.add_client_with("Honest", |_| {});
    let hostile = net.add_client_with("Hostile", |_| {});
    let ready = net.run_until(Duration::from_secs(15), |net| {
        net.clients.iter().all(|c| c.is_playing())
    });
    assert!(ready, "both clients join");

    // Fresh seqs with a life that does not exist: every command is a rejection.
    let mut seq = 1_000_000;
    let closed = net.run_until(Duration::from_secs(10), |net| {
        let commands = (0..4)
            .map(|_| {
                seq += 1;
                shared::protocol::TurnCommand {
                    seq,
                    direction: shared::protocol::TurnDirection::Left,
                    generation: 0xBAD,
                    tick: 0,
                }
            })
            .collect();
        net.clients[hostile].send_raw_turn_batch(shared::protocol::TurnBatch { commands });
        net.server.link_count() == 1
    });
    assert!(closed, "the flooding link is closed by the server");
    assert_eq!(net.server.human_names(), vec!["Honest".to_string()]);
    net.run_server_ticks(60);
    assert!(net.clients[honest].is_playing());
    // The abuse cost one budget's worth of work, not an unbounded amount.
    assert!(
        net.server.turn_log().0.len() < 500,
        "{} events",
        net.server.turn_log().0.len()
    );
}

#[test]
fn resent_copies_of_one_command_are_free_but_a_message_flood_is_not() {
    let _guard = serial();
    let (mut net, _) = clean_session();
    let generation = net.clients[0].rider_generation().expect("life");
    let command = shared::protocol::TurnCommand {
        seq: 0,
        direction: shared::protocol::TurnDirection::Left,
        generation,
        tick: net.clients[0].tick() + 40,
    };
    let batch = shared::protocol::TurnBatch {
        commands: vec![command; 1],
    };
    // A realistic redundancy burst is ignored after the first copy...
    for _ in 0..20 {
        net.clients[0].send_raw_turn_batch(batch.clone());
        net.step();
    }
    assert_eq!(net.server.link_count(), 1);
    let queued = net
        .server
        .turn_log()
        .0
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                tron_zero_server::turns::TurnEventKind::Queued { .. }
            )
        })
        .count();
    assert_eq!(queued, 1, "one decision for twenty copies");
    // ...while thousands of messages per second are not tolerated.
    let closed = net.run_until(Duration::from_secs(10), |net| {
        for _ in 0..4 {
            net.clients[0].send_raw_turn_batch(batch.clone());
        }
        net.server.link_count() == 0
    });
    assert!(closed, "a duplicate flood closes the link");
}
