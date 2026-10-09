//! A relay on localhost, with peers registering over QUIC and plain UDP sockets dialing their
//! relay addresses.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::QuicClientConfig;
use quinn::{Connection, ConnectionError, RecvStream, SendStream};
use tempest_net::{Identity, read_relay_message, tls, write_relay_message};
use tempest_protocol::message::GoodbyeReason;
use tempest_protocol::relay::{RELAY_ALPN, Registered, RelayReject, RelayRejectReason};
use tempest_protocol::{Address, PeerId, RelayDatagram, RelayMessage};
use tempest_relay::{Relay, RelayConfig};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(5);

/// A relay serving in the background until dropped.
struct TestRelay {
    relay: Arc<Relay>,
    _stop: oneshot::Sender<()>,
}

impl TestRelay {
    fn start(config: RelayConfig) -> Self {
        let identity = Identity::generate().unwrap();
        let relay = Arc::new(Relay::bind(&identity, config).unwrap());
        let (stop, stopped) = oneshot::channel();
        let serving = relay.clone();
        tokio::spawn(async move {
            serving
                .serve(async {
                    let _ = stopped.await;
                })
                .await;
        });
        Self { relay, _stop: stop }
    }

    fn addr(&self) -> SocketAddr {
        self.relay.local_addr().unwrap()
    }

    async fn wait_for_registrations(&self, count: usize) {
        timeout(WAIT, async {
            while self.relay.registrations() != count {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("registrations settle");
    }
}

/// A peer's relay connection.
struct Client {
    conn: Connection,
    /// Kept open, though nothing more is sent.
    _send: SendStream,
    recv: RecvStream,
}

impl Client {
    /// Connects to `relay` as `identity` and sends Register.
    async fn connect(identity: &Identity, relay: &TestRelay) -> Self {
        let (conn, _) = dial(identity, relay, relay.relay.key()).await.unwrap();
        let (mut send, recv) = conn.open_bi().await.unwrap();
        write_relay_message(&mut send, &RelayMessage::Register)
            .await
            .unwrap();
        Self {
            conn,
            _send: send,
            recv,
        }
    }

    async fn answer(&mut self) -> RelayMessage {
        timeout(WAIT, read_relay_message(&mut self.recv))
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    }

    async fn registered(&mut self) -> Registered {
        match self.answer().await {
            RelayMessage::Registered(registered) => registered,
            other => panic!("expected Registered, got {other:?}"),
        }
    }

    async fn forwarded(&self) -> (Address, Vec<u8>) {
        let datagram = timeout(WAIT, self.conn.read_datagram())
            .await
            .unwrap()
            .unwrap();
        match RelayDatagram::decode(&datagram).unwrap() {
            RelayDatagram::Forwarded { from, packet } => (from, packet.to_vec()),
            other => panic!("expected Forwarded, got {other:?}"),
        }
    }

    fn forward(&self, to: SocketAddr, packet: &[u8]) {
        let datagram = RelayDatagram::Forward {
            to: to.into(),
            packet,
        };
        self.conn.send_datagram(datagram.encode().into()).unwrap();
    }
}

/// Opens a relay connection from a new socket, expecting the relay's key to be `expected`.
/// Returns the connection and the endpoint, which must outlive it.
async fn dial(
    identity: &Identity,
    relay: &TestRelay,
    expected: PeerId,
) -> Result<(Connection, quinn::Endpoint), ConnectionError> {
    let crypto = tls::client_config(identity, RELAY_ALPN, Some(expected));
    let config = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(crypto).unwrap()));
    let endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    let connecting = endpoint
        .connect_with(config, relay.addr(), "relay")
        .unwrap();
    let conn = timeout(WAIT, connecting)
        .await
        .expect("the handshake ends")?;
    Ok((conn, endpoint))
}

async fn udp() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").await.unwrap()
}

async fn receive(socket: &UdpSocket, within: Duration) -> Option<(Vec<u8>, SocketAddr)> {
    let mut buffer = [0; 2048];
    let (len, from) = timeout(within, socket.recv_from(&mut buffer))
        .await
        .ok()?
        .ok()?;
    Some((buffer[..len].to_vec(), from))
}

