# Tempest Protocol

> **Status: Draft, complete for version 1.** Every section is written. Details may change as implementation and simulation reveal problems.

This folder specifies the Tempest peer-to-peer file sharing protocol. It describes what a client must do to interoperate with other Tempest clients, and nothing more. How a client chooses which data to request, whom to upload to, or how to store data on disk is up to each client, except where this spec says otherwise.

## Sections

| File | Covers | Status |
| --- | --- | --- |
| [content.md](content.md) | How files are split into chunks, hashed into a tree, and described by a manifest | Draft |
| [connections.md](connections.md) | How peers connect, identify each other and encrypt | Draft |
| [messages.md](messages.md) | The messages peers exchange and the rules for exchanging them | Draft |
| [discovery.md](discovery.md) | How peers find each other: links, LAN discovery, trackers, peer exchange | Draft |
| [trackers.md](trackers.md) | The tracker API | Draft |
| [relays.md](relays.md) | How relays lend addresses, forward traffic, and help peers connect directly | Draft |

## Conventions

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **NOT RECOMMENDED**, **MAY**, and **OPTIONAL** in this specification are to be interpreted as described in [BCP 14](https://www.rfc-editor.org/info/bcp14) ([RFC 2119](https://www.rfc-editor.org/rfc/rfc2119), [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174)) when, and only when, they appear in all capitals. Lowercase words carry their ordinary English meaning.

Each section separates three kinds of text:

- **Rules** use the capitalized keywords above and are binding.
- **Rationale** explains why a rule exists. It's non-binding.
- **Implementer notes** give practical advice that is not required for interoperability.

Other notation:

- `‖` means byte concatenation.
- `0x00` is a single byte with the given hexadecimal value.
- Sizes use binary units: 1 KiB = 1,024 bytes.
- `SHA-256(x)` is the 32-byte SHA-256 digest of `x` ([FIPS 180-4](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)).

## Protocol and policy

Tempest distinguishes the **protocol**, the rules every client must follow to interoperate, from **policy**, the choices each client makes on its own. For example, the protocol defines how a peer requests a chunk, but which chunk to request next is policy. This spec defines the protocol. Where it helps, it names a recommended default policy, marked as such.
