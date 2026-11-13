//! Rider look over real UDP loopback: a look chosen before joining or while
//! playing reaches the server, the owner's own predicted rider and every other
//! client; chat lines carry the sender color; a look set on the server for a
//! non-human rider reaches clients.

// Test crate: unwraps and prints are fine here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr
)]

use std::time::Duration;

use bevy::input::keyboard::KeyCode;
use shared::RiderShape;
use tron_zero_nettest::{Impairment, Net, serial};

const WAIT: Duration = Duration::from_secs(15);

const ANN: (u32, RiderShape) = (0xFF3300, RiderShape::Hexagon);
const BOB: (u32, RiderShape) = (0x2255FF, RiderShape::Chevron);

fn two_players(ann: (u32, RiderShape), bob: (u32, RiderShape)) -> (Net, usize, usize) {
    let mut net = Net::new(Impairment::NONE);
    let a = net.add_client_with("Ann", |c| c.set_look(ann.0, ann.1));
    let b = net.add_client_with("Bob", |c| c.set_look(bob.0, bob.1));
    let ready = net.run_until(WAIT, |net| {
        net.clients
            .iter_mut()
            .all(|c| c.is_playing() && c.rider_alive())
    });
    assert!(ready, "both clients join");
    (net, a, b)
}

/// Every client sees both riders as given, and each one's own predicted rider
/// (client 0 is Ann, client 1 is Bob) carries its owner's look too.
fn sees(net: &mut Net, ann: (u32, RiderShape), bob: (u32, RiderShape)) -> bool {
    net.clients[0].own_look() == Some(ann)
        && net.clients[1].own_look() == Some(bob)
        && net
            .clients
            .iter_mut()
            .all(|c| c.look_of("Ann") == Some(ann) && c.look_of("Bob") == Some(bob))
}

/// Wait until every client sees both riders with the expected looks.
fn all_see(net: &mut Net, ann: (u32, RiderShape), bob: (u32, RiderShape)) -> bool {
    net.run_until(WAIT, |net| sees(net, ann, bob))
}

#[test]
fn a_look_chosen_before_joining_is_seen_by_server_self_and_others() {
    let _guard = serial();
    let (mut net, a, b) = two_players(ANN, BOB);
    assert!(
        all_see(&mut net, ANN, BOB),
        "ann sees {:?}/{:?}, bob sees {:?}/{:?}",
        net.clients[a].look_of("Ann"),
        net.clients[a].look_of("Bob"),
        net.clients[b].look_of("Ann"),
        net.clients[b].look_of("Bob"),
    );
    assert_eq!(net.server.look_of("Ann"), Some(ANN));
    assert_eq!(net.server.look_of("Bob"), Some(BOB));
}

#[test]
fn a_live_change_reaches_the_server_the_own_predicted_rider_and_others() {
    let _guard = serial();
    let (mut net, a, _) = two_players(ANN, BOB);
    assert!(all_see(&mut net, ANN, BOB));

    let changed = (0x10EE80, RiderShape::Diamond);
    net.clients[a].set_look(changed.0, changed.1);
    // `own_look` reads Ann's own Predicted + Controlled rider, the one the
    // renderer draws for her; `look_of` reads whichever rider carries her name.
    let propagated = net.run_until(WAIT, |net| sees(net, changed, BOB));
    assert!(
        propagated,
        "ann's own rider {:?}, ann sees {:?}, bob sees {:?}, server has {:?}",
        net.clients[a].own_look(),
        net.clients[a].look_of("Ann"),
        net.clients[1].look_of("Ann"),
        net.server.look_of("Ann"),
    );
    assert_eq!(net.server.look_of("Ann"), Some(changed));
    assert_eq!(net.server.look_of("Bob"), Some(BOB), "Bob is untouched");
}

#[test]
fn a_death_and_respawn_keep_the_look() {
    let _guard = serial();
    let (mut net, a, _) = two_players(ANN, BOB);
    assert!(all_see(&mut net, ANN, BOB));
    let id = net.server.player_id_named("Ann").expect("Ann's rider");
    let old_generation = net.clients[a].rider_generation().expect("life");
    net.server.kill_rider(&id);
    assert!(net.run_until(WAIT, |net| !net.clients[a].rider_alive()));
    // Respawn with Space (retry while dead, like a human would).
    let respawned = net.run_until(WAIT, |net| {
        let client = &mut net.clients[a];
        if !client.rider_alive() {
            client.press(KeyCode::Space);
        }
        client.rider_alive() && client.rider_generation() != Some(old_generation)
    });
    assert!(respawned, "respawn accepted");
    assert_eq!(net.server.look_of("Ann"), Some(ANN));
    assert!(all_see(&mut net, ANN, BOB));
}

#[test]
fn chat_lines_carry_the_sender_color() {
    let _guard = serial();
    let (mut net, a, b) = two_players(ANN, BOB);
    assert!(all_see(&mut net, ANN, BOB));
    net.clients[a].say("hi");
    let delivered = net.run_until(WAIT, |net| {
        [a, b]
            .iter()
            .all(|&i| net.clients[i].chat_colors() == [ANN.0])
    });
    assert!(delivered, "ann's line not tinted with her color");
}

#[test]
fn a_server_side_rider_look_reaches_every_client() {
    let _guard = serial();
    let (mut net, a, b) = two_players(ANN, BOB);
    // Non-default on both axes: the clients cannot guess it locally.
    let botty = (0x123456, RiderShape::Hexagon);
    net.server.spawn_bot_rider("Botty", botty.0, botty.1);
    let seen = net.run_until(WAIT, |net| {
        [a, b]
            .iter()
            .all(|&i| net.clients[i].look_of("Botty") == Some(botty))
    });
    assert!(seen, "server-side rider look not replicated");
}
