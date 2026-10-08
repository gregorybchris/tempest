# Running a tracker

A Tempest tracker helps peers find each other, and can run a catalog: a website where people search the bundles that their sharers chose to list. This page is for people who run one. The peer-list service is protocol, specified in [`protocol/trackers.md`](../protocol/trackers.md); the catalog is an application on top of it (D-025).

## A neutral tool

Tempest is software, like a web server or a file system. The project publishes it and does not run a public tracker or catalog (D-031).

**If you run a tracker or a catalog, you are responsible for what it serves and lists,** under the laws that apply to you. Tempest does not check content, and its catalog has no moderation tools yet: no reports, takedowns or bans. Plan how you will handle complaints before you open a catalog to the public, for example by requiring invites (D-128) so that you know who can publish.

## What it is for

Peer-to-peer distribution shines when many people want the same large files:

- **Datasets:** research data, open government data, scientific archives.
- **Machine learning models:** weights and checkpoints that are slow to fetch from one server.
- **Open-source software:** Linux images, releases and their mirrors.
- **Games:** free games, patches and mods distributed with their authors' permission.
- **Public-domain and openly licensed media:** archives, recordings, films and books.

The catalog's default categories follow this list; `--category` replaces them.

## Running one

`tempest tracker` runs the peer-list service. With `--catalog FILE`, it also keeps a catalog in that SQLite file and serves its website at `/` and its API under `/catalog/v1/`. `tempest tracker --help` lists every option, including invites, categories, relays and the limits on fetching manifests.

- **Listing is opt-in (D-129).** A bundle appears only when its sharer publishes it, with `tempest share --publish` or `tempest catalog publish`, and only once the tracker has fetched its manifest from a peer sharing it (D-207). Publishers can edit and withdraw their listings.
- **Accounts are keys (D-128).** People sign in by signing a code with their Tempest key; there are no passwords to store or leak. Sessions are kept as hashes (D-206).
- **HTTPS** comes from a reverse proxy in front of the tracker; pass `--behind-proxy` so it reads clients' addresses and the host they reached from the proxy's headers.
- **Backups:** the catalog file holds accounts, listings, votes, comments and download counts. The search index is rebuilt from it at startup (D-209).

The catalog's UI is described in [interfaces.md](interfaces.md).
