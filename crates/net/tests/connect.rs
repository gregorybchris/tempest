//! Two endpoints on localhost: handshakes, identities, Goodbyes and framed messages.

use std::time::Duration;

use tempest_core::io::{Closed, Direction};
use tempest_net::{
    Connection, Endpoint, EndpointConfig, HandshakeError, Identity, ReadMessageError, closed_cause,
    read_message, read_message_within, write_message,
};
use tempest_protocol::message::{
    GoodbyeReason, Hello, MessageType, Request, max_data_frame_length,
};
use tempest_protocol::{Address, DecodeError, Hash, Message, PeerId};

fn endpoint() -> Endpoint {
    Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap()
}

/// Dials `server` from `client`, returning the dialed and accepted sides.
async fn connect(
    client: &Endpoint,
    server: &Endpoint,
    expected: Option<PeerId>,
) -> (
    Result<Connection, HandshakeError>,
    Result<Connection, HandshakeError>,
) {
    let addr = server.local_addrs()[0];
    let accepting = server.clone();
    let accept = tokio::spawn(async move { accepting.accept().await.unwrap().accept().await });
    let dialed = client.dial(addr, expected).await;
    (dialed, accept.await.unwrap())
}

async fn pair() -> (Endpoint, Endpoint, Connection, Connection) {
    let (client, server) = (endpoint(), endpoint());
    let (dialed, accepted) = connect(&client, &server, Some(server.peer_id())).await;
    (client, server, dialed.unwrap(), accepted.unwrap())
}

fn hello() -> Message {
    Hello {
        bundle_id: Hash::new([7; 32]),
        capabilities: Vec::new(),
        client: "tempest/test".into(),
        max_request_chunks: 64,
        max_open_requests: 32,
        addresses: vec![Address::from(
            "127.0.0.1:4773".parse::<std::net::SocketAddr>().unwrap(),
        )],
        relay: None,
    }
    .into()
}

#[tokio::test]
async fn both_sides_learn_the_other_peers_id() {
    let (client, server, dialed, accepted) = pair().await;
    assert_eq!(dialed.peer(), server.peer_id());
    assert_eq!(accepted.peer(), client.peer_id());
    assert_eq!(dialed.direction(), Direction::Dialed);
    assert_eq!(accepted.direction(), Direction::Accepted);
    assert_eq!(dialed.remote_address(), server.local_addrs()[0]);
}

#[tokio::test]
async fn dialing_without_an_expected_peer_accepts_any_key() {
    let (client, server) = (endpoint(), endpoint());
    let (dialed, accepted) = connect(&client, &server, None).await;
    assert_eq!(dialed.unwrap().peer(), server.peer_id());
    assert_eq!(accepted.unwrap().peer(), client.peer_id());
}

#[tokio::test]
async fn a_different_key_than_expected_fails_the_handshake() {
    let (client, server) = (endpoint(), endpoint());
    let someone_else = Identity::generate().unwrap().peer_id();
    let (dialed, accepted) = connect(&client, &server, Some(someone_else)).await;
    let error = dialed.unwrap_err();
    let HandshakeError::UnexpectedPeer(unexpected) = &error else {
        panic!("{error}");
    };
    assert_eq!(unexpected.expected, someone_else);
    assert_eq!(unexpected.actual, server.peer_id());
    assert_eq!(
        error.to_string(),
        format!(
            "presented peer ID {}, expected {someone_else}",
            server.peer_id()
        )
    );
    assert!(accepted.is_err());
}

#[tokio::test]
async fn messages_cross_the_control_stream_and_request_streams() {
    let (_client, _server, dialed, accepted) = pair().await;

    // The dialing peer's first stream is the control stream, QUIC stream 0.
    let (mut send, mut recv) = dialed.quic().open_bi().await.unwrap();
    write_message(&mut send, &hello()).await.unwrap();
    let (mut their_send, mut their_recv) = accepted.quic().accept_bi().await.unwrap();
    assert_eq!(their_recv.id().index(), 0);
    assert_eq!(read_message(&mut their_recv).await.unwrap(), Some(hello()));
    write_message(&mut their_send, &hello()).await.unwrap();
    assert_eq!(read_message(&mut recv).await.unwrap(), Some(hello()));

    // A request stream opened by the accepting peer: one message each way, then finish.
    let request = Message::Request(Request { start: 0, count: 4 });
    let (mut send, mut recv) = accepted.quic().open_bi().await.unwrap();
    write_message(&mut send, &request).await.unwrap();
    send.finish().unwrap();
    let (mut reply, mut incoming) = dialed.quic().accept_bi().await.unwrap();
    assert_eq!(read_message(&mut incoming).await.unwrap(), Some(request));
    assert_eq!(read_message(&mut incoming).await.unwrap(), None);
    write_message(&mut reply, &Message::ManifestRequest)
        .await
        .unwrap();
    reply.finish().unwrap();
    assert_eq!(
        read_message(&mut recv).await.unwrap(),
        Some(Message::ManifestRequest)
    );
    assert_eq!(read_message(&mut recv).await.unwrap(), None);
}

