# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Fetch a bounded subset of the public TU-Note violin archive for local analysis.

The original recordings are CC BY-ND 4.0. Keep recordings and extracted excerpts
under target/; this tool does not redistribute them or add release dependencies.
"""

import argparse
import hashlib
import json
import struct
import urllib.request
import zlib
from pathlib import Path

URL = "https://api-depositonce.tu-berlin.de/server/api/core/bitstreams/69330aa6-e073-4923-90d2-fece37dccb9d/content"
SOURCE = "https://depositonce.tu-berlin.de/handle/11303/7527"
IDS = (17, 18, 19, 20, 21, 22, 41, 42, 43, 44, 45, 46,
       65, 66, 67, 68, 69, 70, 89, 90, 91, 92, 93, 94)


def cohort():
    names = ["File_Lists/list_TwoNote.txt", "File_Lists/list_Single.txt"]
    for identifier in IDS:
        names.extend((f"Segments/TwoNote/TwoNote_DPA_{identifier:02}.txt",
                      f"WAV/TwoNote/DPA/TwoNote_DPA_{identifier:02}.wav"))
    return names


def byte_range(start, end):
    request = urllib.request.Request(URL, headers={"Range": f"bytes={start}-{end}"})
    with urllib.request.urlopen(request, timeout=60) as response:
        if response.status != 206:
            raise ValueError("archive server must support bounded HTTP ranges")
        content_range = response.headers["Content-Range"]
        total = int(content_range.split("/")[1])
        if not content_range.startswith(f"bytes {start}-{end}/"):
            raise ValueError("unexpected HTTP byte range")
        data = response.read(end - start + 2)
        if len(data) != end - start + 1:
            raise ValueError("truncated or oversized HTTP range")
        return data, total


def archive_entries():
    _, length = byte_range(0, 0)
    tail, _ = byte_range(length - 65536, length - 1)
    offset = tail.rfind(b"PK\x05\x06")
    if offset < 0:
        raise ValueError("ZIP footer missing")
    footer = struct.unpack("<4s4H2LH", tail[offset:offset + 22])
    count, size, start = footer[4:7]
    if footer[1] or footer[2] or footer[3] != count or size > 1024 * 1024:
        raise ValueError("unsupported ZIP directory")
    directory, _ = byte_range(start, start + size - 1)
    entries = {}
    cursor = 0
    for _ in range(count):
        header = struct.unpack("<4s6H3L5H2L", directory[cursor:cursor + 46])
        if header[0] != b"PK\x01\x02":
            raise ValueError("invalid ZIP directory entry")
        name = directory[cursor + 46:cursor + 46 + header[10]].decode("utf-8")
        entries[name] = {"name": name, "method": header[4], "crc": header[7],
                         "compressed": header[8], "size": header[9], "offset": header[16]}
        cursor += 46 + header[10] + header[11] + header[12]
    if cursor != size:
        raise ValueError("ZIP directory count does not match its size")
    return entries, hashlib.sha256(directory).hexdigest()


def fetch_entry(entry):
    if entry["compressed"] > 20_000_000 or entry["size"] > 40_000_000:
        raise ValueError("archive member exceeds the reference budget")
    offset = entry["offset"]
    data, _ = byte_range(offset, offset + 29)
    header = struct.unpack("<4s5H3L2H", data)
    if header[0] != b"PK\x03\x04" or header[3] != entry["method"]:
        raise ValueError("ZIP local header mismatch")
    start = offset + 30 + header[-2] + header[-1]
    data, _ = byte_range(start, start + entry["compressed"] - 1)
    if entry["method"] == 8:
        inflater = zlib.decompressobj(-15)
        data = inflater.decompress(data, entry["size"] + 1)
        if not inflater.eof:
            raise ValueError("invalid or oversized compressed member")
    elif entry["method"] != 0:
        raise ValueError("unsupported compression method")
    if len(data) != entry["size"] or zlib.crc32(data) != entry["crc"]:
        raise ValueError("archive member size or CRC mismatch")
    return data


def fetch(root, names, lock=None):
    root.mkdir(parents=True, exist_ok=True)
    entries, digest = archive_entries()
    records = []
    for name in names:
        entry = entries[name]
        # Paths come from the explicit cohort, never from untrusted ZIP traversal.
        destination = root / Path(name).name
        if destination.exists():
            data = destination.read_bytes()
            if len(data) != entry["size"] or zlib.crc32(data) != entry["crc"]:
                raise ValueError(f"cached reference changed: {name}")
        else:
            data = fetch_entry(entry)
            destination.write_bytes(data)
        records.append({"member": name, "path": destination.name,
                        "sha256": hashlib.sha256(data).hexdigest()})
        print(f"{name}: {len(data)} bytes", flush=True)
    manifest = {"source": SOURCE, "archive": URL, "directory_sha256": digest,
                "license": "https://creativecommons.org/licenses/by-nd/4.0/",
                "creators": "Henrik von Coler, Jonas Margraf, Paul Schuladen; violin: Michiko Feuerlein",
                "files": records}
    if lock and manifest != json.loads(lock.read_text(encoding="utf-8")):
        raise ValueError("downloaded transition cohort differs from its lock")
    (root / "captures.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--member", action="append", help="explicit archive member; defaults to the fixed 24-transition cohort")
    parser.add_argument("--lock", type=Path, help="verify the fixed download against an archived capture manifest")
    args = parser.parse_args()
    fetch(args.directory, args.member or cohort(), args.lock)
