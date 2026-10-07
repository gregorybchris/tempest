//! Port mapping against a fake router on localhost that speaks PCP or only NAT-PMP, and one
//! manual test against the real router.

use std::fs;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use tempest_net::{
    AnnounceConfig, Announcer, Endpoint, EndpointConfig, Identity, MappingError, MappingProtocol,
    Node, NodeConfig, PortMappingConfig, PortMappingStatus, Storage, StorageConfig, TrackerClient,
};
use tempest_protocol::tracker::{AnnounceEvent, AnnounceRequest};
use tempest_protocol::{Address, Entry, FileInfo, Manifest, PeerId, tree};
use tempest_tracker::{Tracker, TrackerConfig};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{mpsc, oneshot, watch};

const TIMEOUT: Duration = Duration::from_secs(30);

/// The external address the fake router hands out: a public one, as a real router's would be.
const EXTERNAL_IP: Ipv4Addr = Ipv4Addr::new(1, 2, 3, 4);
/// The external port the fake router maps when asked for none in particular.
const EXTERNAL_PORT: u16 = 40000;

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("timed out")
}

/// What the fake router speaks and answers.
#[derive(Debug, Clone, Copy)]
enum Speaks {
    /// PCP, granting `lifetime` seconds at `external`, or failing with `result` if nonzero.
    Pcp {
        external: Ipv4Addr,
        lifetime: u32,
        result: u8,
    },
    /// Only NAT-PMP, granting `lifetime` seconds.
    NatPmp { lifetime: u32 },
}

/// A mapping request the fake router received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Request {
    pcp: bool,
    lifetime: u32,
    internal_port: u16,
    nonce: [u8; 12],
}

/// A fake router's PCP and NAT-PMP server on localhost.
struct FakeRouter {
    addr: SocketAddr,
    requests: mpsc::UnboundedReceiver<Request>,
}

impl FakeRouter {
    async fn start(speaks: Speaks) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let (sender, requests) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut buffer = [0; 1100];
            loop {
                let (len, from) = socket.recv_from(&mut buffer).await.unwrap();
                if let Some(response) = answer(speaks, &buffer[..len], &sender) {
                    let _ = socket.send_to(&response, from).await;
                }
            }
        });
        Self { addr, requests }
    }

    async fn next_request(&mut self) -> Request {
        within(self.requests.recv()).await.unwrap()
    }

    fn config(&self) -> PortMappingConfig {
        PortMappingConfig {
            pcp_server: Some(self.addr),
            upnp: false,
            ..PortMappingConfig::default()
        }
    }
}

fn answer(speaks: Speaks, request: &[u8], log: &mpsc::UnboundedSender<Request>) -> Option<Vec<u8>> {
    let u16_at = |i: usize| u16::from_be_bytes([request[i], request[i + 1]]);
    let u32_at = |i: usize| u32::from_be_bytes(request[i..i + 4].try_into().unwrap());
    let epoch = 99u32.to_be_bytes();
    match (speaks, request) {
        (
            Speaks::Pcp {
                external,
                lifetime,
                result,
            },
            [2, 1, ..],
        ) if request.len() == 60 => {
            let requested = u32_at(4);
            let nonce: [u8; 12] = request[24..36].try_into().unwrap();
            let internal_port = u16_at(40);
            log.send(Request {
                pcp: true,
                lifetime: requested,
                internal_port,
                nonce,
            })
            .unwrap();
            let port = match u16_at(42) {
                0 => EXTERNAL_PORT,
                suggested => suggested,
            };
            let mut response = vec![0; 60];
            response[0] = 2;
            response[1] = 0x81;
            response[3] = result;
            let granted = if requested == 0 { 0 } else { lifetime };
            response[4..8].copy_from_slice(&granted.to_be_bytes());
            response[8..12].copy_from_slice(&epoch);
            response[24..60].copy_from_slice(&request[24..60]);
            response[42..44].copy_from_slice(&port.to_be_bytes());
            response[44..60].copy_from_slice(&external.to_ipv6_mapped().octets());
            Some(response)
        }
        // A NAT-PMP router turns PCP away in NAT-PMP.
        (Speaks::NatPmp { .. }, [2, opcode, ..]) => {
            let mut response = vec![0, 128 + opcode, 0, 1];
            response.extend_from_slice(&epoch);
            Some(response)
        }
        (Speaks::NatPmp { .. }, [0, 0]) => {
            let mut response = vec![0, 128, 0, 0];
            response.extend_from_slice(&epoch);
            response.extend_from_slice(&EXTERNAL_IP.octets());
            Some(response)
        }
        (Speaks::NatPmp { lifetime }, [0, 1, 0, 0, ..]) if request.len() == 12 => {
            let requested = u32_at(8);
            let internal_port = u16_at(4);
            log.send(Request {
                pcp: false,
                lifetime: requested,
                internal_port,
                nonce: [0; 12],
            })
            .unwrap();
            let granted = if requested == 0 { 0 } else { lifetime };
            let mut response = vec![0, 129, 0, 0];
            response.extend_from_slice(&epoch);
            response.extend_from_slice(&internal_port.to_be_bytes());
            response.extend_from_slice(&EXTERNAL_PORT.to_be_bytes());
            response.extend_from_slice(&granted.to_be_bytes());
            Some(response)
        }
        _ => None,
    }
}

