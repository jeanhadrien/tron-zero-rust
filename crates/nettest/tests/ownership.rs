//! Per-client ownership markers: whatever the join order, each client sees
//! exactly its own rider as `Predicted` + `Controlled` and every other rider
//! (human or bot) as `Interpolated` only.

// Test crate: unwraps and prints are fine here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr
)]

use std::time::Duration;

use tron_zero_nettest::client::RiderRole;
use tron_zero_nettest::{Impairment, Net, serial};

const WAIT: Duration = Duration::from_secs(15);

fn playing(net: &mut Net, clients: &[usize]) -> bool {
    net.run_until(WAIT, |net| {
        clients
            .iter()
            .all(|&i| net.clients[i].is_playing() && net.clients[i].rider_alive())
    })
}

/// The roles every client should see once `names` are all connected.
fn expected(own: &str, names: &[&str]) -> Vec<RiderRole> {
    let mut roles: Vec<RiderRole> = names
        .iter()
        .map(|&name| {
            let mine = name == own;
            RiderRole {
                name: name.to_owned(),
                predicted: mine,
                controlled: mine,
                interpolated: !mine,
            }
        })
        .collect();
    roles.sort_by(|a, b| a.name.cmp(&b.name));
    roles
}

/// Wait until each listed client sees the expected roles, then check once more
/// after a settling period that nothing flips.
fn assert_roles(net: &mut Net, clients: &[(usize, &str)], names: &[&str]) {
    let settled = net.run_until(WAIT, |net| {
        clients
            .iter()
            .all(|&(i, own)| net.clients[i].rider_roles() == expected(own, names))
    });
    let seen: Vec<_> = clients
        .iter()
        .map(|&(i, own)| (own, net.clients[i].rider_roles()))
        .collect();
    assert!(settled, "roles never settled: {seen:#?}");
    net.run_server_ticks(120);
    for &(i, own) in clients {
        assert_eq!(net.clients[i].rider_roles(), expected(own, names));
        let id = net.clients[i].player_id().expect("own rider");
        assert_eq!(Some(id), net.server.player_id_named(own), "{own}'s id");
    }
}

#[test]
fn staggered_joins_rejoin_and_respawn_keep_ownership_per_client() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);

    let a = net.add_client_with("Ann", |_| {});
    assert!(playing(&mut net, &[a]));
    assert_roles(&mut net, &[(a, "Ann")], &["Ann"]);

    // Joins after Ann's rider already exists.
    net.run_server_ticks(60);
    let b = net.add_client_with("Bob", |_| {});
    assert!(playing(&mut net, &[b]));
    assert_roles(&mut net, &[(a, "Ann"), (b, "Bob")], &["Ann", "Bob"]);

    net.run_server_ticks(60);
    let c = net.add_client_with("Cy", |_| {});
    assert!(playing(&mut net, &[c]));
    let all = [(a, "Ann"), (b, "Bob"), (c, "Cy")];
    assert_roles(&mut net, &all, &["Ann", "Bob", "Cy"]);

    // The first joiner leaves and comes back: it is a late joiner now.
    net.clients[a].disconnect();
    assert!(net.run_until(WAIT, |net| net.server.human_names() == ["Bob", "Cy"]));
    assert_roles(&mut net, &[(b, "Bob"), (c, "Cy")], &["Bob", "Cy"]);
    let port = net.port();
    net.clients[a].connect_as(port, "Ann");
    assert!(playing(&mut net, &[a]));
    assert_roles(&mut net, &all, &["Ann", "Bob", "Cy"]);

    // A death and respawn changes nobody's role.
    let bob = net.server.player_id_named("Bob").expect("Bob's rider");
    net.server.kill_rider(&bob);
    assert!(net.run_until(WAIT, |net| !net.clients[b].rider_alive()));
    assert_roles(&mut net, &all, &["Ann", "Bob", "Cy"]);
}

#[test]
fn simultaneous_udp_joins_keep_ownership() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);
    let a = net.add_client_with("Ann", |_| {});
    let b = net.add_client_with("Bob", |_| {});
    let c = net.add_client_with("Cy", |_| {});
    assert!(playing(&mut net, &[a, b, c]));
    assert_roles(
        &mut net,
        &[(a, "Ann"), (b, "Bob"), (c, "Cy")],
        &["Ann", "Bob", "Cy"],
    );
}

#[test]
fn simultaneous_joins_over_both_transports_keep_ownership() {
    for web_first in [false, true] {
        let _guard = serial();
        let mut net = Net::new_with_web(Impairment::NONE);
        let (udp, web) = if web_first {
            let web = net.add_web_client("Web");
            (net.add_client_with("Udp", |_| {}), web)
        } else {
            let udp = net.add_client_with("Udp", |_| {});
            (udp, net.add_web_client("Web"))
        };
        assert!(playing(&mut net, &[udp, web]));
        assert_roles(&mut net, &[(udp, "Udp"), (web, "Web")], &["Udp", "Web"]);
    }
}

#[test]
fn udp_and_webtransport_clients_each_own_only_their_rider() {
    let _guard = serial();
    let mut net = Net::new_with_web(Impairment::NONE);
    let udp = net.add_client_with("Udp", |_| {});
    assert!(playing(&mut net, &[udp]));
    let web = net.add_web_client("Web");
    assert!(playing(&mut net, &[web]));
    assert_roles(&mut net, &[(udp, "Udp"), (web, "Web")], &["Udp", "Web"]);
}