#[tokio::test]
async fn a_registered_peer_is_reachable_at_its_relay_address() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let mut client = Client::connect(&Identity::generate().unwrap(), &relay).await;
    let registered = client.registered().await;
    let relay_address = registered.relay_address.socket_addr();
    assert_eq!(relay_address.ip(), relay.addr().ip());
    assert_ne!(relay_address.port(), relay.addr().port());
    assert_eq!(relay.relay.registrations(), 1);

    // Anyone can send to the relay address; the packet arrives wrapped, with its source.
    let dialer = udp().await;
    dialer.send_to(b"hello", relay_address).await.unwrap();
    let (from, packet) = client.forwarded().await;
    assert_eq!(from, dialer.local_addr().unwrap().into());
    assert_eq!(packet, b"hello");

    // Replies leave from the relay address.
    client.forward(dialer.local_addr().unwrap(), b"reply");
    let (packet, source) = receive(&dialer, WAIT).await.expect("the reply arrives");
    assert_eq!(packet, b"reply");
    assert_eq!(source, relay_address);
}

#[tokio::test]
async fn the_relay_observes_the_peers_address() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let identity = Identity::generate().unwrap();
    let (conn, endpoint) = dial(&identity, &relay, relay.relay.key()).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_relay_message(&mut send, &RelayMessage::Register)
        .await
        .unwrap();
    let Some(RelayMessage::Registered(registered)) = read_relay_message(&mut recv).await.unwrap()
    else {
        panic!("expected Registered");
    };
    assert_eq!(
        registered.observed_address,
        endpoint.local_addr().unwrap().into()
    );
}

#[tokio::test]
async fn packets_go_only_to_addresses_that_sent_to_the_relay_address() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let mut client = Client::connect(&Identity::generate().unwrap(), &relay).await;
    let relay_address = client.registered().await.relay_address.socket_addr();

    // A witness that may be sent to shows when the relay has handled what came before.
    let witness = udp().await;
    witness.send_to(b"witness", relay_address).await.unwrap();
    client.forwarded().await;

    let stranger = udp().await;
    client.forward(stranger.local_addr().unwrap(), b"unsolicited");
    client.forward(witness.local_addr().unwrap(), b"probe");
    let (probe, _) = receive(&witness, WAIT).await.expect("the probe arrives");
    assert_eq!(probe, b"probe");

    // Once the stranger has sent something, it may be answered. The first packet it gets is
    // the answer: the unsolicited one, handled before, was dropped.
    stranger.send_to(b"hi", relay_address).await.unwrap();
    client.forwarded().await;
    client.forward(stranger.local_addr().unwrap(), b"answer");
    let (packet, _) = receive(&stranger, WAIT).await.expect("the answer arrives");
    assert_eq!(packet, b"answer");
}

#[tokio::test]
async fn only_allowlisted_peers_may_register() {
    let allowed = Identity::generate().unwrap();
    let relay = TestRelay::start(RelayConfig {
        allowlist: Some([allowed.peer_id()].into()),
        ..RelayConfig::localhost()
    });

    let mut stranger = Client::connect(&Identity::generate().unwrap(), &relay).await;
    let answer = stranger.answer().await;
    assert!(
        matches!(
            answer,
            RelayMessage::RelayReject(RelayReject {
                reason: RelayRejectReason::NotAllowed,
                ..
            })
        ),
        "{answer:?}"
    );
    // The relay closes the connection after refusing.
    let closed = timeout(WAIT, stranger.conn.closed()).await.unwrap();
    assert!(matches!(closed, ConnectionError::ApplicationClosed(_)));

    let mut client = Client::connect(&allowed, &relay).await;
    client.registered().await;
}

