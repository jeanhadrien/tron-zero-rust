//! Rounds mode over real UDP loopback: a lone rider rides free, a second one
//! starts a match, a round ends with a winner, the dead cannot respawn, the
//! next round places everybody again and the last one ends the match.

// Test crate: unwraps and prints are fine here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr
)]

use std::time::Duration;

use bevy::input::keyboard::KeyCode;
use shared::protocol::RespawnOutcome;
use shared::{EndRules, MatchPhase, MatchRules, Respawn};
use tron_zero_nettest::{Net, serial};

const WAIT: Duration = Duration::from_secs(20);

/// Two rounds, no time limit, half-second breaks.
fn rules() -> MatchRules {
    MatchRules {
        respawn: Respawn::Never,
        end: EndRules {
            time_limit: None,
            last_standing: true,
        },
        rounds: 2,
        intermission: 60,
        final_intermission: 60,
    }
}

fn phase(net: &mut Net, client: usize) -> Option<(MatchPhase, u32)> {
    net.clients[client]
        .match_status()
        .map(|s| (s.phase, s.round))
}

#[test]
fn a_match_runs_from_the_first_round_to_the_standings_and_a_new_match() {
    let _guard = serial();
    let mut net = Net::new_rounds(rules());
    let ann = net.add_client_with("Ann", |_| {});
    let free = net.run_until(WAIT, |net| {
        net.clients[ann].is_playing()
            && net.clients[ann].rider_alive()
            && phase(net, ann) == Some((MatchPhase::Free, 0))
    });
    assert!(free, "a lone rider rides free");
    assert_eq!(net.clients[ann].match_status().unwrap().rounds, 2);

    // A second rider starts the match; both end up placed in round 1.
    let bob = net.add_client_with("Bob", |_| {});
    let started = net.run_until(WAIT, |net| {
        [ann, bob].iter().all(|&i| {
            net.clients[i].rider_alive() && phase(net, i) == Some((MatchPhase::Playing, 1))
        })
    });
    assert!(started, "round 1 starts with both riders alive");
    let life_before = net.clients[ann].rider_generation().unwrap();

    // Bob crashes: Ann wins round 1 and both see it.
    let bob_id = net.clients[bob].player_id().unwrap();
    net.server.kill_rider(&bob_id);
    let won = net.run_until(WAIT, |net| {
        [ann, bob].iter().all(|&i| {
            net.clients[i].match_status().is_some_and(|s| {
                s.phase == MatchPhase::Intermission
                    && s.winner.as_deref() == Some("Ann")
                    && s.standings.first() == Some(&("Ann".to_owned(), 1))
            })
        })
    });
    assert!(won, "{:?}", net.clients[ann].match_status());

    // Dead riders cannot respawn in the middle of a match.
    net.clients[bob].press(KeyCode::Space);
    let refused = net.run_until(WAIT, |net| {
        net.clients[bob].respawn_outcome() == Some(RespawnOutcome::WaitForNextRound)
            || phase(net, bob) == Some((MatchPhase::Playing, 2))
    });
    assert!(refused);

    // Round 2: everybody is back, in a new life.
    let second = net.run_until(WAIT, |net| {
        [ann, bob].iter().all(|&i| {
            net.clients[i].rider_alive() && phase(net, i) == Some((MatchPhase::Playing, 2))
        })
    });
    assert!(second, "round 2 places both riders");
    assert!(net.clients[ann].rider_generation().unwrap() > life_before);

    // Last round: Ann crashes, Bob takes it, the match is over with 1-1.
    let ann_id = net.clients[ann].player_id().unwrap();
    net.server.kill_rider(&ann_id);
    let over = net.run_until(WAIT, |net| {
        net.clients[bob].match_status().is_some_and(|s| {
            s.phase == MatchPhase::MatchOver
                && s.winner.as_deref() == Some("Bob")
                && s.standings.iter().all(|(_, wins)| *wins == 1)
        })
    });
    assert!(over, "{:?}", net.clients[bob].match_status());

    // A new match starts with the scores back at zero.
    let again = net.run_until(WAIT, |net| {
        net.clients[bob].rider_alive()
            && net.clients[bob].match_status().is_some_and(|s| {
                s.phase == MatchPhase::Playing
                    && s.round == 1
                    && s.standings.iter().all(|(_, wins)| *wins == 0)
            })
    });
    assert!(again, "{:?}", net.clients[bob].match_status());
    assert_eq!(net.server.link_count(), 2);
}
