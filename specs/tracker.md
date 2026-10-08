# Running a tracker

A Tempest tracker helps peers find each other, and can run a catalog: a website where people search the bundles that their sharers chose to list. This page is for people who run one. The peer-list service is protocol, specified in [`protocol/trackers.md`](../protocol/trackers.md); the catalog is an application on top of it (D-025).

## A neutral tool

Tempest is software, like a web server or a file system. The project publishes it and does not run a public tracker or catalog (D-031).

**If you run a tracker or a catalog, you are responsible for what it serves and lists,** under the laws that apply to you. Tempest does not check content. The catalog gives you the means to act on complaints (see Moderation below), but nothing acts for you. Plan how you will handle complaints before you open a catalog to the public, for example by requiring invites (D-128) so that you know who can publish.

## What it is for

Peer-to-peer distribution shines when many people want the same large files:

- **Datasets:** research data, open government data, scientific archives.
- **Machine learning models:** weights and checkpoints that are slow to fetch from one server.
- **Open-source software:** Linux images, releases and their mirrors.
- **Games:** free games, patches and mods distributed with their authors' permission.
- **Public-domain and openly licensed media:** archives, recordings, films and books.

The catalog's default categories follow this list; `--category` replaces them.

## Running one

`tempest-tracker` is its own program (D-236), built from source (`cargo install --path crates/tracker-cli`, after building the website with trunk; see the README). It runs the peer-list service. With `--catalog FILE`, it also keeps a catalog in that SQLite file and serves its website at `/` and the listing API beside the peer-list service's, under `/tempest/v1/`. `tempest-tracker --help` lists every option, including invites, categories, relays and the limits on fetching manifests.

- **Relays (D-185, D-236):** a tracker doesn't run a relay. Run [`tempest-relay`](../protocol/relays.md) separately, and pass the identity it prints on startup as `--relay '<key>@host:port'`. Clients with `relays.from_trackers` on (D-186) then use that relay. Repeat `--relay` for more.

- **Listing is opt-in (D-129).** A bundle appears only when its sharer publishes it, with `tempest share --publish` or `tempest publish`, and only once the tracker has fetched its manifest from a peer sharing it (D-207). Publishers can edit and unpublish their listings.
- **Accounts are keys (D-128).** People sign in by signing a code with their Tempest key; there are no passwords to store or leak. Sessions are kept as hashes (D-206).
- **HTTPS** comes from a reverse proxy in front of the tracker; pass `--behind-proxy` so it reads clients' addresses and the host they reached from the proxy's headers.
- **Rate limits:** the catalog limits each client IP (an IPv6 /64) per action: asking for sign-in codes, signing in and polling, browsing and searching, publishing new bundles, editing, voting, commenting, reporting, and counted downloads. Each allows a burst, then one request per interval; requests over it get `rate_limited` with `retry_after_secs`, except that an announce over the publish or download limit still succeeds but lists or counts nothing. One IP also holds only a few sign-in codes at once. The defaults are in [`limits.rs`](../crates/tracker/src/limits.rs); `--catalog-limit` and `--challenges-per-ip` change them.
- **Moderation (D-225):** `tempest-tracker hide` and `unhide` take a listing off the catalog and back, and `ban` and `unban` do the same for a key, which hides all its listings and stops it listing, editing, voting, commenting and reporting. They change the catalog file itself, so they work whether or not a tracker is running on it, and a running tracker obeys at once. A hidden listing stays visible to its publisher. Each listing page has a Report link for signed-in people; `tempest-tracker reports` lists the reports, newest first, with the keys involved. A key's later report of a listing replaces its earlier one. Nothing happens to a reported listing until you act. The first key to list a bundle still owns it (D-206).
- **Backups:** the catalog file holds accounts, listings, votes, comments, reports, bans and download counts. The search index is rebuilt from it at startup (D-209).

The tracker website is described in [interfaces.md](interfaces.md). A tracker run without `--catalog` takes no listings: `tempest publish` to it fails with "the tracker doesn't take listings".
