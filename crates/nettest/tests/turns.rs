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
    let sent = net.clients[client].turns_sent() as usize;
    assert_eq!(run.log.len(), sent, "every sent command reaches the server");
    let client_turns = turns_of(&net.clients[client].trace().first_input);
    assert_eq!(client_turns.len(), sent, "one predicted turn per command");

    let mut last_tick = None;
    let mut applied = Vec::new();
    for (i, delivery) in run.log.iter().enumerate() {
        assert_eq!(delivery.seq, i as u64, "sequence has no gaps");
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
        "no tick ran twice outside rollback"
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
        // The native Lightyear input history agrees on the tick (shared timeline).
        let Fate::Applied { tick } = delivery.fate else {
            unreachable!()
        };
        let native = net.server.trace().0[&(id.clone(), tick)].native;
        assert!(
            matches!(
                native,
                Some(PlayerInput::TurnLeftFor(_) | PlayerInput::TurnRightFor(_))
            ),
            "native input history at tick {tick} is {native:?}"
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
    assert!(on_time * 10 >= run.log.len() * 8, "most turns are on time");
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
