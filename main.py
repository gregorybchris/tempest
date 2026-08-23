import hashlib
import os
from dataclasses import dataclass, field
from enum import StrEnum, auto
from ipaddress import IPv4Address, ip_address
from typing import Self, Set
from uuid import UUID, uuid4

from rich.console import Console

console = Console()

Hash = bytes
Bitfield = bytes


class MessageType(StrEnum):
    Choke = auto()
    Unchoke = auto()
    Interested = auto()
    NotInterested = auto()
    Have = auto()
    Bitfield = auto()
    Request = auto()
    Piece = auto()
    Cancel = auto()


@dataclass(frozen=True, kw_only=True)
class Message:
    pass


@dataclass(frozen=True, kw_only=True)
class ChokeMessage(Message):
    pass


@dataclass(frozen=True, kw_only=True)
class UnchokeMessage(Message):
    pass


@dataclass(frozen=True, kw_only=True)
class InterestedMessage(Message):
    pass


@dataclass(frozen=True, kw_only=True)
class NotInterestedMessage(Message):
    pass


@dataclass(frozen=True, kw_only=True)
class HaveMessage(Message):
    """Have message indicating that the sender has a specific piece."""

    index: int  # Index of the piece that the sender has


@dataclass(frozen=True, kw_only=True)
class BitfieldMessage(Message):
    """Bitfield message indicating which pieces the sender has."""

    bitfield: Bitfield  # Bitfield representing pieces the sender has

@dataclass(frozen=True, kw_only=True)
class RequestMessage(Message):
    """Request message indicating that the sender is requesting a specific piece."""

    index: int  # Index of the piece being requested
    offset: int  # Offset within the piece for the requested block
    length: int  # Length of the block being requested


@dataclass(frozen=True, kw_only=True)
class PieceMessage(Message):
    """Piece message containing a block of data for a specific piece."""

    index: int  # Index of the piece being sent
    offset: int  # Offset within the piece for the block being sent
    block: bytes  # Block of data being sent


@dataclass(frozen=True, kw_only=True)
class CancelMessage(Message):
    """Cancel message indicating that the a previous request is being canceled."""

    index: int  # Index of the piece being canceled
    offset: int  # Offset within the piece for the canceled block
    length: int  # Length of the block being canceled


@dataclass(frozen=True, kw_only=True)
class Metadata:
    piece_length: int  # Length of each piece in bytes
    # TODO: Update this to use Merkle tree
    piece_hashes: list[Hash]  # List of SHA256 hashes for each piece
    length: int  # Total length of the file in bytes
    name: str  # Name of the file

    def hash(self) -> Hash:
        """Compute the SHA256 hash of the metadata."""
        hasher = hashlib.sha256()
        hasher.update(self.piece_length.to_bytes(8, "big"))
        for piece_hash in self.piece_hashes:
            hasher.update(piece_hash)
        hasher.update(self.length.to_bytes(8, "big"))
        hasher.update(self.name.encode("utf-8"))
        return hasher.digest()

    @classmethod
    def from_file(cls, file: File, piece_length: int = 1024) -> Metadata:
        piece_hashes = []
        for i in range(0, len(file.data), piece_length):
            piece = file.data[i : i + piece_length]
            piece_hash = hashlib.sha256(piece).digest()
            piece_hashes.append(piece_hash)
        return Metadata(piece_length=piece_length, piece_hashes=piece_hashes, length=len(file.data), name=file.name)


@dataclass
class File:
    name: str
    data: bytes

    @classmethod
    def new_random(cls, name: str, size: int) -> Self:
        data = os.urandom(size)
        return cls(name=name, data=data)


