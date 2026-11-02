//! WebTransport listener for browser players.
//!
//! Browsers cannot speak raw UDP, so the server also accepts WebTransport
//! sessions (QUIC datagrams, still UDP underneath) on a second port. Both
//! transports feed the **same** Lightyear server entity: a WebTransport
//! session becomes a `LinkOf` child of it exactly like a UDP "connection", so
//! `RawConnectionPlugin` makes it `Connected` + `ClientOf` and everything
//! downstream (admission, sessions, turns, budgets, replication) applies
//! unchanged. Two server entities would not work: `Replicate::to_clients`
//! resolves its targets through a single *started* `Server` entity.
//!
//! The listener is deliberately not started through lightyear's
//! `WebTransportServerIo`: that component binds to the server entity's single
//! `LocalAddr` (taken by the UDP port) and mirrors the endpoint's lifecycle
//! onto the server entity, so a listener that fails to bind would unlink the
//! UDP server too. Here a failed listener only logs.
//!
//! Certificates: browsers accept `serverCertificateHashes` only for ECDSA
//! P-256 certificates valid for at most 14 days, so without `WEB_CERT` /
//! `WEB_KEY` the server mints such a certificate at startup and publishes its
//! SHA-256 digest; it exits cleanly after [`WebConfig::max_uptime`] so the
//! restart policy brings it back with a fresh one.

use aeronet_io::Session;
use aeronet_io::connection::PeerAddr;
use aeronet_io::server::Closed;
use aeronet_webtransport::server::{ServerConfig, WebTransportServer, WebTransportServerClient};
use aeronet_webtransport::wtransport::Identity;
use aeronet_webtransport::wtransport::tls::Sha256DigestFmt;
use bevy::prelude::*;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use lightyear::prelude::{Link, LinkOf};
use lightyear_aeronet::AeronetLinkOf;
use std::path::PathBuf;
use std::time::Duration;

/// Default UDP port of the WebTransport listener (`WEB_PORT`).
pub const DEFAULT_WEB_PORT: u16 = 5001;
/// Self-signed certificates live 14 days (the browser maximum, started one
/// hour in the past to absorb clock skew); restart a day before they expire.
pub const DEFAULT_MAX_UPTIME_HOURS: u64 = 13 * 24;
/// QUIC keep-alive pings the server sends so NAT mappings and the idle timer
/// stay alive while a (backgrounded) browser tab sends nothing.
const KEEP_ALIVE: Duration = Duration::from_secs(1);
/// A WebTransport session with no packet for this long is dropped. The
/// effective timeout is the smaller of both peers' values.
const MAX_IDLE: Duration = Duration::from_secs(10);

/// Where the WebTransport certificate comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertSource {
    /// Mint a short-lived ECDSA P-256 certificate at startup.
    SelfSigned,
    /// PEM certificate chain and private key (e.g. Let's Encrypt).
    PemFiles { cert: PathBuf, key: PathBuf },
}

/// Environment-driven listener settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConfig {
    pub port: u16,
    /// Port advertised to the lobby (differs from `port` behind port mapping).
    pub advertised_port: u16,
    pub cert: CertSource,
    /// Exit after this long when self-signed (certificate rotation).
    pub max_uptime: Duration,
}

impl WebConfig {
    /// `WEB_PORT` (default 5001, `0` disables the listener), `WEB_CERT` +
    /// `WEB_KEY`, `ADVERTISED_WEB_PORT`, `WEB_MAX_UPTIME_HOURS`.
    pub fn from_env() -> Option<Self> {
        Self::from_map(&|key| std::env::var(key).ok())
    }

