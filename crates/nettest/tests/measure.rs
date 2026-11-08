//! Measurements behind the turn scheduling policy. Run with
//! `cargo test -p tron-zero-nettest --test measure -- --ignored --nocapture`.

use tron_zero_nettest::analysis::{Distribution, Fate, deliveries};
use tron_zero_nettest::{Impairment, Net, serial, staircase};

/// Presses per profile: the staircase must stay inside the arena (a dead rider
/// sends nothing useful), which 40 steps of 16 ticks do from the lower left.
const TURNS: usize = 40;

/// One scripted run: `turns` presses 16 ticks apart. Prints, per profile, how
/// early (negative) or late commands arrive relative to their intended tick,
/// how late they executed, and what the client had to correct.
fn measure(impairment: Impairment, turns: usize) {
    let mut net = Net::new(impairment);
    net.add_client();
    net.join_and_settle();
    let rollbacks_before = net.clients[0].rollbacks();
    let start = net.clients[0].tick() + 30;
    staircase(&mut net.clients[0], start, turns, 16);
    let done = net.run_until(std::time::Duration::from_secs(60), |net| {
        net.clients[0].scripted_remaining() == 0 && net.clients[0].pending_presses() == 0
    });
    assert!(done);
    net.run_server_ticks(240);

    let log = deliveries(&net.server, None);
    let offsets: Vec<i64> = log.iter().filter_map(|d| d.arrival_offset()).collect();
    let lateness: Vec<i64> = log.iter().filter_map(|d| d.lateness()).collect();
    let press_stamp: Vec<i64> = log
        .iter()
        .zip(&net.clients[0].injected_at)
        .map(|(d, pressed)| i64::from(d.received) - i64::from(*pressed))
        .collect();
    let late = log
        .iter()
        .filter(|d| d.arrival_offset().is_some_and(|offset| offset >= 0))
        .count();
    let rejected = log
        .iter()
        .filter(|d| matches!(d.fate, Fate::Rejected(_)))
        .count();
    println!("--- {impairment} ---");
    println!(
        "  receipt - intended tick (negative = early): {}",
        Distribution::of(offsets)
    );
    println!(
        "  receipt - press-time tick (old stamp):      {}",
        Distribution::of(press_stamp)
    );
    println!(
        "  executed - intended tick:                   {}",
        Distribution::of(lateness)
    );
    println!(
        "  pressed {} / sent {} / logged {} / late {} / rejected {} / rollbacks {} / rider alive {}",
        net.clients[0].injected_at.len(),
        net.clients[0].turns_sent(),
        log.len(),
        late,
        rejected,
        net.clients[0].rollbacks() - rollbacks_before,
        net.clients[0].rider_alive()
    );
}

/// Server → client bandwidth while two riders lay long staircase trails (one
/// corner every 16 ticks, so each trail grows to ~40 corners). Prints the mean
/// payload rate per client link over the staircase.
#[test]
#[ignore = "prints measurements; takes about fifteen seconds"]
fn downlink_bandwidth_with_long_trails() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);
    net.add_client();
    net.add_client();
    net.join_and_settle();
    let start = net.clients[0].tick() + 30;
    for client in &mut net.clients {
        staircase(client, start, TURNS, 16);
    }
    // Skip the lead-in; measure from the first corner to the last.
    while net.clients[0].tick() < start {
        net.step();
    }
    let (bytes_before, tick_before) = (net.server.sent_bytes(), net.server.tick());
    let done = net.run_until(std::time::Duration::from_secs(30), |net| {
        net.clients
            .iter()
            .all(|c| c.scripted_remaining() == 0 && c.pending_presses() == 0)
    });
    assert!(done);
    let bytes = net.server.sent_bytes() - bytes_before;
    let secs = f64::from(net.server.tick() - tick_before) / shared::TICK_HZ;
    let links = net.clients.len() as f64;
    println!(
        "  downlink per client: {:.1} KB/s ({} bytes over {secs:.2} s, {} links), riders alive {:?}",
        bytes as f64 / secs / links / 1000.0,
        bytes,
        net.clients.len(),
        net.clients
            .iter_mut()
            .map(|c| c.rider_alive())
            .collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "prints measurements; takes about a minute"]
fn arrival_offsets_by_profile() {
    let _guard = serial();
    for impairment in [
        Impairment::NONE,
        Impairment::new(0, 5, 0.0),
        Impairment::new(50, 10, 0.0),
        Impairment::new(150, 20, 0.0),
        // The lossy rows are random: three runs of the 3% profile.
        Impairment::new(150, 20, 0.03),
        Impairment::new(150, 20, 0.03),
        Impairment::new(150, 20, 0.03),
        Impairment::new(100, 30, 0.05),
        Impairment::new(250, 50, 0.0),
    ] {
        measure(impairment, TURNS);
    }
}
