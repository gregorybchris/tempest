# Engine API

The engine (D-079) serves an HTTP + JSON API with a stream of server-sent events (D-080). The web UI, the TUI and the CLI are its clients. This file is the API's contract; the request and response types are in the `tempest-api` crate, and a transfer's fields are documented on `TransferInfo`.

## Access

- By default the API is plain HTTP on a loopback address, TCP 4776 (D-126), and needs no credentials. It answers only requests addressed to a loopback host, and requests from web pages only if the page is on a loopback host too. This keeps other websites out, including through DNS rebinding.
- Remote access is opt-in (D-125). It serves HTTPS, with a configured certificate or a self-signed one the engine makes, and every request, local or not, must carry the access token as `Authorization: Bearer <token>`, or a session from signing in with it (below). Without either the answer is 401. The engine makes the token if there is none and keeps it, like the certificate, in its data directory. On startup the daemon prints the certificate's SHA-256 fingerprint for remote clients to check.
- A browser can't send the header on its event stream, and the web UI never holds the token. With remote access on, the web UI shows a sign-in page (D-222). `POST /v1/session` takes the token and answers with a session in a cookie that is `HttpOnly`, `Secure` and `SameSite=Strict`, which then authorizes requests and the event stream like the header does. The CLI and TUI keep using the header.
- A session is stateless: its cookie holds an expiry and a MAC of it keyed by the access token. Sessions expire after a fixed lifetime, survive an engine restart, and all end when the token changes. `DELETE /v1/session` clears the cookie. Bodies must be JSON, so no cross-site form can sign in.
- While the API runs, the engine records its URL, and its certificate when remote access is on, in a file in the data directory. Local clients find the API there, whatever its port, and trust exactly that certificate.

The settings are under `api` in the config file.

The `tempest` binary's engine also serves the client web UI at `/` and its files beside it, without the access token, since they hold no data; every path the UI doesn't use belongs to the API.

## Requests and errors

- Every route is under `/v1`. A later, incompatible version gets a new prefix.
- Request bodies are JSON and must be sent as `application/json`; anything else gets 415. Browsers can't send such requests to another origin without asking first, and the API never says yes.
- Every error has the same body: `{"error": "<code>", "message": "<sentence>"}`. The codes are `invalid_request` (400), `unknown_category` (400), `unauthorized` (401), `forbidden` (403), `not_found` (404), `method_not_allowed` (405), `already_added` (409), `removing` (409), `moving` (409, its files are being moved), `no_data` (409, nothing on disk to check yet), `no_files` (409, nothing on disk to show yet), `local_only` (403, the action works only on the engine's own machine), `reveal_failed` (500, the file manager didn't start), `unsupported_media_type` (415), `internal` (500) and `unavailable` (503, the engine is shutting down).
- A transfer's ID is its bundle ID in hex.

## Routes

| Method and path | Body | Answer |
| --- | --- | --- |
| `GET /v1/engine` | | The engine's version, peer ID, advertised addresses, home relay and port mapping |
| `GET /v1/transfers` | | Every transfer, in queue order |
| `POST /v1/transfers` | `link`, or an absolute `path` to share; optionally `category`, `tags`, `trackers`; for a link `save_folder`, `stopped`; for a path `name`, `comment`, `created` | 201 and the transfer |
| `GET /v1/transfers/{id}` | | The transfer |
| `DELETE /v1/transfers/{id}?delete_files=true` | | 204, once removed (and its files deleted, if asked) |
| `GET /v1/transfers/{id}/files` | | Each file's path, size, chunks and chunks stored; none before the manifest arrives |
| `GET /v1/transfers/{id}/peers` | | Each connection of a running transfer: peer ID, address, direct or relayed, Want and Serving both ways, rates and totals, the peer's progress and round-trip time; none while it isn't running |
| `GET /v1/transfers/{id}/chunks?cells=N` | | The chunk map: runs of chunks in at most `N` cells (1024 by default, 65,536 at most), each `[have, in_flight, missing, availability]` |
| `POST /v1/transfers/{id}/start` | | The transfer, started through the queue |
| `POST /v1/transfers/{id}/force-start` | | The transfer, started regardless of the queue |
| `POST /v1/transfers/{id}/stop` | | The transfer |
| `POST /v1/transfers/{id}/recheck` | | The transfer, whose data is hashed again from scratch ("checking"); it then resumes, fetching what failed |
| `POST /v1/transfers/{id}/reveal` | | `{"path": "..."}`, what the engine showed in its machine's file manager (D-237): the file, for a bundle of one, and otherwise the folder. Only with local access; remote access answers 403 `local_only`. 409 `no_files` before anything is saved or if the folder is gone |
| `POST /v1/transfers/{id}/move` | `{"save_folder": "..."}`, absolute | The transfer, whose files move there ("moving"); a failed move leaves them in place and shows the error |
| `PUT /v1/transfers/{id}/name` | `{"name": "..."}`, or `null` for the bundle's own | The transfer |
| `PUT /v1/transfers/{id}/tags` | `{"tags": [...]}` | The transfer |
| `PUT /v1/transfers/{id}/category` | `{"category": "..."}`, or `null` | The transfer |
| `PUT /v1/transfers/{id}/settings` | The transfer's own settings by dotted key, replacing what it had: session settings, `limits.download` and `limits.upload`, and `sharing.*` (`sharing.idle_timeout` applies when the transfer next starts) | The transfer |
| `POST /v1/transfers/{id}/queue` | `{"to": "top" \| "up" \| "down" \| "bottom" \| <place>}` | Every transfer, in the new order |
| `POST /v1/transfers/{id}/publish` | `{"tracker", "listing", "invite"}`: a tracker's URL, the listing's fields as the tracker's `PUT /tempest/v1/listings/{bundle}` takes them, and an optional invite | The listing, after the engine signed in with its key and set it; from then on the transfer announces there with `listing` (D-129) |
| `DELETE /v1/transfers/{id}/publish?tracker=…` | | The transfer, after the engine signed in with its key and removed the listing through the tracker's `DELETE /tempest/v1/listings/{bundle}`; from then on it announces there without `listing`, or not at all if only publishing put it there. A listing the tracker doesn't have counts as removed |
| `POST /v1/trackers/sign-in` | `{"tracker", "code", "invite"}` | The account, after the engine signed in for whoever asked the tracker's website for `code`, such as a browser |
| `GET /v1/settings` | | The settings in effect, which were changed through the API, which groups await a restart, and whether the alternative speed limits apply now |
| `PATCH /v1/settings` | Settings by dotted key, as in the config file | As `GET`; all the changes apply, or none |
| `DELETE /v1/settings/{key}` | | As `GET`, with the config file's value back |
| `GET /v1/categories` | | Each category's save folder, by name |
| `PUT /v1/categories/{name}` | `{"save_folder": "..."}` | The category |
| `DELETE /v1/categories/{name}` | | 204; transfers in it have none |
| `GET /v1/events?after=N` | | The event stream |
| `GET /v1/session` | | `{"required", "signed_in"}`: whether the engine wants a sign-in and whether the request has one. Answered without credentials |
| `POST /v1/session` | `{"token": "..."}` | As `GET`, after the token matched, with the session cookie set; 401 otherwise. Answered without credentials |
| `DELETE /v1/session` | | 204, with the session cookie cleared |

