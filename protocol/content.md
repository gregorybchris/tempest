# Content

> **Status: Draft (version 1).**

This section defines how the content of a bundle is described and verified: how a file is split into **chunks**, how chunks are hashed into a **tree** whose **root** identifies the file, and how a **manifest** describes a whole bundle.

The goal is that a downloader can verify every chunk the moment it arrives from an untrusted peer, using only the bundle's ID.

## Chunks

A file is divided into **chunks** of 16 KiB (16,384 bytes).

- Chunk *i* covers bytes `[i × 16384, (i + 1) × 16384)` of the file.
- The last chunk MAY be shorter than 16 KiB. It MUST NOT be padded with extra bytes.
- An empty file consists of exactly one empty chunk.

The number of chunks in a file of `S` bytes is therefore `max(1, ⌈S / 16384⌉)`.

**Rationale.** 16 KiB is small enough that a corrupt chunk wastes little download and pinpoints the peer that sent it, and large enough that hashing overhead stays small. It's the unit used by existing protocols (BitTorrent's requests and BitTorrent v2's tree leaves), so their tuning knowledge carries over.

## The chunk tree

Each file's chunks are hashed into a binary Merkle tree. The tree's top hash, the file's **root**, identifies the file's contents.

### Leaves

The leaf hash of chunk *i* is:

```text
leaf(i) = SHA-256(0x00 ‖ chunk_i)
```

### Width and padding

The tree's **width** `W` is the smallest power of two greater than or equal to the number of chunks `N`. Leaf positions `N` through `W − 1` are **padding leaves**. Each padding leaf has the value of 32 zero bytes. A padding leaf is not a hash of anything.

### Inner nodes

Each inner node combines its two children, left then right:

```text
node(left, right) = SHA-256(0x01 ‖ left ‖ right)
```

Starting from the `W` leaves, pairs are combined level by level until one hash remains. That hash is the file's **root**. A file with one chunk (`W = 1`) has `root = leaf(0)`.

```text
Example: a file with 3 chunks (N = 3, W = 4)

                    root = node(n01, n23)
                   /                      \
        n01 = node(L0, L1)          n23 = node(L2, P)
          /          \                /          \
   L0 = leaf(0)  L1 = leaf(1)   L2 = leaf(2)   P = 32 zero bytes
        |             |              |
     chunk 0       chunk 1        chunk 2
```

**Rationale.**

