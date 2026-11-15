//! A headless human that joins a running server and never plays.
//!
//! The server keeps its bots off while no human is connected, so profiling the
//! server binary (`scripts/profile.ps1 server`) needs one. Usage:
//! `idle_human [port]` (default 5000); runs until killed.

#![allow(clippy::unwrap_used, clippy::expect_used)] // dev tool: fail fast on bad input

use std::time::Duration;

use tron_zero_nettest::{Impairment, TestClient};

fn main() {
    let port = std::env::args()
        .nth(1)
        .map_or(5000, |p| p.parse().expect("port is a number"));
    let mut client = TestClient::new(Impairment::NONE);
    client.connect_as(port, "Idle");
    loop {
        client.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}