A chunk map's cells cover consecutive chunks in order, their sizes differing by at most one, so its size stays bounded however large the bundle. A cell's availability is the fewest connected peers holding any one of its chunks. A transfer that isn't running has nothing in flight and every availability 0, and says so with `"running": false` (D-193).

A transfer carries its status (`state`, plus a `reason` when stalled or errored), its progress, rates, connection and peer counts, time left, ratio, tags, category, folders, link, dates and the trackers it is published on.

The `ui` settings include whether the first-run account of the network defaults was shown (D-227), so it is said once per data directory, whichever client is first.

Settings changed through the API are kept in the state database and apply over the config file, which the engine never rewrites. Queue, speed and sharing limits, notifications, trackers, relays, port mapping and the clients' `ui` settings (D-195) apply at once, session settings when a transfer next starts, and `peers`, `lan` and `api` when the engine restarts. `data_dir` is only set in the file. The sharing limits are a ratio, a time sharing and an idle timeout, whichever comes first, and an action at the limit; a share starts with none, so its link keeps working, and only limits set on the share apply (D-224). Session settings under `lingering.` are refused, with a pointer to the `sharing.` setting that replaces them. Switching the alternative speed limits on and off is a setting like any other, `limits.alternative.enabled`. At start the daemon logs each config-file key that a stored setting overrides, and how to reset it (D-231).

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

- Every event but `snapshot` and `reset` has an `id:`, increasing by one per event. A client that reconnects with `Last-Event-ID`, or `?after=` where it can't set headers as a browser's `EventSource` can't, gets every event after that one; the header wins if both are given. If the engine no longer has them all, or has restarted since, the stream starts with `reset`, and the client should replace its view.
- `snapshot` and `reset` have no ID, so a client that reconnects resumes from the last numbered event it saw.
- A client that falls too far behind gets `reset` and carries on.
- Comments keep idle connections open. The stream ends when the engine stops.

## The CLI

`tempest list`, `start`, `stop` and `remove` use the API of the daemon on the same data directory. When no daemon answers, they open the engine's state themselves without running any transfer, make the change, and say so. Only one engine uses a data directory at a time.

`tempest share` and `tempest get` hand their transfer to the daemon when one answers: `share` prints the link and returns, and `get` follows the download to completion. Every option that describes the transfer itself, such as `--tracker` or `--created`, goes to the daemon in the add request (D-228). Only `--standalone` and the options of a separate process (its port, identity, binding, relay and LAN settings) run them without the daemon (D-175).

A `tempest tui` or `tempest web` with no daemon runs an engine of its own. Quitting it while transfers are active asks whether to hand them to a daemon or stop them. Handing over shuts the engine down and starts a daemon in the background on the same data directory, which resumes them (D-228).

`tempest share --publish <tracker>` also lists the share on that tracker, through the daemon's `publish` route or, standalone, by signing in itself. `tempest publish` does the same for a transfer already added, `tempest unpublish` removes the listing, and `tempest sign-in <tracker> --code` signs in a browser showing the tracker's website. A tracker without listings refuses with "the tracker doesn't take listings". All of them sign with the engine's key.
