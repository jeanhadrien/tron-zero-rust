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
use tron_zero_server::embedded::{HostConfig, HostHandle, HostStatus, Transport, spawn};

const WAIT: Duration = Duration::from_secs(15);

fn host(ports: &[u16]) -> HostHandle {
    spawn(HostConfig {
        transport: Transport::udp_any(ports.iter().copied()),
        bots: true,
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
