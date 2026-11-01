//! Session lifecycle over real UDP loopback: the hello handshake (name and
//! protocol version), capacity, disconnect -> reconnect cycles, and the
//! client-side latency simulation.

use std::time::Duration;

use tron_zero_client::sim::NetSim;
use tron_zero_nettest::{Impairment, Net, serial};

const WAIT: Duration = Duration::from_secs(15);

fn playing(net: &mut Net, clients: &[usize]) -> bool {
    net.run_until(WAIT, |net| {
        clients
            .iter()
            .all(|&i| net.clients[i].is_playing() && net.clients[i].rider_alive())
    })
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[test]
fn names_are_sanitized_deduplicated_and_replicated_to_everyone() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);
    let a = net.add_client_with("  Neo\u{7} ", |_| {});
    let b = net.add_client_with("neo", |_| {});
    assert!(playing(&mut net, &[a, b]), "both clients join");

    // Whoever the server saw first keeps the plain name; the other is suffixed.
    let accepted = [
        net.clients[a].session().name.clone(),
        net.clients[b].session().name.clone(),
    ];
    assert_ne!(accepted[0], accepted[1]);
    assert!(
        accepted.iter().any(|n| n == "Neo")
            && accepted.iter().any(|n| n == "neo 2" || n == "Neo 2"),
        "got {accepted:?}"
    );
    let mut expected = accepted.to_vec();
    expected.sort();
    assert_eq!(net.server.human_names(), expected);

    // A late joiner receives the existing riders' final names, and everyone
    // receives the late joiner's.
    let c = net.add_client_with("Zed", |_| {});
    assert!(playing(&mut net, &[c]));
    expected.push("Zed".into());
    expected.sort();
    let everyone_sees_all = net.run_until(WAIT, |net| {
        [a, b, c]
            .iter()
            .all(|&i| net.clients[i].rider_names() == expected)
    });
    assert!(everyone_sees_all, "names did not replicate to all clients");
}

#[test]
fn empty_or_overlong_names_fall_back_to_safe_values() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);
    let a = net.add_client_with("", |_| {});
    let b = net.add_client_with("abcdefghijklmnopqrstuvwxyz", |_| {});
    assert!(playing(&mut net, &[a, b]));
    let mut names = net.server.human_names();
    names.sort();
    assert_eq!(names, strings(&["Player", "abcdefghijklmnop"]));
}

#[test]
fn mismatched_protocol_is_rejected_with_a_reason_and_no_rider() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);
    let stale = net.add_client_with("Old", |c| {
        c.set_protocol(shared::protocol::PROTOCOL_ID.wrapping_add(1))
    });
    let rejected = net.run_until(WAIT, |net| {
        net.clients[stale].session().status.contains("Incompatible")
    });
    assert!(
        rejected,
        "client never saw the rejection; status: {}",
        net.clients[stale].session().status
    );
    assert!(
        net.server.player_ids().is_empty(),
        "no rider for a stale client"
    );
    // The client tears the session down cleanly, ready to try another server,
    // and the server drops the link.
    assert!(net.run_until(WAIT, |net| net.clients[stale].phase()
        == tron_zero_client::connection::ConnectionPhase::Offline
        && net.server.link_count() == 0));
    assert!(
        net.clients[stale].session().status.contains("Incompatible"),
        "the reason stays visible after teardown"
    );
}

#[test]
fn full_server_rejects_the_extra_client_clearly() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);
    net.server.set_max_players(1);
    let first = net.add_client_with("First", |_| {});
    assert!(playing(&mut net, &[first]));
    let second = net.add_client_with("Second", |_| {});
    assert!(
        net.run_until(WAIT, |net| net.clients[second]
            .session()
            .status
            .contains("full")),
        "status: {}",
        net.clients[second].session().status
    );
    assert_eq!(net.server.human_names(), strings(&["First"]));
    // The occupant is unaffected.
    net.run_server_ticks(120);
    assert!(net.clients[first].is_playing());
}

#[test]
fn leaving_and_rejoining_repeatedly_cleans_up_and_renames() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);
    let a = net.add_client_with("Alpha", |_| {});
    let b = net.add_client_with("Beta", |_| {});
    assert!(playing(&mut net, &[a, b]));
    let port = net.port();

    for (cycle, name) in ["Alpha 1", "Alpha 2", "Alpha 3"].into_iter().enumerate() {
        net.clients[a].disconnect();
        let gone = net.run_until(WAIT, |net| {
            net.clients[a].phase() == tron_zero_client::connection::ConnectionPhase::Offline
                && net.server.human_names() == strings(&["Beta"])
                && net.server.link_count() == 1
                && net.clients[b].rider_names() == strings(&["Beta"])
        });
        assert!(gone, "cycle {cycle}: rider not cleaned up everywhere");

        net.clients[a].connect_as(port, name);
        assert!(playing(&mut net, &[a]), "cycle {cycle}: rejoin failed");
        let mut expected = strings(&["Beta", name]);
        expected.sort();
        let synced = net.run_until(WAIT, |net| {
            net.server.human_names() == expected && net.clients[b].rider_names() == expected
        });
        assert!(synced, "cycle {cycle}: second client does not see {name}");
        assert_eq!(net.clients[a].session().name, name);
    }
    assert_eq!(net.server.link_count(), 2);
}