- **Per-chunk verification.** A downloader verifies a chunk using a short proof: the sibling hashes along its path to the root. It doesn't need to wait for a large piece of data, and a bad chunk identifies exactly the peer that sent it.
- **Tiny descriptions.** A file of any size is identified by one 32-byte root.
- **Padding to a perfect tree.** Chunk *i* is always leaf *i*, and every node's position follows from simple arithmetic regardless of file size.
- **The `0x00` and `0x01` prefixes (domain separation).** Without them, an inner node's hash would equal the leaf hash of a 64-byte chunk made of its two children's hashes, so one hash could stand for two different things. With them, leaves and inner nodes can never be confused, without relying on the verifier knowing the file size. Certificate Transparency ([RFC 6962](https://www.rfc-editor.org/rfc/rfc6962)) uses the same technique.

**Implementer notes.**

- Verify each chunk in memory as it arrives. There's no need to read data back from disk to check it.
- Requests, proofs and their verification are defined in [messages.md](messages.md#data).
- Because padding leaves are fixed values, the hash of a subtree that is entirely padding depends only on its height. It can be computed once and cached.

### Test vectors

These roots were computed with a short throwaway script. They should be checked against the Rust implementation once it exists.

| Input | Size (bytes) | Chunks `N` | Width `W` | Root (hex) |
| --- | ---: | ---: | ---: | --- |
| empty file | 0 | 1 | 1 | `6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d` |
| the 3 ASCII bytes `abc` | 3 | 1 | 1 | `609f6e36d2405585188d5cfd761f407c7cc46a7d3f314c88270469dde315fcd1` |
| 16,384 bytes of `0x61` (`a`) | 16,384 | 1 | 1 | `efc218e05045c10736574abdc1b7ce7f1e1a0e8366215664189852745d06eb90` |
| 16,385 bytes of `0x61` | 16,385 | 2 | 2 | `553a8ddffb209ad5cf57755f33ab1e8a5191f5261e1442720d087085ec47f7a7` |
| 40,000 bytes where byte *i* = *i* mod 256 | 40,000 | 3 | 4 | `cf6700fcd8e64ac06a512eac8b2f94101104f0e199c5fbfa2c4b43adb562c2f8` |

## The manifest

A **bundle** is one or more files (and possibly directories and symlinks) offered together. A bundle is described by its **manifest**.

Every field in the manifest is part of the bundle's identity. Two manifests that differ in any field, even a comment or a timestamp, have different bundle IDs and form separate swarms.

### Bundle-level fields

| Field | Meaning |
| --- | --- |
| name | Suggested display name for the bundle (typically the top-level folder or file name). MUST be a valid path segment (see [Paths](#paths)) |
| comment | Free-text description from the creator. MAY be empty |
| created | When the bundle was created (a [timestamp](#timestamps)) |
| creator | Name and version of the tool that created the bundle |
| entries | The list of entries described below |

Where a client writes a bundle on disk is policy. Typically it creates a folder called `name` in the user's download location and writes the entries inside it.

### Entries

Each entry has a **path** (relative to the bundle's top level) and a **kind**:

| Kind | Additional fields |
| --- | --- |
| file | size (bytes), root (32 bytes), executable flag, modification time (a [timestamp](#timestamps)) |
| symlink | target path |
| directory | none |

- A manifest MUST contain at least one entry.
- Entries MUST be sorted in ascending order by the bytes of their UTF-8 encoded paths. No two entries may have the same path.
- A directory entry MUST NOT be present for a directory that contains any other entry. Directories that contain entries exist implicitly, so only empty directories have entries.
- A file entry with size 0 MUST have the root `SHA-256(0x00)` (see [Chunks](#chunks)).
- The total number of chunks across all files MUST NOT exceed 2³² − 1, since chunk indexes are 32-bit (see [messages.md](messages.md#chunk-numbering)).

### Paths

Paths are designed so that any valid bundle can be written identically on Windows, macOS and Linux.

- A path MUST be UTF-8 in Unicode Normalization Form C (NFC).
- Segments are separated by `/`. A path MUST NOT begin or end with `/`, and MUST NOT exceed 4,096 bytes.
- Each segment:
  - MUST NOT be empty, `.` or `..`
  - MUST NOT exceed 255 bytes
  - MUST NOT contain control characters (U+0000 to U+001F, and U+007F) or any of `< > : " \ | ? *`
  - MUST NOT end with `.` or a space
  - MUST NOT be a name reserved on Windows (see below)
- Where this section compares paths **case-insensitively**, it means comparing them after Unicode simple case folding (the `C` and `S` mappings of `CaseFolding.txt`, Unicode 15.1).
- No two different paths may be equal when compared case-insensitively. This covers entries' paths and the paths of the directories that implicitly contain them. For example, files `A/b` and `a/c` cannot both appear.
- No entry's path may lie inside the path of a file or symlink entry, compared case-insensitively. For example, a file `a` and a file `A/b` cannot both appear.

**Reserved names.** A segment is reserved if the part before its first `.`, with any trailing spaces removed, matches one of the following when compared case-insensitively:

```text
CON  PRN  AUX  NUL  CONIN$  CONOUT$
COM0 … COM9  COM¹  COM²  COM³
LPT0 … LPT9  LPT¹  LPT²  LPT³
```

So `con`, `CON.txt`, `Con .tar.gz` and `nul.` are all reserved.

**Symlinks.** A symlink's target is a relative path that follows the segment rules above, except that it MAY contain `..` segments. Resolve the target one segment at a time, starting from the symlink's own directory (`..` steps up one level). Then:

- no step may leave the bundle's top level;
- no intermediate or final path along the way may be the path of a symlink entry, compared case-insensitively. Symlinks MUST NOT point through or at other symlinks.

**Rationale.** The rules forbid anything that is invalid on at least one major operating system. If a manifest is valid, every client can write it exactly as described, so no client needs its own renaming rules. Creators with incompatible names (such as `a:b`) rename them before sharing.

- **Path length.** The 4,096-byte limit is a sanity bound. Windows clients should use long-path support, since the download folder adds to every path's length.
- **Case.** Case-insensitive rules matter because macOS and Windows usually treat `Photo.jpg` and `photo.jpg` as the same file, so a file written to `A/b` could otherwise land inside a symlink named `a`.
- **NFC.** It matters because the same visible name can be encoded in more than one way.
- **Symlinks.** Checking a symlink's target step by step, and forbidding chains of symlinks, stops a combination of individually harmless-looking links from pointing outside the bundle.

### Timestamps

A timestamp is a signed integer number of milliseconds since 1970-01-01 00:00:00 UTC.

**Implementer notes.** Some file systems store coarser times. FAT32, for example, rounds to 2 seconds. A client writing a file restores the modification time as closely as its file system allows.

### Encoding

The manifest is encoded with the primitive types defined in [messages.md](messages.md#encoding):

```text
Manifest {
  version: u8            // manifest format version; MUST be 1
  name:    string
  comment: string
  created: i64           // timestamp
  creator: string
  entries: list<Entry>   // sorted by path, as described above
}

Entry {
  kind: u8               // 0 = file, 1 = symlink, 2 = directory
  path: string
  …                      // followed by the fields for its kind:
}

  file:      size: u64, root: hash, executable: bool, modified: i64
  symlink:   target: string
  directory: (no further fields)
```

Because every primitive has exactly one valid encoding and entries have a fixed order, a manifest has exactly one byte form. A client MUST reject a manifest with an unknown `version` or `kind`, or with any bytes left over after the last entry. A creator MUST NOT produce a manifest larger than 64 MiB.

### Bundle ID

The bundle ID is computed from the manifest's encoded bytes:

```text
bundle ID = SHA-256(0x02 ‖ manifest bytes)
```

The bundle ID identifies the bundle everywhere: in links, in peer discovery, and when two peers agree on what they're exchanging. As text, bundle IDs and roots are written as 64 lowercase hexadecimal characters.

**Rationale.** Every kind of hash in Tempest has its own one-byte prefix: `0x00` for a leaf, `0x01` for an inner node and `0x02` for a manifest. So a bundle ID can never equal a file root or a tree node.

### Manifest test vector

A manifest named `example`, with an empty comment, `created` and every `modified` = 1700000000000 (2023-11-14 22:13:20 UTC), creator `tempest 0.1.0`, and four entries:

- `a.txt`: a file containing the 3 bytes `abc`
- `docs/empty.txt`: an empty file
- `docs/link`: a symlink to `../a.txt`
- `empty-dir`: an empty directory

| Bytes (hex) | Meaning |
| --- | --- |
| `01` | version = 1 |
| `000000076578616d706c65` | name = "example" |
| `00000000` | comment = "" |
| `0000018bcfe56800` | created = 1700000000000 ms |
| `0000000d74656d7065737420302e312e30` | creator = "tempest 0.1.0" |
| `00000004` | entries: count = 4 |
| `00` | entry 1: kind = 0 (file) |
| `00000005612e747874` | ↳ path = "a.txt" |
| `0000000000000003` | ↳ size = 3 |
| `609f6e36d2405585188d5cfd761f407c7cc46a7d3f314c88270469dde315fcd1` | ↳ root = leaf("abc") |
| `00` | ↳ executable = 0 |
| `0000018bcfe56800` | ↳ modified |
| `00` | entry 2: kind = 0 (file) |
| `0000000e646f63732f656d7074792e747874` | ↳ path = "docs/empty.txt" |
| `0000000000000000` | ↳ size = 0 |
| `6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d` | ↳ root = SHA-256(0x00) |
| `00` | ↳ executable = 0 |
| `0000018bcfe56800` | ↳ modified |
| `01` | entry 3: kind = 1 (symlink) |
| `00000009646f63732f6c696e6b` | ↳ path = "docs/link" |
| `000000082e2e2f612e747874` | ↳ target = "../a.txt" |
| `02` | entry 4: kind = 2 (directory) |
| `00000009656d7074792d646972` | ↳ path = "empty-dir" |

Full manifest (212 bytes):

```text
01000000076578616d706c65000000000000018bcfe568000000000d74656d7065737420302e312e30000000040000000005612e7478740000000000000003609f6e36d2405585188d5cfd761f407c7cc46a7d3f314c88270469dde315fcd1000000018bcfe56800000000000e646f63732f656d7074792e74787400000000000000006e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d000000018bcfe568000100000009646f63732f6c696e6b000000082e2e2f612e7478740200000009656d7074792d646972
```

Bundle ID:

```text
207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a
```

### Safety rules

Clients MUST treat manifests as untrusted input.

- A client MUST reject a manifest that breaks any rule in this section.
- A client SHOULD create symlinks only after writing every file and directory, and MUST NOT follow symlinks when writing files.
- A client MUST verify that a file's received data produces the root listed in the manifest before treating the file as complete.

**Rationale.** Per-file roots mean a file's identity doesn't depend on the bundle it appears in. The same file in two different bundles has the same root even when the manifests differ, which leaves room for a future feature that finds a file in other swarms. Descriptive fields (name, comment, dates) are included for a better user experience, accepting that they split swarms when two people share the same files separately.
