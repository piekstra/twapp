#!/usr/bin/env python3
"""Put the largest PNG representation first without changing ICNS image data.

Some icon consumers decode the first available image instead of choosing a size.
Run after regenerating icons: python3 scripts/order-icon.py
"""
import argparse
from pathlib import Path
import struct


def order_icon(path: Path) -> None:
    data = path.read_bytes()
    if len(data) < 8 or data[:4] != b"icns" or struct.unpack(">I", data[4:8])[0] != len(data):
        raise ValueError("invalid ICNS header or file length")
    chunks = []
    offset = 8
    while offset < len(data):
        if offset + 8 > len(data):
            raise ValueError("truncated ICNS entry")
        size = struct.unpack(">I", data[offset + 4:offset + 8])[0]
        if size < 8 or offset + size > len(data):
            raise ValueError("invalid ICNS entry length")
        chunk = data[offset:offset + size]
        if chunk[8:16] == b"\x89PNG\r\n\x1a\n" and len(chunk) >= 32:
            width, height = struct.unpack(">II", chunk[24:32])
            pixels = width * height
        else:
            pixels = 0
        chunks.append((pixels, chunk))
        offset += size
    chunks.sort(key=lambda entry: entry[0], reverse=True)
    path.write_bytes(data[:8] + b"".join(chunk for _, chunk in chunks))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", nargs="?", type=Path,
                        default=Path(__file__).resolve().parents[1] / "src-tauri/icons/icon.icns")
    order_icon(parser.parse_args().path)
