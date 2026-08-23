import io

def at_end_of_container(data: io.BufferedReader) -> bool:
    return peek_byte(data) == b'e'

def bdecode_dict(data: io.BufferedReader) -> dict:
    result = {}
    while True:
        if at_end_of_container(data):
            data.read(1)
            break
        key = bdecode(data)
        value = bdecode(data)
        result[key] = value
    return result

def bdecode_list(data: io.BufferedReader) -> list:
    result = []
    while True:
        if at_end_of_container(data):
            data.read(1)
            break
        value = bdecode(data)
        result.append(value)
    return result

def peek_byte(data: io.BufferedReader) -> bytes:
    return data.peek(1)[:1]

def expect_byte(data: io.BufferedReader, expected: bytes) -> None:
    c = data.read(1)
    assert c == expected, f"Expected {expected} but got {c}"

def read_positive_int(data: io.BufferedReader) -> int:
    result = 0
    while (c := peek_byte(data)).isdigit():
        data.read(1)
        result = result * 10 + (c[0] - ord(b'0'))
    return result

def read_string(data: io.BufferedReader, length: int) -> bytes:
    result = data.read(length)
    assert len(result) == length
    return result

def read_int(data: io.BufferedReader) -> int:
    is_negative = False
    if peek_byte(data) == b'-':
        is_negative = True
        data.read(1)
    result = read_positive_int(data)
    expect_byte(data, b'e')
    if is_negative:
        result = -result
    return result

def bdecode(data: io.BufferedReader) -> object:
    tag = peek_byte(data)
    match tag:
        case b'd':
            data.read(1)
            return bdecode_dict(data)
        case b'l':
            data.read(1)
            return bdecode_list(data)
        case c if c.isdigit():
            length = read_positive_int(data)
            expect_byte(data, b':')
            return read_string(data, length)
        case b'i':
            data.read(1)
            return read_int(data)
        case c: raise NotImplementedError(c)

def bencode_helper(f: io.BytesIO, data: object) -> None:
    match data:
        case int():
            f.write(b'i')
            f.write(str(data).encode('utf-8'))
            f.write(b'e')
        case bytes():
            f.write(str(len(data)).encode('utf-8'))
            f.write(b':')
            f.write(data)
        case list():
            f.write(b'l')
            for item in data:
                bencode_helper(f, item)
            f.write(b'e')
        case dict():
            f.write(b'd')
            for key, value in data.items():
                bencode_helper(f, key)
                bencode_helper(f, value)
            f.write(b'e')
        case _: raise NotImplementedError(type(data))

def bencode(data: object) -> bytes:
    f = io.BytesIO()
    bencode_helper(f, data)
    return f.getvalue()

import unittest

class BdecodeTests(unittest.TestCase):
    def _decode(self, data: bytes) -> object:
        return bdecode(io.BufferedReader(io.BytesIO(data)))

    def test_bdecode_int_zero(self) -> None:
        result = self._decode(b'i0e')
        self.assertEqual(result, 0)

    def test_bdecode_positive_int(self) -> None:
        result = self._decode(b'i12e')
        self.assertEqual(result, 12)

    def test_bdecode_negative_int(self) -> None:
        result = self._decode(b'i-12e')
        self.assertEqual(result, -12)

    def test_bdecode_empty_string(self) -> None:
        result = self._decode(b'0:')
        self.assertEqual(result, b'')

    def test_bdecode_string(self) -> None:
        result = self._decode(b'7:abcdefg')
        self.assertEqual(result, b'abcdefg')

    def test_bdecode_empty_list(self) -> None:
        result = self._decode(b'le')
        self.assertEqual(result, [])

    def test_bdecode_list(self) -> None:
        result = self._decode(b'l3:abci17ee')
        self.assertEqual(result, [b'abc', 17])

    def test_bdecode_empty_dict(self) -> None:
        result = self._decode(b'de')
        self.assertEqual(result, {})

    def test_bdecode_dict(self) -> None:
        result = self._decode(b'd3:foo3:bare')
        self.assertEqual(result, {b'foo': b'bar'})

    def test_bdecode_list_of_dict(self) -> None:
        result = self._decode(b'ldedee')
        self.assertEqual(result, [{}, {}])

    def test_bdecode_dict_of_list(self) -> None:
        result = self._decode(b'd3:foolee')
        self.assertEqual(result, {b'foo': []})

class BencodeTests(unittest.TestCase):
    def test_bencode_int_zero(self) -> None:
        result = bencode(0)
        self.assertEqual(result, b'i0e')

    def test_bencode_positive_int(self) -> None:
        result = bencode(12)
        self.assertEqual(result, b'i12e')

    def test_bencode_negative_int(self) -> None:
        result = bencode(-12)
        self.assertEqual(result, b'i-12e')

    def test_bencode_empty_string(self) -> None:
        result = bencode(b'')
        self.assertEqual(result, b'0:')

    def test_bencode_string(self) -> None:
        result = bencode(b'abcdefg')
        self.assertEqual(result, b'7:abcdefg')

    def test_bencode_empty_list(self) -> None:
        result = bencode([])
        self.assertEqual(result, b'le')

    def test_bencode_list(self) -> None:
        result = bencode([b'abc', 17])
        self.assertEqual(result, b'l3:abci17ee')

    def test_bencode_empty_dict(self) -> None:
        result = bencode({})
        self.assertEqual(result, b'de')

    def test_bencode_dict(self) -> None:
        result = bencode({b'foo': b'bar'})
        self.assertEqual(result, b'd3:foo3:bare')

    def test_bencode_list_of_dict(self) -> None:
        result = bencode([{}, {}])
        self.assertEqual(result, b'ldedee')

    def test_bencode_dict_of_list(self) -> None:
        result = bencode({b'foo': []})
        self.assertEqual(result, b'd3:foolee')

if __name__ == '__main__':
    unittest.main()
