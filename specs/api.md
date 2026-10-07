# Engine API

The engine (D-079) serves an HTTP + JSON API with a stream of server-sent events (D-080). The web UI, the TUI and the CLI are its clients. This file is the API's contract; the request and response types are in `tempest-engine`'s `api` module, and a transfer's fields are documented on `TransferInfo`.

## Access

- By default the API is plain HTTP on a loopback address, TCP 4776 (D-126), and needs no credentials. It answers only requests addressed to a loopback host, and requests from web pages only if the page is on a loopback host too. This keeps other websites out, including through DNS rebinding.
- Remote access is opt-in (D-125). It serves HTTPS, with a configured certificate or a self-signed one the engine makes, and every request, local or not, must carry the access token as `Authorization: Bearer <token>`. Without it the answer is 401. The engine makes the token if there is none and keeps it, like the certificate, in its data directory. On startup the daemon prints the certificate's SHA-256 fingerprint for remote clients to check.
- While the API runs, the engine records its URL, and its certificate when remote access is on, in a file in the data directory. Local clients find the API there, whatever its port, and trust exactly that certificate.

The settings are under `api` in the config file.

## Requests and errors

- Every route is under `/v1`. A later, incompatible version gets a new prefix.
- Request bodies are JSON and must be sent as `application/json`; anything else gets 415. Browsers can't send such requests to another origin without asking first, and the API never says yes.
- Every error has the same body: `{"error": "<code>", "message": "<sentence>"}`. The codes are `invalid_request` (400), `unknown_category` (400), `unauthorized` (401), `forbidden` (403), `not_found` (404), `method_not_allowed` (405), `already_added` (409), `removing` (409), `moving` (409, its files are being moved), `no_data` (409, nothing on disk to check yet), `unsupported_media_type` (415), `internal` (500) and `unavailable` (503, the engine is shutting down).
- A transfer's ID is its bundle ID in hex.

## Routes

| Method and path | Body | Answer |
| --- | --- | --- |
| `GET /v1/engine` | | The engine's version, peer ID, advertised addresses, home relay and port mapping |
| `GET /v1/transfers` | | Every transfer, in queue order |
| `POST /v1/transfers` | `link`, or an absolute `path` to share; optionally `category`, `tags`; for a link `save_folder`, `stopped`; for a path `name`, `comment` | 201 and the transfer |
| `GET /v1/transfers/{id}` | | The transfer |
| `DELETE /v1/transfers/{id}?delete_files=true` | | 204, once removed (and its files deleted, if asked) |
| `GET /v1/transfers/{id}/files` | | Each file's path, size, chunks and chunks stored; none before the manifest arrives |
| `GET /v1/transfers/{id}/peers` | | Each connection of a running transfer: peer ID, address, direct or relayed, Want and Serving both ways, rates and totals, the peer's progress and round-trip time; none while it isn't running |
| `GET /v1/transfers/{id}/chunks?cells=N` | | The chunk map: runs of chunks in at most `N` cells (1024 by default, 65,536 at most), each `[have, in_flight, missing, availability]` |
| `POST /v1/transfers/{id}/start` | | The transfer, started through the queue |
| `POST /v1/transfers/{id}/force-start` | | The transfer, started regardless of the queue |
| `POST /v1/transfers/{id}/stop` | | The transfer |
| `POST /v1/transfers/{id}/recheck` | | The transfer, whose data is hashed again from scratch ("checking"); it then resumes, fetching what failed |
| `POST /v1/transfers/{id}/move` | `{"save_folder": "..."}`, absolute | The transfer, whose files move there ("moving"); a failed move leaves them in place and shows the error |
| `PUT /v1/transfers/{id}/name` | `{"name": "..."}`, or `null` for the bundle's own | The transfer |
| `PUT /v1/transfers/{id}/tags` | `{"tags": [...]}` | The transfer |
| `PUT /v1/transfers/{id}/category` | `{"category": "..."}`, or `null` | The transfer |
| `PUT /v1/transfers/{id}/settings` | The transfer's own settings by dotted key, replacing what it had: session settings, `limits.download` and `limits.upload`, and `sharing.*` | The transfer |
| `POST /v1/transfers/{id}/queue` | `{"to": "top" \| "up" \| "down" \| "bottom" \| <place>}` | Every transfer, in the new order |
| `GET /v1/settings` | | The settings in effect, which were changed through the API, which groups await a restart, and whether the alternative speed limits apply now |
| `PATCH /v1/settings` | Settings by dotted key, as in the config file | As `GET`; all the changes apply, or none |
| `DELETE /v1/settings/{key}` | | As `GET`, with the config file's value back |
| `GET /v1/categories` | | Each category's save folder, by name |
| `PUT /v1/categories/{name}` | `{"save_folder": "..."}` | The category |
| `DELETE /v1/categories/{name}` | | 204; transfers in it have none |
| `GET /v1/events` | | The event stream |

A chunk map's cells cover consecutive chunks in order, their sizes differing by at most one, so its size stays bounded however large the bundle. A cell's availability is the fewest connected peers holding any one of its chunks. A transfer that isn't running has nothing in flight and every availability 0, and says so with `"running": false` (D-193).

A transfer carries its status (`state`, plus a `reason` when stalled or errored), its progress, rates, connection and peer counts, time left, ratio, tags, category, folders, link and dates.

Settings changed through the API are kept in the state database and apply over the config file, which the engine never rewrites. Queue, speed and sharing limits, notifications, trackers, relays, port mapping and the clients' `ui` settings (D-195) apply at once, session settings when a transfer next starts, and `peers`, `lan` and `api` when the engine restarts. `data_dir` is only set in the file. Switching the alternative speed limits on and off is a setting like any other, `limits.alternative.enabled`.

## Events

`GET /v1/events` is a `text/event-stream`. Each event has a JSON `data` line and one of these names:

| Event | Data |
| --- | --- |
| `transfer` | A transfer that was added or changed: the whole transfer |
| `removed` | `{"id"}` |
| `decision` | A session's decision event (D-006): `{"id", "time", "kind", "reason", "text"}` |
| `decisions_missed` | `{"id", "count"}`: decision events dropped because they came too fast |
| `snapshot` | `{"transfers": [...]}`: every transfer, first on a new stream |
| `reset` | As `snapshot`, when the events a client asked for are gone |

- Every event but `snapshot` and `reset` has an `id:`, increasing by one per event. A client that reconnects with `Last-Event-ID` gets every event after that one. If the engine no longer has them all, or has restarted since, the stream starts with `reset`, and the client should replace its view.
- `snapshot` and `reset` have no ID, so a client that reconnects resumes from the last numbered event it saw.
- A client that falls too far behind gets `reset` and carries on.
- Comments keep idle connections open. The stream ends when the engine stops.

## The CLI

`tempest list`, `start`, `stop` and `remove` use the API of the daemon on the same data directory. When no daemon answers, they open the engine's state themselves without running any transfer, make the change, and say so. Only one engine uses a data directory at a time.

`tempest share` and `tempest get` hand their transfer to the daemon when one answers: `share` prints the link and returns, and `get` follows the download to completion. Options that only make sense for a process of its own, such as `--port` or `--identity`, and `--standalone`, run them without the daemon as before (D-175).
