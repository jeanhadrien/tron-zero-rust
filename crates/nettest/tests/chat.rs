//! Player chat over real UDP loopback: typed lines reach every player with the
//! server-known sender name, bad input is dropped, floods are bounded.

// Test crate: unwraps and prints are fine here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr
)]

use std::time::Duration;

use tron_zero_nettest::{Impairment, Net, serial};

const WAIT: Duration = Duration::from_secs(15);

fn two_players() -> (Net, usize, usize) {
    let mut net = Net::new(Impairment::NONE);
    let a = net.add_client_with("Ann", |_| {});
    let b = net.add_client_with("Bob", |_| {});
    let ready = net.run_until(WAIT, |net| net.clients.iter().all(|c| c.is_playing()));
    assert!(ready, "both clients join");
    (net, a, b)
}

#[test]
fn a_typed_line_reaches_everyone_with_the_senders_name() {
    let _guard = serial();
    let (mut net, a, b) = two_players();
    net.clients[a].say("héllo ; wörld ù$");
    let delivered = net.run_until(WAIT, |net| {
        [a, b]
            .iter()
            .all(|&i| net.clients[i].chat_lines() == ["Ann: héllo ; wörld ù$"])
    });
    assert!(delivered, "line not delivered to both players");
}

#[test]
fn typing_does_not_steer_the_rider() {
    let _guard = serial();
    let (mut net, a, _) = two_players();
    net.clients[a].say("qk");
    assert_eq!(net.clients[a].pending_presses(), 0);
}

#[test]
fn unprintable_and_empty_messages_are_dropped_and_names_cannot_be_forged() {
    let _guard = serial();
    let (mut net, a, b) = two_players();
    net.clients[a].send_raw_chat("\u{202E}\u{200B}  \u{7}");
    net.clients[a].send_raw_chat("");
    net.run_server_ticks(30);
    net.clients[a].send_raw_chat("real\u{202E} message");
    let delivered = net.run_until(WAIT, |net| {
        net.clients[b].chat_lines() == ["Ann: real message"]
    });
    assert!(delivered, "got {:?}", net.clients[b].chat_lines());
}

#[test]
fn a_chat_burst_is_rate_limited_without_dropping_the_link() {
    let _guard = serial();
    let (mut net, a, b) = two_players();
    for i in 0..12 {
        net.clients[a].send_raw_chat(&format!("m{i}"));
    }
    net.run_server_ticks(60);
    let received = net.clients[b].chat_lines().len();
    assert!(
        (1..12).contains(&received),
        "{received} lines: the burst is capped, not dropped entirely"
    );
    assert_eq!(net.server.link_count(), 2, "nobody is disconnected");
}
