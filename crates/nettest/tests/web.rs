//! One server, two transports: a native UDP client and a WebTransport client
//! (what a browser uses) join the same game over loopback.

use std::time::Duration;

use glam::Vec2;
use tron_zero_nettest::analysis::{Fate, deliveries};
use tron_zero_nettest::{Impairment, Net, serial, staircase};

const WAIT: Duration = Duration::from_secs(15);

#[test]
fn udp_and_webtransport_clients_share_one_game() {
    let _guard = serial();
    let mut net = Net::new_with_web(Impairment::NONE);
    let udp = net.add_client_with("Udp", |_| {});
    let web = net.add_web_client("Web");

    // Both pass admission and spawn, whatever the transport.
    assert!(
        net.run_until(WAIT, |net| net
            .clients
            .iter_mut()
            .all(|c| c.is_playing() && c.rider_alive())),
        "clients did not reach Playing: udp {:?}, web {:?}",
        net.clients[udp].session().status,
        net.clients[web].session().status
    );
    assert_eq!(net.server.link_count(), 2);
    assert_eq!(net.server.replicating_link_count(), 2);
    assert_eq!(net.server.human_names(), ["Udp", "Web"]);

    // Replication reaches both transports: each sees its own and the other's rider.
    let both = vec!["Udp".to_owned(), "Web".to_owned()];
    assert!(
        net.run_until(WAIT, |net| {
            net.clients[udp].rider_names() == both && net.clients[web].rider_names() == both
        }),
        "riders did not replicate across transports: udp sees {:?}, web sees {:?}",
        net.clients[udp].rider_names(),
        net.clients[web].rider_names()
    );

    // Each rider on its own lane, heading +x, so the staircase stays in the arena.
    for (lane, client) in [udp, web].into_iter().enumerate() {
        let id = net.clients[client].player_id().expect("rider exists");
        let start = Vec2::new(-1000.0, -1000.0 + 700.0 * lane as f32);
        net.server.place_rider(&id, start, Vec2::X);
    }
    net.run_server_ticks(180);

    // Turn commands from the WebTransport client are accepted and executed on time.
    let start = net.clients[web].tick() + 30;
    staircase(&mut net.clients[web], start, 6, 12);
    assert!(
        net.run_until(Duration::from_secs(30), |net| {
            net.clients[web].scripted_remaining() == 0 && net.clients[web].pending_presses() == 0
        }),
        "script did not finish"
    );
    net.run_server_ticks(150);

    let id = net.clients[web].player_id().expect("web rider");
    let (_, link) = net.server.rider(&id).expect("web rider");
    let log = deliveries(&net.server, Some(link));
    assert_eq!(log.len(), 6, "every turn reaches the server");
    assert!(
        log.iter()
            .all(|d| matches!(d.fate, Fate::Applied { .. }) && d.lateness() == Some(0)),
        "turns not applied on time: {log:?}"
    );
    assert!(net.clients[web].rider_alive() && net.clients[udp].rider_alive());
}