#[tokio::test]
async fn a_peer_that_reconnects_keeps_its_relay_address() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let identity = Identity::generate().unwrap();
    let mut first = Client::connect(&identity, &relay).await;
    let address = first.registered().await.relay_address;
    first.conn.close(0u32.into(), b"");
    relay.wait_for_registrations(0).await;

    let mut second = Client::connect(&identity, &relay).await;
    assert_eq!(second.registered().await.relay_address, address);

    // Someone else gets another address.
    let mut other = Client::connect(&Identity::generate().unwrap(), &relay).await;
    assert_ne!(other.registered().await.relay_address, address);
}

#[tokio::test]
async fn registering_again_replaces_the_old_connection() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let identity = Identity::generate().unwrap();
    let mut old = Client::connect(&identity, &relay).await;
    let address = old.registered().await.relay_address;

    let mut new = Client::connect(&identity, &relay).await;
    assert_eq!(new.registered().await.relay_address, address);
    let closed = timeout(WAIT, old.conn.closed()).await.unwrap();
    let ConnectionError::ApplicationClosed(close) = closed else {
        panic!("expected a Goodbye, got {closed:?}");
    };
    assert_eq!(
        GoodbyeReason::from_code(close.error_code.into_inner()),
        GoodbyeReason::Duplicate
    );
    relay.wait_for_registrations(1).await;

    // Traffic now reaches the new connection.
    let dialer = udp().await;
    dialer
        .send_to(b"hello", address.socket_addr())
        .await
        .unwrap();
    assert_eq!(new.forwarded().await.1, b"hello");
}

#[tokio::test]
async fn each_ip_address_may_hold_a_limited_number_of_registrations() {
    let relay = TestRelay::start(RelayConfig {
        max_per_ip: 1,
        ..RelayConfig::localhost()
    });
    let mut first = Client::connect(&Identity::generate().unwrap(), &relay).await;
    first.registered().await;
    let mut second = Client::connect(&Identity::generate().unwrap(), &relay).await;
    assert!(matches!(
        second.answer().await,
        RelayMessage::RelayReject(RelayReject {
            reason: RelayRejectReason::Full,
            ..
        })
    ));
}

#[tokio::test]
async fn the_rate_limit_drops_what_exceeds_it() {
    // 1,000 bytes per second allows a burst of 250 bytes.
    let relay = TestRelay::start(RelayConfig {
        rate_limit: Some(1000),
        ..RelayConfig::localhost()
    });
    let mut client = Client::connect(&Identity::generate().unwrap(), &relay).await;
    let relay_address = client.registered().await.relay_address.socket_addr();

    let dialer = udp().await;
    for _ in 0..10 {
        dialer.send_to(&[7; 200], relay_address).await.unwrap();
    }
    // The first fits the burst; then count what follows until the relay falls quiet.
    client.forwarded().await;
    let mut arrived = 1;
    while timeout(Duration::from_secs(1), client.conn.read_datagram())
        .await
        .is_ok()
    {
        arrived += 1;
    }
    assert!(arrived < 10, "{arrived} arrived");
}

#[tokio::test]
async fn a_peer_that_sends_anything_but_register_is_closed() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let identity = Identity::generate().unwrap();
    let (conn, _endpoint) = dial(&identity, &relay, relay.relay.key()).await.unwrap();
    let (mut send, _recv) = conn.open_bi().await.unwrap();
    let registered = Registered {
        relay_address: "192.0.2.1:1".parse().unwrap(),
        observed_address: "192.0.2.1:1".parse().unwrap(),
    };
    write_relay_message(&mut send, &registered.into())
        .await
        .unwrap();
    let closed = timeout(WAIT, conn.closed()).await.unwrap();
    let ConnectionError::ApplicationClosed(close) = closed else {
        panic!("expected a Goodbye, got {closed:?}");
    };
    assert_eq!(
        GoodbyeReason::from_code(close.error_code.into_inner()),
        GoodbyeReason::ProtocolViolation
    );
    assert_eq!(relay.relay.registrations(), 0);
}

#[tokio::test]
async fn a_peer_expecting_another_key_refuses_the_relay() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let identity = Identity::generate().unwrap();
    let wrong = Identity::generate().unwrap().peer_id();
    assert!(dial(&identity, &relay, wrong).await.is_err());
}