@dataclass
class PartialFile:
    metadata: Metadata
    # if we have a piece, the hash is guaranteed to match
    pieces: dict[int, bytes] = field(default_factory=dict)

    def add_piece(self, index: int, piece: bytes) -> None:
        assert index not in self.pieces
        assert index < len(self.metadata.piece_hashes)
        expected_hash = self.metadata.piece_hashes[index]
        actual_hash = hashlib.sha256(piece).digest()
        assert expected_hash == actual_hash
        self.pieces[index] = piece

    def is_completed(self) -> bool:
        return len(self.pieces) == len(self.metadata.piece_hashes)

    @classmethod
    def from_file(cls, metadata: Metadata, file: File) -> Self:
        pieces = {}
        for i in range(len(metadata.piece_hashes)):
            piece = file.data[i * metadata.piece_length : (i + 1) * metadata.piece_length]
            pieces[i] = piece
        return cls(metadata=metadata, pieces=pieces)

    def as_bitfield_message(self) -> BitfieldMessage:
        """Return a BitfieldMessage representing the pieces we have."""
        num_pieces = len(self.metadata.piece_hashes)
        bitfield_length = (num_pieces + 7) // 8
        bitfield = bytearray(bitfield_length)
        for i in range(num_pieces):
            if i in self.pieces:
                byte_index = i // 8
                bit_index = i % 8
                bitfield[byte_index] |= 1 << (7 - bit_index)
        return BitfieldMessage(bitfield=bytes(bitfield))


type AnnounceEvent = Literal["started", "stopped", "completed"]


@dataclass
class Announce:
    info_hash: Hash
    peer_id: UUID
    port: int
    uploaded: int
    downloaded: int
    left: int
    event: Optional[AnnounceEvent]


@dataclass
class Tracker:
    """Server that keeps a table of info hash -> peer addresses."""

    table: dict[str, list[IPv4Address]] = field(default_factory=dict)

    # def receive_announce(self, message: Announce) -> None:


class DiscoveryMode(StrEnum):
    Tracker = auto()
    Dht = auto()
    Pex = auto()


@dataclass(frozen=True)
class Connection:
    metadata_hash: Hash
    peer_id: UUID
    id: UUID = field(default_factory=uuid4)


@dataclass
class Client:
    """Peer that downloads pieces of a file from other peers and verifies them."""

    discovery_mode: DiscoveryMode
    peer_id: UUID = field(default_factory=uuid4)
    metadata: Dict[Hash, Metadata] = field(default_factory=dict)
    # connection ID -> connection
    connections: dict[UUID, ConnectionInfo] = field(default_factory=dict)
    partial_files: dict[Hash, PartialFile] = field(default_factory=dict)

    # TODO: Implement in client the logic to request the final pieces from all peers
    # and send cancel requests once the piece is received.
    # This is called "endgame mode" in the BitTorrent protocol.

    def add_metadata(self, metadata: Metadata) -> None:
        hash = metadata.hash()
        assert hash not in self.metadata
        self.metadata[hash] = metadata

    def connect(self, peer: UUID, metadata_hash: Hash) -> Connection:
        assert metadata_hash in self.metadata
        connection = Connection(peer_id=peer_id, metadata_hash=metadata_hash)
        assert connection.id not in self.connections
        self.connections[connection.id] = connection
        if metadata_hash not in self.partial_files:
            self.partial_files[metadata_hash] = PartialFile(metadata=self.metadata[metadata_hash])
        return connection

    def receive_connection_hash(self, connection: Connection, metadata_hash: Hash) -> None:
        assert self.connections[connection.id].metadata_hash == metadata_hash

    def receive_connection_peer_id(self, connection: Connection, peer_id: UUID) -> None:
        assert self.connections[connection.id].peer_id == peer_id

    def receive(self, connection: Connection, message: Message) -> None:
        match message:
            case ChokeMessage():
                msg = "Choke message not implemented"
                raise NotImplementedError(msg)
            case UnchokeMessage():
                msg = "Unchoke message not implemented"
                raise NotImplementedError(msg)
            case InterestedMessage():
                msg = "Interested message not implemented"
                raise NotImplementedError(msg)
            case NotInterestedMessage():
                msg = "Not Interested message not implemented"
                raise NotImplementedError(msg)
            case HaveMessage(index=index):
                msg = f"Have message not implemented. index: {index}"
                raise NotImplementedError(msg)
            case BitfieldMessage(bitfield=bitfield):
                msg = f"Bitfield message not implemented. bitfield: {bitfield.hex()}"
                raise NotImplementedError(msg)
            case RequestMessage(index=index, offset=offset, length=length):
                msg = f"Request message not implemented. index: {index}, offset: {offset}, length: {length}"
                raise NotImplementedError(msg)
            case PieceMessage(index=index, offset=offset, block=block):
                info_hash = connection.metadata_hash
                partial_file = self.partial_files[info_hash]
                # TODO: handle blocks
                assert offset == 0
                block_length = len(block)
                piece_length = partial_file.metadata.piece_length
                file_size = partial_file.metadata.length
                leftover_piece_length = file_size % piece_length
                if index == len(partial_file.metadata.piece_hashes) - 1 and leftover_piece_length != 0:
                    # Last piece might be smaller than the piece length
                    assert block_length == leftover_piece_length
                else:
                    assert block_length == piece_length
                partial_file.add_piece(index=index, piece=block)
            case CancelMessage(index=index, offset=offset, length=length):
                msg = f"Cancel message not implemented. index: {index}, offset: {offset}, length: {length}"
                raise NotImplementedError(msg)
            case _:
                msg_0 = f"Unknown message type: {type(message).__name__}"
                raise NotImplementedError(msg_0)