#[test]
fn simulated_latency_sets_the_round_trip_not_one_direction() {
    let _guard = serial();
    let rtt = 100;
    let sim = NetSim {
        rtt_ms: rtt,
        jitter_ms: 0,
        loss: 0.0,
    };
    let mut net = Net::new(Impairment::NONE);
    let simulated = net.add_simulated_client(sim);
    let plain = net.add_client();
    assert!(playing(&mut net, &[simulated, plain]));
    net.run_server_ticks(600);

    let measured = net.clients[simulated].link_rtt();
    let baseline = net.clients[plain].link_rtt();
    println!("simulated {measured:?}, baseline {baseline:?}");
    assert!(
        baseline < Duration::from_millis(30),
        "baseline {baseline:?}"
    );
    assert!(
        measured >= Duration::from_millis(rtt * 9 / 10)
            && measured <= Duration::from_millis(rtt + 40),
        "requested {rtt} ms RTT, measured {measured:?}"
    );
}

/// Open `count` UDP sockets that each send one junk datagram to the server:
/// to a raw-UDP server every new source address is a connection.
fn spoofed_peers(net: &Net, count: usize) -> Vec<std::net::UdpSocket> {
    (0..count)
        .map(|_| {
            let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind");
            socket
                .send_to(&[0u8; 4], ("127.0.0.1", net.port()))
                .expect("send");
            socket
        })
        .collect()
}

#[test]
fn nothing_is_replicated_to_a_link_before_its_hello() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);
    let _spoofed = spoofed_peers(&net, 1);
    assert!(
        net.run_until(WAIT, |net| net.server.link_count() == 1),
        "the stray packet creates a link"
    );
    net.run_server_ticks(120);
    assert_eq!(net.server.link_count(), 1);
    assert_eq!(
        net.server.replicating_link_count(),
        0,
        "an unadmitted link is never replicated to"
    );

    // A real client is admitted next to it and only it gets the world.
    let client = net.add_client();
    assert!(playing(&mut net, &[client]));
    assert_eq!(net.server.replicating_link_count(), 1);

    // The silent link is dropped by the hello timeout, the player stays.
    assert!(
        net.run_until(WAIT, |net| net.server.link_count() == 1),
        "the silent link times out"
    );
    assert!(net.clients[client].is_playing());
}

#[test]
fn unadmitted_links_are_capped() {
    let _guard = serial();
    let mut net = Net::new(Impairment::NONE);
    let cap = tron_zero_server::admission::MAX_UNADMITTED_LINKS;
    let _spoofed = spoofed_peers(&net, cap + 12);
    net.run_until(Duration::from_secs(3), |_| false);
    let links = net.server.link_count();
    assert!(
        (cap / 2..=cap).contains(&links),
        "{links} links held for {} would-be peers (cap {cap})",
        cap + 12
    );
    assert_eq!(net.server.replicating_link_count(), 0);
}

#[test]
fn rejection_survives_packet_loss() {
    let _guard = serial();
    // 30% loss each way: before the rejected link was kept open to retransmit,
    // a single lost rejection left that client with a timeout message.
    let mut net = Net::new(Impairment::new(40, 0, 0.3));
    let stale: Vec<usize> = (0..8)
        .map(|_| {
            net.add_client_with("Old", |c| {
                c.set_protocol(shared::protocol::PROTOCOL_ID.wrapping_add(1))
            })
        })
        .collect();
    let all_told = net.run_until(WAIT, |net| {
        stale.iter().all(|&i| {
            net.clients[i].session().status.contains("Incompatible")
                && net.clients[i].phase() == tron_zero_client::connection::ConnectionPhase::Offline
        })
    });
    let statuses: Vec<String> = stale
        .iter()
        .map(|&i| net.clients[i].session().status.clone())
        .collect();
    assert!(all_told, "statuses: {statuses:?}");
    assert!(net.server.player_ids().is_empty());
    assert!(
        net.run_until(WAIT, |net| net.server.link_count() == 0),
        "the server closes the refused links"
    );
}
