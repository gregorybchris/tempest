# Trackers

> **Status: Draft (version 1).**

A **tracker** is a server that helps peers in the same swarm find each other. Peers **announce** that they're participating in a bundle, and the tracker replies with other peers in that swarm. This section defines the tracker API. A tracker MAY offer more, such as a catalog website, but that is outside the protocol.

## Transport

Trackers are HTTPS servers. Requests and responses are JSON (`Content-Type: application/json`), encoded as UTF-8.

- All endpoints are under the path prefix `/tempest/v1/`. A tracker URL names the tracker's base, e.g. `https://tracker.example.org` or `https://example.org/tracker`. Clients remove any trailing `/` from it and append the endpoint path, e.g. `https://example.org/tracker/tempest/v1/announce`.
- Bundle IDs and peer IDs are 64 lowercase hexadecimal characters.
- Addresses are strings of the form `ip:port`, with IPv6 addresses in brackets (`[2001:db8::1]:4433`).
- Receivers MUST ignore JSON fields they don't recognize.

**Rationale.** HTTPS and JSON let anyone explore a tracker with `curl` or a browser. A tracker can be built with standard web tooling, and the same server can host a catalog website. The version in the path lets a future, incompatible API coexist with this one. Ignoring unknown fields lets either side add fields without breaking the other.

## Announce

```text
POST /tempest/v1/announce
```

### Request

| Field | Type | Meaning |
| --- | --- | --- |
| `bundle` | string | The bundle ID |
| `peer` | string | The announcing peer's peer ID |
| `port` | integer | The UDP port of the peer's QUIC endpoint |
| `addrs` | array of strings | OPTIONAL. Additional direct addresses (e.g. an IPv6 address). Defaults to none |
| `relay` | string or null | REQUIRED. The peer's relay address (`ip:port`, lent by its home relay; see [relays.md](relays.md)), or `null` |
| `event` | string | `started`, `update`, `completed` or `stopped` |
| `complete` | boolean | Whether the peer has the whole bundle |
| `num_want` | integer | OPTIONAL. How many peers the client would like, default 50 |

### Response (HTTP 200)

| Field | Type | Meaning |
| --- | --- | --- |
| `interval_secs` | integer | Seconds until the peer should announce again |
| `peers` | array | Other peers in the swarm, each `{ "peer": …, "addrs": […], "relay": … }` |
| `complete` | integer | Number of complete peers the tracker knows of (not to be confused with the request's boolean `complete`) |
| `incomplete` | integer | Number of incomplete peers the tracker knows of |
| `your_ip` | string | The IP address the tracker observed for this request |
| `relays` | array of strings | Relays the tracker recommends, as relay identities (`<key>@host:port`; see [relays.md](relays.md#relay-identity)). MAY be empty |

### Rules

**Client:**

- A client MUST send `started` when it begins participating in a bundle (with `complete: true` if it already has everything), `completed` when it becomes complete while participating, and `stopped` when it stops participating.
- Between those events, it SHOULD send `update` every `interval_secs`, and MUST NOT send `update` more often.
- Clients SHOULD include only addresses reachable from the internet in `addrs`. Local network addresses would reveal internal details to the tracker and to other peers.

**Tracker:**

- The tracker MUST add the request's observed source IP, combined with `port`, to the peer's addresses. This is a best guess: it works when the peer's router keeps the same port, or when the peer has a port mapping.
- The tracker MUST NOT include the requesting peer in `peers`. It SHOULD return at most `num_want` peers (and MAY cap `num_want`), chosen at random.
- On `stopped`, the tracker SHOULD remove the peer immediately and MAY return an empty `peers` list.
- The tracker SHOULD forget a peer that hasn't announced for twice the most recent `interval_secs` it gave that peer.

**Example:**

```text
POST /tempest/v1/announce
{
  "bundle": "3f9a…",
  "peer": "7c21…",
  "port": 4433,
  "relay": null,
  "event": "started",
  "complete": false
}

200 OK
{
  "interval_secs": 300,
  "peers": [
    { "peer": "a1b2…", "addrs": ["198.51.100.9:4433"], "relay": null }
  ],
  "complete": 1,
  "incomplete": 4,
  "your_ip": "203.0.113.5",
  "relays": []
}
```

**Rationale.**

- **Periodic announces.** The tracker can stay simple and stateless and serve very large numbers of peers. Swarm counts are at most one interval stale.
- **No signatures.** Announces carry no proof of the peer ID. A forged listing only wastes connection attempts, because connecting verifies the peer ID through TLS ([connections.md](connections.md#peer-identity)).
- **`your_ip`.** It tells a peer its public IP address, which helps with reaching peers behind routers. Only the IP is reported: the request arrives over HTTPS (TCP), whose source port says nothing about the router's UDP mapping for the peer's QUIC socket.

## Bundle stats

```text
GET /tempest/v1/bundles/{bundle}
```

Returns a bundle's swarm counts without announcing:

```text
200 OK
{ "complete": 1, "incomplete": 4 }
```

A tracker that knows nothing about the bundle returns zero counts.

## Errors

An error is an HTTP status code (4xx or 5xx) with a JSON body:

| Field | Type | Meaning |
| --- | --- | --- |
| `error` | string | A machine-readable code (below) |
| `message` | string | A human-readable explanation |
| `retry_after_secs` | integer | OPTIONAL. When to try again. If absent after a 5xx error, the client chooses its own back-off |

| Code | Typical status | Meaning |
| --- | --- | --- |
| `invalid_request` | 400 | A field is missing or malformed |
| `rate_limited` | 429 | The client is announcing too often |
| `bundle_not_allowed` | 403 | This tracker doesn't serve this bundle (e.g. a private tracker) |
| `unavailable` | 503 | The tracker is temporarily unable to serve requests |

A client MUST honor `retry_after_secs` when present.

**Rationale.** Explicit retry guidance (as in BitTorrent's BEP 31) prevents clients from hammering a struggling tracker.