@dataclass
class MerkleTree:
    """Merkle tree for a file, used to verify pieces of the file."""


@dataclass
class Network:
    pass

    # TODO: Network needs to store the per client upload and download speeds
    # TODO: Network needs to track the heapq to simulate events on the network


def main() -> None:
    # message_bytes: bytes = b"Hello World"
    # message_hash = hashlib.sha256(message_bytes)
    # message_hash_hex: str = message_hash.hexdigest()
    # message_hash_bytes: Hash = message_hash.digest()
    # console.print(f"SHA256 hash of '{message_bytes.decode()}': [red]{message_hash_hex}")
    # console.print(f"SHA256 hash bytes count: {len(message_hash_bytes)}")

    # discovery_mode = DiscoveryMode.Tracker
    # console.print(f"Discovery mode: {discovery_mode.value}")
    # client = Client(discovery_mode=discovery_mode)
    # console.print(f"Client peer ID: {client.peer_id}")

    # address = ip_address("192.168.1.10")  # IPv4Address
    # console.print(f"IP address: {address}, type: {type(address).__name__}")

    file = File.new_random("hi_chris", size=1024)
    file_metadata = Metadata.from_file(file=file, piece_length=17)  # a perfectly normal number
    info_hash = file_metadata.hash()

    partial_file = PartialFile.from_file(file_metadata, file)
    msg = partial_file.as_bitfield_message()
    console.print(bin(int.from_bytes(msg.bitfield, byteorder="big")))

    a = Client(discovery_mode=DiscoveryMode.Tracker)
    b = Client(discovery_mode=DiscoveryMode.Tracker)
    tracker = Tracker()
    # announce that b has the file
    tracker.receive_announce(Announce(info_hash=info_hash,
                                      peer_id=a.peer_id, port=6881, uploaded=0,
                                      downloaded=0, left=0, event="started"))
    # announce that a wants the file
    peers = tracker.receive_announce(Announce(info_hash=info_hash,
                                              peer_id=b.peer_id, port=6881, uploaded=0,
                                              downloaded=0, left=file_metadata.length, event="started"))
    for peer in peers:
        a_connection = a.connect(peer=peer, metadata_hash=info_hash)
        b_connection = b.connect(peer=a.peer_id, metadata_hash=info_hash)
        b_connection.receive_connection_hash(connection=b_connection, metadata_hash=info_hash)
        b_connection.receive_connection_peer_id(connection=b_connection, peer_id=a.peer_id)
        a.receive(a_connection, BitfieldMessage.have_file())


if __name__ == "__main__":
    main()
