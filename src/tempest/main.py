import hashlib
import os
from dataclasses import dataclass, field
from enum import StrEnum, auto
from ipaddress import IPv4Address
from queue import PriorityQueue
from typing import Annotated, Self
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

    index: Annotated[int, "Index of the piece that the sender has"]


@dataclass(frozen=True, kw_only=True)
class BitfieldMessage(Message):
    """Bitfield message indicating which pieces the sender has."""

    bitfield: Annotated[Bitfield, "Bitfield representing pieces the sender has"]


@dataclass(frozen=True, kw_only=True)
class RequestMessage(Message):
    """Request message indicating that the sender is requesting a specific piece."""

    index: Annotated[int, "Index of the piece being requested"]
    offset: Annotated[int, "Offset within the piece for the requested block"]
    length: Annotated[int, "Length of the block being requested"]


@dataclass(frozen=True, kw_only=True)
class PieceMessage(Message):
    """Piece message containing a block of data for a specific piece."""

    index: Annotated[int, "Index of the piece being sent"]
    offset: Annotated[int, "Offset within the piece for the block being sent"]
    block: Annotated[bytes, "Block of data being sent"]


@dataclass(frozen=True, kw_only=True)
class CancelMessage(Message):
    """Cancel message indicating that the a previous request is being canceled."""

    index: Annotated[int, "Index of the piece being canceled"]
    offset: Annotated[int, "Offset within the piece for the canceled block"]
    length: Annotated[int, "Length of the block being canceled"]


@dataclass(frozen=True, kw_only=True)
class Metadata:
    piece_length: Annotated[int, "Length of each piece in bytes"]
    # TODO: Update this to use Merkle tree
    piece_hashes: Annotated[list[Hash], "List of SHA256 hashes for each piece"]
    length: Annotated[int, "Total length of the file in bytes"]
    name: Annotated[str, "Name of the file"]

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


@dataclass
class ClientInfo:
    client: Annotated[Client, "Client object"]
    upload_speed: Annotated[float, "Upload speed in bytes per second"]
    download_speed: Annotated[float, "Download speed in bytes per second"]


@dataclass
class Network:
    clients: dict[IPv4Address, ClientInfo] = field(default_factory=dict)
    # use a heapq to simulate events on the network, such as message delivery and timeouts
    message_queue: PriorityQueue[tuple[float, Message]] = field(default_factory=PriorityQueue)

    @classmethod
    def rand_speed(cls, min_mbps: int, max_mbps: int) -> int:
        mbps = int.from_bytes(os.urandom(1), "big") % (max_mbps - min_mbps + 1) + min_mbps
        bytes_per_second = mbps * 1_000_000 / 8
        return int(bytes_per_second)

    def add_client(self, client: Client) -> None:
        upload_mbps_range = (10, 35)
        upload_speed_bps = self.rand_speed(*upload_mbps_range)

        download_mbps_range = (100, 500)
        download_speed_bps = self.rand_speed(*download_mbps_range)

        client_info = ClientInfo(
            client=client,
            upload_speed=upload_speed_bps,
            download_speed=download_speed_bps,
        )
        self.clients[client.ip_address] = client_info

    def send(self, message: Message) -> None:
        msg = "Network.send() not implemented"
        raise NotImplementedError(msg)


class DiscoveryMode(StrEnum):
    Tracker = auto()
    Dht = auto()
    Pex = auto()


@dataclass
class Client:
    """Peer that downloads pieces of a file from other peers and verifies them."""

    discovery_mode: DiscoveryMode
    ip_address: IPv4Address
    network: Network
    client_id: UUID = field(default_factory=uuid4)

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


def main() -> None:
    message_bytes: bytes = b"Hello World"
    message_hash = hashlib.sha256(message_bytes)
    message_hash_hex: str = message_hash.hexdigest()
    message_hash_bytes: Hash = message_hash.digest()
    console.print(f"SHA256 hash of '{message_bytes.decode()}': [red]{message_hash_hex}")
    console.print(f"SHA256 hash bytes count: {len(message_hash_bytes)}")

    network = Network()

    n_peers = 2
    for _ in range(n_peers):
        discovery_mode = DiscoveryMode.Tracker
        console.print(f"Discovery mode: {discovery_mode.value}")
        ip_address = IPv4Address(int.from_bytes(os.urandom(4), "big"))
        console.print(f"Client IP address: {ip_address}")
        client = Client(
            discovery_mode=discovery_mode,
            ip_address=ip_address,
            network=network,
        )
        console.print(f"Client peer ID: {client.client_id}")

        network.add_client(client)


if __name__ == "__main__":
    main()
