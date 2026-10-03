# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "soundfile==0.14.0"]
# ///
"""Fetch only frozen VocalSet sung-vowel captures, then reproduce the prepared cohort."""

import argparse
import hashlib
import json
import struct
import zlib
from pathlib import Path
from urllib.request import Request, urlopen

from choir_reference import prepare
from prepare_physical_reference import manifest_digest

REFERENCES = Path(__file__).parent / "references"


def decode_entry(blob, capture):
    """Check bounded ZIP metadata, decompression length, CRC and original audio SHA-256."""
    if len(blob) < 30 or blob[:4] != b"PK\x03\x04":
        raise ValueError("invalid local ZIP header")
    header = struct.unpack_from("<4s5H3L2H", blob)
    filename_length, extra_length = header[-2:]
    if len(blob) < 30 + filename_length + extra_length:
        raise ValueError("truncated local ZIP metadata")
    if header[3] != 8 or extra_length > 1024 or capture["size"] > 16_000_000:
        raise ValueError("unexpected ZIP compression or entry size")
    name = blob[30:30 + filename_length].decode("utf-8")
    if name != capture["zip_path"]:
        raise ValueError("ZIP entry name differs from frozen capture")
    start = 30 + filename_length + extra_length
    compressed = blob[start:start + capture["compressed"]]
    if len(compressed) != capture["compressed"]:
        raise ValueError("truncated compressed recording")
    inflater = zlib.decompressobj(-15)
    raw = inflater.decompress(compressed, capture["size"] + 1)
    if not inflater.eof or inflater.unused_data or len(raw) != capture["size"]:
        raise ValueError("unexpected decompressed recording size")
    if zlib.crc32(raw) != capture["crc"] or hashlib.sha256(raw).hexdigest() != capture["sha256"]:
        raise ValueError("recording checksum differs from frozen source")
    return raw


def cohort_lock(manifest):
    """Store readable extraction metadata and fingerprints without duplicating pitch guides."""
    notes = [{key: value for key, value in note.items() if key != "bends"}
             | {"bends_sha256": manifest_digest(note["bends"])} for note in manifest["notes"]]
    return {key: value for key, value in manifest.items() if key != "notes"} | {
        "manifest_sha256": manifest_digest(manifest), "notes": notes}


def fetch(root, capture_path=REFERENCES / "choir-captures.json",
          note_path=REFERENCES / "choir-notes.json"):
    """Download 60 bounded recordings rather than the complete 2 GB archive."""
    lock = json.loads(capture_path.read_text(encoding="utf-8"))
    root.mkdir(parents=True, exist_ok=True)
    for capture in lock["captures"]:
        name = capture["path"]
        if Path(name).name != name or "/" in name or "\\" in name:
            raise ValueError("capture paths must be filenames")
        path = root / name
        if not path.exists():
            start = capture["offset"]
            end = start + 30 + len(capture["zip_path"].encode()) + 1024 + capture["compressed"] - 1
            url = lock["archive_url"] + f"&start={start}"
            request = Request(url, headers={"Range": f"bytes={start}-{end}",
                                            "User-Agent": "Auris-Choir-Calibration"})
            with urlopen(request, timeout=60) as stream:
                expected = f"bytes {start}-{end}/{lock['archive_bytes']}"
                if stream.status != 206 or stream.headers.get("Content-Range") != expected:
                    raise ValueError("server must return the exact requested ZIP range")
                blob = stream.read(end - start + 2)
            if len(blob) != end - start + 1:
                raise ValueError("truncated or oversized ZIP range")
            path.write_bytes(decode_entry(blob, capture))
        if hashlib.sha256(path.read_bytes()).hexdigest() != capture["sha256"]:
            raise ValueError(f"source hash mismatch: {path}")
        print(f"verified {name}", flush=True)
    (root / "NOTICE.txt").write_text(
        "VocalSet: A Singing Voice Dataset (1.1 archive, 1.2 record).\n"
        "Julia Wilkins, Prem Seetharaman, Alison Wahl, Bryan Pardo.\n"
        "Source: https://doi.org/10.5281/zenodo.1442513\n"
        "License: Creative Commons Attribution 4.0 International.\n"
        "https://creativecommons.org/licenses/by/4.0/\n"
        "Prepared notes: onset-cropped, mono, resampled to 24 kHz; no time stretching or looping.\n",
        encoding="utf-8")
    manifest = prepare(root, lock["captures"])
    if cohort_lock(manifest) != json.loads(note_path.read_text(encoding="utf-8")):
        raise ValueError("prepared notes differ from frozen extraction; check dependency versions")
    (root / "notes.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    fetch(parser.parse_args().directory)