/// An endpoint on every IPv4 interface, as behind a home router, with a port the system picks.
fn endpoint() -> Endpoint {
    let config = EndpointConfig {
        ipv4: Some(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
        ipv6: None,
        ..EndpointConfig::default()
    };
    Endpoint::bind(Identity::generate().unwrap(), &config).unwrap()
}

fn port(endpoint: &Endpoint) -> u16 {
    endpoint.local_addrs()[0].port()
}

async fn settled(status: &mut watch::Receiver<PortMappingStatus>) -> PortMappingStatus {
    let settled = status.wait_for(|status| {
        matches!(
            status,
            PortMappingStatus::Mapped { .. } | PortMappingStatus::Unavailable { .. }
        )
    });
    within(settled).await.unwrap().clone()
}

fn mapped(protocol: MappingProtocol, port: u16) -> PortMappingStatus {
    PortMappingStatus::Mapped {
        protocol,
        external: Address::from(SocketAddr::from((EXTERNAL_IP, port))),
    }
}

#[tokio::test]
async fn a_pcp_mapping_is_renewed_and_removed_on_close() {
    let mut router = FakeRouter::start(Speaks::Pcp {
        external: EXTERNAL_IP,
        lifetime: 2,
        result: 0,
    })
    .await;
    let endpoint = endpoint();
    let mut status = endpoint.port_mapping().subscribe();
    endpoint.set_port_mapping(router.config());

    let first = router.next_request().await;
    assert_eq!(first.internal_port, port(&endpoint));
    assert_eq!(first.lifetime, 7200);
    assert_eq!(
        settled(&mut status).await,
        mapped(MappingProtocol::Pcp, EXTERNAL_PORT)
    );
    assert_eq!(
        endpoint.port_mapping().address(),
        Some(Address::from(SocketAddr::from((
            EXTERNAL_IP,
            EXTERNAL_PORT
        ))))
    );

    // A 2 s lease is renewed after 1 s, with the same nonce.
    let renewal = router.next_request().await;
    assert_eq!(renewal.nonce, first.nonce);
    assert_eq!(renewal.lifetime, 7200);

    endpoint.close(tempest_protocol::message::GoodbyeReason::ShuttingDown, "");
    within(endpoint.wait_idle()).await;
    let removal = router.next_request().await;
    assert_eq!((removal.lifetime, removal.nonce), (0, first.nonce));
    assert_eq!(endpoint.port_mapping().status(), PortMappingStatus::Off);
}

#[tokio::test]
async fn a_router_that_only_speaks_natpmp_maps_with_it() {
    let mut router = FakeRouter::start(Speaks::NatPmp { lifetime: 3600 }).await;
    let endpoint = endpoint();
    let mut status = endpoint.port_mapping().subscribe();
    endpoint.set_port_mapping(router.config());

    let request = router.next_request().await;
    assert!(!request.pcp);
    assert_eq!(request.internal_port, port(&endpoint));
    assert_eq!(
        settled(&mut status).await,
        mapped(MappingProtocol::NatPmp, EXTERNAL_PORT)
    );

    // Turning mapping off removes it.
    endpoint.set_port_mapping(PortMappingConfig::off());
    let removal = router.next_request().await;
    assert_eq!(removal.lifetime, 0);
    within(status.wait_for(|status| *status == PortMappingStatus::Off))
        .await
        .unwrap();
}

#[tokio::test]
async fn refusals_are_reported() {
    let router = FakeRouter::start(Speaks::Pcp {
        external: EXTERNAL_IP,
        lifetime: 3600,
        result: 2,
    })
    .await;
    let endpoint = endpoint();
    let mut status = endpoint.port_mapping().subscribe();
    endpoint.set_port_mapping(router.config());
    let PortMappingStatus::Unavailable { failures } = settled(&mut status).await else {
        panic!("expected a failure");
    };
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].to_string(),
        "PCP: the router refused: not authorized"
    );
    assert_eq!(endpoint.port_mapping().address(), None);
}

