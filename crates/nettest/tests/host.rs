//! The server a client can host on its own thread (`tron_zero_server::embedded`):
//! it starts, a client plays on it, it stops cleanly and frees its port, and a
//! busy port is skipped or reported.

// Test crate: unwraps and prints are fine here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr
)]

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use tron_zero_nettest::{Impairment, TestClient, free_port, serial};
use tron_zero_server::embedded::{
    HostConfig, HostHandle, HostMode, HostStatus, Phase, Transport, spawn,
};

const WAIT: Duration = Duration::from_secs(15);

fn host(ports: &[u16]) -> HostHandle {
    spawn(HostConfig {
        transport: Transport::udp_any(ports.iter().copied()),
        mode: HostMode::Play { bots: true },
    })
}

/// Poll until the status is no longer `Starting`.
fn settled(host: &HostHandle) -> HostStatus {
    let end = Instant::now() + WAIT;
    while Instant::now() < end {
        let status = host.status();
        if status != HostStatus::Starting {
            return status;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the host never left Starting");
}

#[test]
fn hosted_server_start_stop() {
    let _guard = serial();
    let port = free_port();
    let host = host(&[port]);
    assert_eq!(settled(&host), HostStatus::Running { port });

    let mut client = TestClient::new(Impairment::NONE);
    client.connect(port);
    let end = Instant::now() + WAIT;
    while !client.is_playing() && Instant::now() < end {
        client.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(client.is_playing(), "the client joins the hosted server");

    host.stop();
    assert_eq!(host.status(), HostStatus::Stopping);
    let end = Instant::now() + WAIT;
    while host.status() != HostStatus::Stopped && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(host.status(), HostStatus::Stopped);
    assert!(host.is_finished());
    drop(host);
    UdpSocket::bind(("0.0.0.0", port)).expect("the port is free again");
}

#[test]
fn a_tutorial_host_places_its_one_player_after_the_countdown() {
    let _guard = serial();
    let port = free_port();
    let host = spawn(HostConfig {
        transport: Transport::udp_loopback([port]),
        mode: HostMode::Tutorial {
            text: tron_zero_server::embedded::TUTORIAL.into(),
        },
    });
    assert_eq!(settled(&host), HostStatus::Running { port });

    let mut client = TestClient::new(Impairment::NONE);
    client.connect(port);
    let end = Instant::now() + WAIT;
    let playing = |host: &HostHandle| host.tutorial().is_some_and(|s| s.phase == Phase::Playing);
    while !playing(&host) && Instant::now() < end {
        client.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    let status = host.tutorial().expect("the tutorial reports");
    assert_eq!(
        (status.phase, status.attempt, status.step),
        (Phase::Playing, 1, 0)
    );
    assert!(!status.prompt.is_empty());
}

/// One step: an inner wall straight ahead of the player.
const WALLED_STEP: &str = r#"
[[step]]
prompt = "Hit the wall."
timeout = 10
solution = []
goal = { kind = "reach", area = [600, 600, 900, 900] }

[step.scene]
walls = [[-300, -200, -300, 200]]
[[step.scene.rider]]
name = "player"
at = [-600, 0]
heading = "+x"
"#;

#[test]
fn a_step_wall_reaches_the_client_and_stops_the_predicted_rider() {
    let _guard = serial();
    let port = free_port();
    let host = spawn(HostConfig {
        transport: Transport::udp_loopback([port]),
        mode: HostMode::Tutorial {
            text: WALLED_STEP.into(),
        },
    });
    assert_eq!(settled(&host), HostStatus::Running { port });

    let mut client = TestClient::new(Impairment::NONE);
    client.connect(port);
    let wall = [-300.0, -200.0, -300.0, 200.0];
    let end = Instant::now() + WAIT;
    let (mut rode, mut furthest) = (false, f32::MIN);
    while Instant::now() < end {
        client.update();
        if client.rider_alive() {
            rode = true;
            furthest = furthest.max(client.rider_position().unwrap().x);
        } else if rode {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(client.walls().contains(&wall), "the step's wall replicates");
    assert!(rode, "the player was placed");
    assert!(!client.rider_alive(), "the wall killed the player");
    // The predicted head closes in on the wall and never goes through it.
    assert!(
        (-310.0..-300.0).contains(&furthest),
        "furthest x {furthest}"
    );
    // The predicted death runs ahead of the server's verdict.
    let failed = |host: &HostHandle| host.tutorial().is_some_and(|s| s.fail.is_some());
    while !failed(&host) && Instant::now() < end {
        client.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    let status = host.tutorial().unwrap();
    assert_eq!(status.fail, Some(tron_zero_server::embedded::Fail::Death));
}

#[test]
fn a_busy_port_fails_or_is_skipped() {
    let _guard = serial();
    let busy = UdpSocket::bind(("0.0.0.0", 0)).unwrap();
    let busy_port = busy.local_addr().unwrap().port();

    let only = host(&[busy_port]);
    assert!(matches!(settled(&only), HostStatus::Failed(_)));
    drop(only);

    let free = free_port();
    let next = host(&[busy_port, free]);
    assert_eq!(settled(&next), HostStatus::Running { port: free });
}
