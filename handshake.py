import hashlib
import io
import os
import urllib.parse
import urllib.request
from bencode import bencode, bdecode

path = os.path.expanduser("~/Downloads/ubuntu-26.04-desktop-amd64.iso.torrent")
# path = os.path.expanduser("~/Downloads/alice.torrent")
with open(path, "rb") as f:
    data = bdecode(f)
# print(data)
announce_url = data[b'announce'].decode('utf-8')

my_peer_id = os.urandom(20)
info_hash = hashlib.sha1(bencode(data[b'info'])).digest()
with_query_params = urllib.parse.urlencode({
    "info_hash": info_hash,
    "peer_id": my_peer_id,
    "port": 6881,
    "uploaded": 0,
    "downloaded": 0,
    "left": data[b'info'][b'length'],
})
full_url_joined = urllib.parse.urljoin(announce_url, f"?{with_query_params}")
print(full_url_joined)

response = urllib.request.urlopen(full_url_joined)
reader = io.BufferedReader(io.BytesIO(response.read()))
response_data = bdecode(reader)
peers = response_data[b'peers']
peer = peers[0]

import socket
s = socket.socket(socket.AF_INET6, socket.SOCK_STREAM)
s.connect((peer[b'ip'], peer[b'port']))
s.send(b'\x13BitTorrent protocol')
s.send(b'\x00' * 8)
s.send(info_hash)
s.send(my_peer_id)
def assert_recv(s, expected):
    recvd = s.recv(len(expected))
    assert recvd == expected, f"Expected {expected.hex()}, but received {recvd.hex()}"
def decode_reserved_bits(reserved_bits: bytes):
    return {
        'DHT': bool(reserved_bits[7] & 0x01),  # BEP 5
        'Fast Extension': bool(reserved_bits[7] & 0x04),  # BEP 6
        'Extended Messaging': bool(reserved_bits[5] & 0x10),  # BEP 10
    }
assert_recv(s, b'\x13BitTorrent protocol')
features = s.recv(8)
print(decode_reserved_bits(features))
assert_recv(s, info_hash)
assert_recv(s, peer[b'peer id'])
# expect a bitfield message
length = s.recv(4)
assert_recv(s, b'\x05')
def myreceive(s, msglen):
    chunks = []
    bytes_recd = 0
    while bytes_recd < msglen:
        chunk = s.recv(min(msglen - bytes_recd, 2048))
        if chunk == b'':
            raise RuntimeError("socket connection broken")
        chunks.append(chunk)
        bytes_recd = bytes_recd + len(chunk)
    return b''.join(chunks)
bitfield = myreceive(s, int.from_bytes(length, 'big'))
print(len(bitfield)*8, len(data[b'info'][b'pieces']))
# compare bit length of bitfield to number of pieces in the torrent
# num_pieces = data[b'info'][b'pieces'].__len__() // 20
# assert len(bitfield) * 8 == num_pieces, f"Expected bitfield length {num_pieces}, but received {len(bitfield) * 8}"


# send an interested message
# s.send(b'\x00\x00\x00\x01\x02')
