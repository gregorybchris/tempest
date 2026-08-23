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
with_query_params = urllib.parse.urlencode({
    "info_hash": hashlib.sha1(bencode(data[b'info'])).digest(),
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
print(response_data)