#[tokio::test]
async fn a_private_external_address_is_not_used() {
    let mut router = FakeRouter::start(Speaks::Pcp {
        external: Ipv4Addr::new(100, 64, 0, 7),
        lifetime: 3600,
        result: 0,
    })
    .await;
    let endpoint = endpoint();
    let mut status = endpoint.port_mapping().subscribe();
    endpoint.set_port_mapping(router.config());
    let PortMappingStatus::Unavailable { failures } = settled(&mut status).await else {
        panic!("expected a failure");
    };
    assert!(
        matches!(failures[0].error, MappingError::NotPublic(_)),
        "{failures:?}"
    );
    // The useless mapping is removed again.
    assert_eq!(router.next_request().await.lifetime, 7200);
    assert_eq!(router.next_request().await.lifetime, 0);
}

#[tokio::test]
async fn a_router_that_does_not_answer_is_reported() {
    // A port nothing listens on.
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = silent.local_addr().unwrap();
    drop(silent);
    let endpoint = endpoint();
    let mut status = endpoint.port_mapping().subscribe();
    endpoint.set_port_mapping(PortMappingConfig {
        pcp_server: Some(addr),
        upnp: false,
        ..PortMappingConfig::default()
    });
    let PortMappingStatus::Unavailable { failures } = settled(&mut status).await else {
        panic!("expected a failure");
    };
    assert_eq!(failures[0].error, MappingError::NoAnswer);
}

#[tokio::test]
async fn sockets_on_loopback_are_not_mapped() {
    let router = FakeRouter::start(Speaks::NatPmp { lifetime: 3600 }).await;
    let endpoint =
        Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap();
    endpoint.set_port_mapping(router.config());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(endpoint.port_mapping().status(), PortMappingStatus::Off);
}

/// A tracker on localhost. Returns its URL and what stops it.
async fn start_tracker() -> (String, oneshot::Sender<()>) {
    let config = TrackerConfig {
        interval: Duration::from_secs(1),
        min_interval: Duration::ZERO,
        ..TrackerConfig::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(Tracker::new(config).unwrap().serve(listener, async {
        let _ = stopped.await;
    }));
    (url, stop)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_external_address_is_announced() {
    let router = FakeRouter::start(Speaks::Pcp {
        external: EXTERNAL_IP,
        lifetime: 3600,
        result: 0,
    })
    .await;
    let (tracker, _stop) = start_tracker().await;
    let node = Node::start(endpoint(), NodeConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let data = b"mapped".to_vec();
    fs::write(dir.path().join("a.txt"), &data).unwrap();
    let info = FileInfo {
        size: data.len() as u64,
        root: tree::root(&data),
        executable: false,
        modified: 1_700_000_000_000,
    };
    let manifest =
        Manifest::new("mapped", "", 0, "test", vec![Entry::file("a.txt", info)]).unwrap();
    let storage = Storage::open_shared(dir.path(), Arc::new(manifest), StorageConfig::default())
        .await
        .unwrap();
    let shared = node.share(storage).await.unwrap();
    let config = AnnounceConfig {
        min_interval: Duration::ZERO,
        ..AnnounceConfig::new(node.endpoint().peer_id(), port(node.endpoint()))
    };
    let client = TrackerClient::new(Duration::from_secs(5)).unwrap();
    let announcer = Announcer::spawn(client.clone(), tracker.clone(), config, &shared);

    // Mapping after the first announce: the announcer tells the tracker at once.
    node.endpoint().set_port_mapping(router.config());
    let external = Address::from(SocketAddr::from((EXTERNAL_IP, EXTERNAL_PORT)));
    let request = AnnounceRequest {
        bundle: shared.bundle_id(),
        peer: PeerId::new([3; 32]),
        port: 9,
        addrs: Vec::new(),
        relay: None,
        event: AnnounceEvent::Started,
        complete: false,
        num_want: 50,
    };
    let heard = async {
        loop {
            let response = client.announce(&tracker, &request).await.unwrap();
            let entry = response
                .peers
                .iter()
                .find(|entry| entry.peer == node.endpoint().peer_id());
            if entry.is_some_and(|entry| entry.addrs.contains(&external)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    within(heard).await;
    announcer.stop().await;
    node.shutdown().await;
}

/// Maps a port with the real router, then removes the mapping. Run by hand on a network whose
/// router supports PCP, NAT-PMP or UPnP-IGD: `cargo test -p tempest-net --test port_mapping --
/// --ignored --nocapture`.
#[tokio::test]
#[ignore = "asks the real router for a port mapping"]
async fn the_real_router_maps_a_port() {
    let endpoint = endpoint();
    let mut status = endpoint.port_mapping().subscribe();
    endpoint.set_port_mapping(PortMappingConfig::default());
    let settled = settled(&mut status).await;
    println!("{settled:?}");
    endpoint.close(tempest_protocol::message::GoodbyeReason::ShuttingDown, "");
    within(endpoint.wait_idle()).await;
    assert!(matches!(settled, PortMappingStatus::Mapped { .. }));
}
