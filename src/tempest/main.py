import hashlib
import os
from dataclasses import dataclass, field
from enum import StrEnum, auto
from ipaddress import IPv4Address, ip_address
from typing import Self
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


@dataclass
class File:
    data: bytes

    @classmethod
    def new_random(cls, size: int) -> Self:
        data = os.urandom(size)
        return cls(data=data)


@dataclass
class Tracker:
    """Server that keeps a table of info hash -> peer addresses."""

    table: dict[str, list[IPv4Address]] = field(default_factory=dict)


class DiscoveryMode(StrEnum):
    Tracker = auto()
    Dht = auto()
    Pex = auto()


@dataclass
class Client:
    """Peer that downloads pieces of a file from other peers and verifies them."""

    discovery_mode: DiscoveryMode
    peer_id: UUID = field(default_factory=uuid4)

    # TODO: Implement in client the logic to request the final pieces from all peers
    # and send cancel requests once the piece is received.
    # This is called "endgame mode" in the BitTorrent protocol.

    def receive(self, message: Message) -> None:
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
                msg = f"Piece message not implemented. index: {index}, offset: {offset}, block length: {len(block)}"
                raise NotImplementedError(msg)
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
    message_bytes: bytes = b"Hello World"
    message_hash = hashlib.sha256(message_bytes)
    message_hash_hex: str = message_hash.hexdigest()
    message_hash_bytes: Hash = message_hash.digest()
    console.print(f"SHA256 hash of '{message_bytes.decode()}': [red]{message_hash_hex}")
    console.print(f"SHA256 hash bytes count: {len(message_hash_bytes)}")

    discovery_mode = DiscoveryMode.Tracker
    console.print(f"Discovery mode: {discovery_mode.value}")
    client = Client(discovery_mode=discovery_mode)
    console.print(f"Client peer ID: {client.peer_id}")

    address = ip_address("192.168.1.10")  # IPv4Address
    console.print(f"IP address: {address}, type: {type(address).__name__}")


if __name__ == "__main__":
    main()