    /// Test seam: read config from any key-value lookup.
    pub fn from_map(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let value = |key: &str| {
            get(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let port = match value("WEB_PORT") {
            None => DEFAULT_WEB_PORT,
            Some(raw) => raw.parse().unwrap_or_else(|_| {
                tracing::warn!(%raw, fallback = DEFAULT_WEB_PORT, "Invalid WEB_PORT");
                DEFAULT_WEB_PORT
            }),
        };
        if port == 0 {
            return None;
        }
        let advertised_port = value("ADVERTISED_WEB_PORT")
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(port);
        let cert = match (value("WEB_CERT"), value("WEB_KEY")) {
            (Some(cert), Some(key)) => CertSource::PemFiles {
                cert: cert.into(),
                key: key.into(),
            },
            (None, None) => CertSource::SelfSigned,
            _ => {
                tracing::warn!(
                    "WEB_CERT and WEB_KEY must be set together; using a self-signed certificate"
                );
                CertSource::SelfSigned
            }
        };
        let max_uptime_hours = value("WEB_MAX_UPTIME_HOURS")
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(DEFAULT_MAX_UPTIME_HOURS);
        Some(Self {
            port,
            advertised_port,
            cert,
            max_uptime: Duration::from_secs(max_uptime_hours * 3600),
        })
    }
}

/// A ready-to-open listener: the certificate plus what clients need to trust it.
#[derive(Resource)]
pub struct WebEndpoint {
    pub port: u16,
    identity: Identity,
    /// Lowercase hex SHA-256 of the certificate (no colons), the value for
    /// the browser's `serverCertificateHashes`. `None` for a CA-issued
    /// certificate, which browsers verify without a hash.
    pub cert_hash: Option<String>,
    /// Exit after this long, if the certificate is self-signed.
    pub restart_after: Option<Duration>,
}

impl WebEndpoint {
    /// Load or mint the certificate described by `config`.
    pub fn new(config: &WebConfig) -> Result<Self, String> {
        let (identity, cert_hash, restart_after) = match &config.cert {
            CertSource::SelfSigned => {
                let identity = self_signed_identity()?;
                let hash = cert_hash(&identity);
                (identity, Some(hash), Some(config.max_uptime))
            }
            CertSource::PemFiles { cert, key } => (load_pem_identity(cert, key)?, None, None),
        };
        Ok(Self {
            port: config.port,
            identity,
            cert_hash,
            restart_after,
        })
    }

    /// Test seam: a listener on `port` with a fresh self-signed certificate.
    pub fn self_signed(port: u16) -> Result<Self, String> {
        let identity = self_signed_identity()?;
        Ok(Self {
            port,
            cert_hash: Some(cert_hash(&identity)),
            identity,
            restart_after: None,
        })
    }
}

/// ECDSA P-256 certificate, 14 days from one hour ago (never longer: browsers
/// reject `serverCertificateHashes` certificates valid for more than 14 days).
fn self_signed_identity() -> Result<Identity, String> {
    let not_before = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    Identity::self_signed_builder()
        .subject_alt_names(["localhost", "127.0.0.1", "::1"])
        .validity_period(not_before, not_before + time::Duration::days(14))
        .build()
        .map_err(|e| format!("self-signed certificate: {e}"))
}

fn load_pem_identity(cert: &PathBuf, key: &PathBuf) -> Result<Identity, String> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(Identity::load_pemfiles(cert, key))
        .map_err(|e| format!("loading WEB_CERT/WEB_KEY: {e}"))
}

/// SHA-256 digest of the leaf certificate, lowercase hex without colons.
fn cert_hash(identity: &Identity) -> String {
    let leaf = &identity.certificate_chain().as_slice()[0];
    leaf.hash().fmt(Sha256DigestFmt::DottedHex).replace(':', "")
}

/// Marks the aeronet listener entity and the game server entity it feeds.
#[derive(Component)]
pub struct WebListenerOf(pub Entity);

/// Open the WebTransport listener, feeding sessions into `server`'s links.
pub fn open_listener(commands: &mut Commands, server: Entity, endpoint: &WebEndpoint) {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), endpoint.port);
    let config = ServerConfig::builder()
        .with_bind_address(address)
        .with_identity(endpoint.identity.clone_identity())
        .keep_alive_interval(Some(KEEP_ALIVE))
        .max_idle_timeout(Some(MAX_IDLE))
        .expect("a valid idle timeout")
        .build();
    info!(%address, cert_hash = ?endpoint.cert_hash, "WebTransport listener starting");
    commands
        .spawn((WebListenerOf(server), Name::new("WebTransportListener")))
        .queue(WebTransportServer::open(config));
}

/// Plugin-less wiring: observers that turn sessions into game links.
pub fn add_observers(app: &mut App) {
    app.add_observer(on_session);
    app.add_observer(on_listener_closed);
}

/// A new WebTransport session gets a `Link` child of the game server, the same
/// shape the UDP transport creates; `RawConnectionPlugin` does the rest once
/// the session is up (`Linked`).
fn on_session(
    trigger: On<Add, Session>,
    sessions: Query<(&ChildOf, &PeerAddr), With<WebTransportServerClient>>,
    listeners: Query<&WebListenerOf>,
    mut commands: Commands,
) {
    let Ok((parent, peer)) = sessions.get(trigger.entity) else {
        return;
    };
    let Ok(listener) = listeners.get(parent.parent()) else {
        return;
    };
    let link = commands
        .spawn((
            LinkOf { server: listener.0 },
            Link::new(None),
            PeerAddr(peer.0),
            Name::new("WebTransportClientOf"),
        ))
        .id();
    commands.entity(trigger.entity).insert(AeronetLinkOf(link));
}

/// A listener that fails (e.g. port in use) only costs the web transport.
fn on_listener_closed(trigger: On<Closed>, listeners: Query<(), With<WebListenerOf>>) {
    if listeners.contains(trigger.entity) {
        error!(reason = ?trigger.reason, "WebTransport listener closed; browser players cannot join");
    }
}

/// Ends the app after the configured uptime (certificate rotation).
#[derive(Resource)]
pub struct RestartAfter(pub Duration);

/// Request a clean exit once [`RestartAfter`] has elapsed.
pub fn exit_after_uptime(
    limit: Res<RestartAfter>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
) {
    if time.elapsed() >= limit.0 {
        info!("Max uptime reached (self-signed certificate rotation); exiting for a restart");
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> Option<WebConfig> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        WebConfig::from_map(&|key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
    }

    #[test]
    fn defaults_to_self_signed_on_5001_with_13_days() {
        let cfg = config(&[]).unwrap();
        assert_eq!(cfg.port, 5001);
        assert_eq!(cfg.advertised_port, 5001);
        assert_eq!(cfg.cert, CertSource::SelfSigned);
        assert_eq!(cfg.max_uptime, Duration::from_secs(13 * 24 * 3600));
    }

    #[test]
    fn port_zero_disables_and_pem_needs_both_files() {
        assert!(config(&[("WEB_PORT", "0")]).is_none());
        let half = config(&[("WEB_CERT", "c.pem")]).unwrap();
        assert_eq!(half.cert, CertSource::SelfSigned);
        let both = config(&[("WEB_CERT", "c.pem"), ("WEB_KEY", "k.pem")]).unwrap();
        assert!(matches!(both.cert, CertSource::PemFiles { .. }));
    }

    #[test]
    fn self_signed_endpoint_publishes_a_hex_digest() {
        let endpoint = WebEndpoint::self_signed(5001).unwrap();
        let hash = endpoint.cert_hash.unwrap();
        assert_eq!(hash.len(), 64);
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }
}
