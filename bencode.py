import io
import os

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

unittest.main()
