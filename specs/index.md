# Tempest

## Overview

We're going to create a peer-to-peer file sharing project called Tempest. It'll be based on the ideas of BitTorrent, but compatability is not an objective.

Tempest is primarily a teaching and research tool. It's a real, usable client, but it's also a didactic instrument for understanding peer-to-peer file sharing and for measuring the impact of individual features and parameter settings. Key objectives of Tempest, in priority order:

1. Conceptual clarity & understandability - the concepts should be easy to understand from the protocol and from the code, and the protocol should be easier to understand and implement than BitTorrent
2. Configurability & measurability - the client's behavior is driven by configuration, so the simulation system can measure how different features and settings perform under different conditions
3. Speed - it should be fast at downloading and uploading files
4. Beauty & usability - user interfaces should be intuitive and visually appealing

Tempest supports two kinds of use from the start: direct sends (sharing with a specific person via a link) and public swarms (sharing among many peers). Ideally the client can operate intelligently in either regime based on its configuration.

Non-goals: anonymity. Public sharing exposes your IP address, and Tempest says so plainly. Connections between peers are always encrypted.

See [decisions.md](decisions.md) for the log of design decisions and their rationale, [glossary.md](glossary.md) for terminology, [strategies.md](strategies.md) for the client's configurable download and upload strategies, [architecture.md](architecture.md) for how the code is organized, [interfaces.md](interfaces.md) for the user interfaces and their quality bar, and [roadmap.md](roadmap.md) for the implementation milestones.

## Protocol

The protocol should be maintained separately from the client implementation. We want other people to be able to implement their own clients based on the protocol specification. The protocol spec lives in the top-level [`protocol/`](../protocol/README.md) folder and must stand on its own, without depending on anything in `specs/`.

The goal of the protocol is to lay out as succinctly as possible the how someone needs to implement their client to be compatible with other Tempest clients, without including unnecessary implementation details or optimizations.

The benefits of having our own protocol are:

1. We can make things really easy to understand and create message and field names that are way more intuitive than what BitTorrent uses now. BitTorrent right now is sort of hard to understand, I think partly because the terminology is sort of confusing.
2. We can remove anything from BitTorrent that wasn't performant or well-designed and not support things that we've learned aren't great, making the code and protocol simpler.
3. We can jump straight to the advanced features that took years to gain acceptance and that some modern clients still don't fully implement.

## Learning guide

Separate from the protocol, we want people to be able to easily learn how Tempest works. This should be a guide with visual aids, like a blog post. It need not reference BitTorrent at all. Our target audience is not someone who has used BitTorrent, but someone who is new to peer-to-peer file sharing and wants to understand how it works.

The goal is to teach someone the fundamental concepts and the tricks that make it work so well in practice, without getting into historical details, confusing terminology, or implementation-specific minutiae. Implementation specifics are what the protocol specification is for.

## Client implementation

I'd like the whole thing to be implemented in Rust. I want the whole thing to compile so that it can be distributed easily. I'd like someone to be able to install this on their machine really easily whether they're on Windows, macOS, or Linux.

I'd like to have multiple interfaces for interacting with the client, including a terminal interface and a web interface. Both of these should also be written in Rust. The web interface should be really beautiful and extremely intuitive. We're looking for something that feels sort of retro, evoking 1980's hacker culture, but with a layout that's easy to understand at a glance and navigate efficiently. The terminal interface should be minimalistic but powerful, providing quick access to all essential features.

The backend should be really performant and written in async Rust, making use of multithreading to handle concurrent and parallel streaming of files efficiently.

## Tracker

The repo also includes a Tempest tracker that anyone can run, with its own server and web frontend, both written in Rust. It is branded like the client so everything feels like one ecosystem.

The tracker has two parts:

1. **Peer-list service.** Clients report which bundles they are serving, and the tracker tells them about other peers in the same swarm. This part is defined by the protocol spec.
2. **Catalog.** A web application with high-quality search, categories, ranking, comments, and easy filtering and sorting, so people can find what they are looking for. It communicates with clients continuously, so it reflects what is actually being served on the network. This part is an application built on top of the protocol, not part of it.

The user interface should be beautiful and extremely easy to use.

## Simulation

It's important to me that while we build this protocol and client that we have a robust system for simulation. I want to be able to make changes to the protocol and the client and test for different network configurations, file types, user load, etc, that we remain performant.

We should be able to configure different scenarios in YAML, run simulations, analyze the results, and make informed decisions about protocol and client improvements based on the data.

Simulation results are browsed in a web UI, also written in Rust, that reads results from a local database. It should be really beautiful, with high-quality interactive charts, held to the same quality bar as the client and tracker UIs. See [simulation.md](simulation.md) for the simulator's design.

## Feedback style

I want a lot of control over how this thing is built. I want there to be a lot of back and forth where AI agents provide me with context and ask me how I want something built. This is really important to me that I'm in the loop, that I'm consulted on most questions. Some things that are minor like how to name a message type in the protocol I'd really like to feel ownership of. I'd rather do a lot of question + answer than get the end result and feel like I didn't design the system.

That said, I'm relatively new to understanding how BitTorrent works. I haven't really even torrented that much myself. So I'll need to be given a fair amount of context and explanation in conjunction with being prompted for executive decisions.

I want to have control over terminology. I want to have control over which features we implement into the protocol. I want to have some control over how we structure the code.

## Phases

1. Improve this spec with AI in general for structure and clarification
2. Do research to update this spec based on new knowledge from internet searches about protocol features, Rust implementation practices, etc.
3. Create markdown specs laying out the Tempest protocol
4. Create a robust system for simulation and testing of a Tempest client.
5. Implement the Tempest client in Rust based on the finalized protocol specs.
6. Iterate on the protocol and client based on feedback from simulation and testing.
7. Create web interface for the Tempest client.
8. Create terminal interface for the Tempest client.
9. Create an intuitive learning guide for new users of the Tempest client.
10. Create the Tempest tracker catalog (search, categories, comments) and its web interface, reusing the client web UI's design. The tracker's peer-list service is built alongside the client in phase 5, since public swarms depend on it.

**Gate before heavy implementation:** before doing a lot of implementation, and before locking in a UI framework, build a test UI in Rust to check whether it reaches the quality bar for Tempest's UIs (client, tracker catalog and simulation results viewer).

## Code Quality

I want this project to be simple, easy to reason about, easy to read, etc. But I also want the quality of the code to be production-grade, maintainable, and scalable.

## Research

- Search generally to see if there are other projects like this, see if any are successful, see what similarities and differences there are in the objectives those projects have.
- Search for existing research papers, articles, or blog posts that discuss improvements to BitTorrent or similar peer-to-peer protocols. Consider the list of proposals on the BitTorrent enhancement page (BEPs) as a starting point. Go through each idea and condense it into 3 pieces: an objective description of the feature, an argument for including it, and argument against including it in Tempest.
- Search for tooling that could be useful for this project. We want to identify libraries, frameworks, and utilities that can help with networking, concurrency, simulation, and testing in Rust. Also frameworks for web interfaces and terminal interfaces. Also packages for making Rust projects more production quality and scalable.