#[tokio::test]
async fn an_oversized_frame_is_rejected_before_its_body_arrives() {
    let (_client, _server, dialed, accepted) = pair().await;
    let (mut send, _recv) = dialed.quic().open_bi().await.unwrap();
    // A Request header claiming a 100-byte body, which never comes; the stream stays open.
    send.write_all(&[0, 0, 0, 100, MessageType::Request.code()])
        .await
        .unwrap();
    let (_send, mut recv) = accepted.quic().accept_bi().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), read_message(&mut recv))
        .await
        .expect("rejected without waiting");
    assert!(matches!(
        result,
        Err(ReadMessageError::Decode(DecodeError::FrameTooLong {
            length: 100,
            max: 9
        }))
    ));
}

#[tokio::test]
async fn a_tighter_limit_applies_to_data() {
    let (_client, _server, dialed, accepted) = pair().await;
    let (mut send, _recv) = dialed.quic().open_bi().await.unwrap();
    let length = max_data_frame_length(2) + 1;
    let mut header = length.to_be_bytes().to_vec();
    header.push(MessageType::Data.code());
    send.write_all(&header).await.unwrap();
    let (_send, mut recv) = accepted.quic().accept_bi().await.unwrap();
    let result = read_message_within(&mut recv, |t| match t {
        MessageType::Data => max_data_frame_length(1),
        other => other.max_frame_length(),
    })
    .await;
    assert!(matches!(
        result,
        Err(ReadMessageError::Decode(DecodeError::FrameTooLong { .. }))
    ));
}

#[tokio::test]
async fn a_stream_that_ends_inside_a_frame_is_truncated() {
    let (_client, _server, dialed, accepted) = pair().await;
    let (mut send, _recv) = dialed.quic().open_bi().await.unwrap();
    let frame = hello().encode_frame();
    send.write_all(&frame[..frame.len() - 1]).await.unwrap();
    send.finish().unwrap();
    let (_send, mut recv) = accepted.quic().accept_bi().await.unwrap();
    assert!(matches!(
        read_message(&mut recv).await,
        Err(ReadMessageError::Truncated)
    ));

    let (mut send, _recv) = dialed.quic().open_bi().await.unwrap();
    send.write_all(&frame[..3]).await.unwrap();
    send.finish().unwrap();
    let (_send, mut recv) = accepted.quic().accept_bi().await.unwrap();
    assert!(matches!(
        read_message(&mut recv).await,
        Err(ReadMessageError::Truncated)
    ));
}

#[tokio::test]
async fn a_goodbye_reaches_the_other_peer() {
    let (_client, _server, dialed, accepted) = pair().await;
    accepted.close(GoodbyeReason::UnknownBundle, "not sharing that");
    let error = dialed.closed().await;
    assert_eq!(
        closed_cause(&error),
        Some(Closed::Goodbye {
            reason: GoodbyeReason::UnknownBundle,
            message: "not sharing that".into(),
        })
    );
    assert_eq!(closed_cause(&accepted.closed().await), None);
}

#[tokio::test]
async fn closing_the_endpoint_says_goodbye_to_every_peer() {
    let (client, server, dialed, _accepted) = pair().await;
    server.close(GoodbyeReason::ShuttingDown, "");
    server.wait_idle().await;
    assert!(matches!(
        closed_cause(&dialed.closed().await),
        Some(Closed::Goodbye {
            reason: GoodbyeReason::ShuttingDown,
            ..
        })
    ));
    assert!(server.accept().await.is_none());
    drop(client);
}

#[tokio::test]
async fn each_address_family_has_its_own_socket() {
    let config = EndpointConfig {
        ipv4: Some("127.0.0.1:0".parse().unwrap()),
        ipv6: Some("[::1]:0".parse().unwrap()),
        ..EndpointConfig::default()
    };
    let bind = || Endpoint::bind(Identity::generate().unwrap(), &config).unwrap();
    let (client, server) = (bind(), bind());
    let [v4, v6] = server.local_addrs()[..] else {
        // This host has no IPv6 loopback.
        return;
    };
    assert!(v4.is_ipv4() && v6.is_ipv6());

    let accepting = server.clone();
    tokio::spawn(async move {
        while let Some(incoming) = accepting.accept().await {
            tokio::spawn(incoming.accept());
        }
    });
    let dialed = client.dial(v6, Some(server.peer_id())).await.unwrap();
    assert_eq!(dialed.remote_address(), v6);
    // An IPv4 address in IPv6-mapped form goes out of the IPv4 socket.
    let mapped = format!("[::ffff:127.0.0.1]:{}", v4.port()).parse().unwrap();
    let dialed = client.dial(mapped, Some(server.peer_id())).await.unwrap();
    assert_eq!(dialed.remote_address(), v4);
}
