//! Measurements behind the turn scheduling policy. Run with
//! `cargo test -p tron-zero-nettest --test measure -- --ignored --nocapture`.

use tron_zero_nettest::analysis::{Distribution, Fate, deliveries};
use tron_zero_nettest::{Impairment, Net, serial, staircase};

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
        "  pressed {} / sent {} / logged {} / rejected {} / rollbacks {} / rider alive {}",
        net.clients[0].injected_at.len(),
        net.clients[0].turns_sent(),
        log.len(),
        rejected,
        net.clients[0].rollbacks() - rollbacks_before,
        net.clients[0].rider_alive()
    );
}

#[test]
#[ignore = "prints measurements; takes about a minute"]
fn arrival_offsets_by_profile() {
    let _guard = serial();
    for impairment in [
        Impairment::NONE,
        Impairment::new(50, 10, 0.0),
        Impairment::new(150, 20, 0.0),
        Impairment::new(150, 20, 0.03),
        Impairment::new(100, 30, 0.05),
        Impairment::new(250, 50, 0.0),
    ] {
        measure(impairment, 60);
    }
}
