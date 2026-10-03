# /// script
# requires-python = ">=3.10"
# dependencies = ["requests>=2.31", "numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Fetch a fixed, bounded EGFxSet clean/BluesDriver cohort using HTTP ZIP ranges."""

import argparse
import hashlib
import io
import json
import struct
import time
import zipfile
import zlib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
import requests
import soundfile as sf
from scipy.io import wavfile
from scipy.signal import resample_poly

RECORD = "https://zenodo.org/api/records/7044411"
OPEN = (64, 59, 55, 50, 45, 40)


def range_bytes(url, start, end):
    for attempt in range(4):
        response = requests.get(url + f"?range_start={start}",
                                headers={"Range": f"bytes={start}-{end}"}, timeout=45, stream=True)
        try:
            if response.status_code == 429 or response.status_code >= 500:
                time.sleep(2**attempt)
                continue
            response.raise_for_status()
            if response.status_code != 206:
                raise ValueError("server did not honor bounded byte range")
            if not response.headers.get("Content-Range", "").startswith(f"bytes {start}-{end}/"):
                raise ValueError("incorrect byte range returned")
            data = bytearray()
            for chunk in response.iter_content(65536):
                data.extend(chunk)
                if len(data) > end - start + 1:
                    raise ValueError("byte range response exceeds request")
            if len(data) != end - start + 1:
                raise ValueError("incomplete byte range response")
            return bytes(data)
        finally:
            response.close()
    raise RuntimeError("reference server remained unavailable")


def cohort():
    """One canonical string for every pitch E2..D6; no duplicate frettings."""
    for string, base in enumerate(OPEN, 1):
        count = 23 if string == 1 else OPEN[string - 2] - base
        for fret in range(count):
            yield string, fret, base + fret


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--lock", type=Path,
                        default=Path(__file__).parent / "references/electric-notes.json")
    args = parser.parse_args()
    root = args.directory
    root.mkdir(parents=True, exist_ok=True)
    response = requests.get(RECORD, timeout=30)
    response.raise_for_status()
    record = response.json()
    files = {file["key"]: file for file in record["files"]}
    captures = []
    for effect in ("Clean", "BluesDriver"):
        file = files[effect + ".zip"]
        url = file["links"]["self"]
        size = file["size"]
        start = size - 131072
        tail = range_bytes(url, start, size - 1)
        archive = zipfile.ZipFile(io.BytesIO(tail))
        members = {info.filename: info for info in archive.infolist()}

        def fetch(spec, effect=effect, members=members, start=start, url=url, size=size):
            pickup, string, fret, pitch = spec
            member = f"{effect}/{pickup}/{string}-{fret}.wav"
            info = members[member]
            path = root / "original" / member
            path.parent.mkdir(parents=True, exist_ok=True)
            if not path.exists():
                offset = info.header_offset + start
                # The local header precedes the compressed data; allow at most 4 KiB extras.
                data = range_bytes(url, offset, min(size - 1, offset + info.compress_size + 4095))
                fields = struct.unpack("<4s5H3I2H", data[:30])
                if fields[0] != b"PK\x03\x04" or fields[3] != zipfile.ZIP_DEFLATED:
                    raise ValueError("unexpected ZIP member format")
                content_start = 30 + fields[-2] + fields[-1]
                raw = zlib.decompress(data[content_start:content_start + info.compress_size], -15)
                if len(raw) != info.file_size or zlib.crc32(raw) != info.CRC:
                    raise ValueError("reference ZIP member checksum mismatch")
                path.write_bytes(raw)
            raw = path.read_bytes()
            if len(raw) != info.file_size or zlib.crc32(raw) != info.CRC:
                raise ValueError("cached reference checksum mismatch")
            audio, rate = sf.read(path, always_2d=True)
            audio = audio.mean(axis=1)
            if rate != 48000 or not np.isfinite(audio).all():
                raise ValueError("unexpected EGFxSet recording")
            onset = int(np.flatnonzero(abs(audio) >= abs(audio).max() * 0.02)[0])
            prepared = resample_poly(audio[max(0, onset - 48):], 1, 2)
            prepared = np.pad(prepared[:72000], (0, max(0, 72000 - len(prepared))))
            target = root / "notes" / f"{effect}-{pickup}-{string}-{fret}.wav"
            target.parent.mkdir(exist_ok=True)
            # libsndfile adds a timestamped PEAK chunk to float WAVs. The SciPy writer
            # stores the same float32 samples without wall-clock-dependent metadata.
            wavfile.write(target, 24000, prepared.astype(np.float32))
            return {"id": target.stem, "effect": effect, "pickup": pickup,
                    "string": string, "fret": fret, "pitch": pitch, "velocity": 0.8,
                    "split": "train" if pitch % 2 == 0 else "validation",
                    "original": path.relative_to(root).as_posix(),
                    "original_sha256": hashlib.sha256(raw).hexdigest(),
                    "path": target.relative_to(root).as_posix(),
                    "sha256": hashlib.sha256(target.read_bytes()).hexdigest(),
                    "onset_sample_48000": onset}

        specs = [(pickup, string, fret, pitch) for pickup in ("Neck", "Bridge")
                 for string, fret, pitch in cohort()]
        with ThreadPoolExecutor(max_workers=3) as pool:
            for row in pool.map(fetch, specs):
                captures.append(row)
                print(row["id"], flush=True)
    manifest = {"source": "https://zenodo.org/records/7044411", "license": record["metadata"]["license"],
                "authors": [creator["name"] for creator in record["metadata"]["creators"]],
                "archives": {key: {"bytes": files[key]["size"], "checksum": files[key]["checksum"]}
                             for key in ("Clean.zip", "BluesDriver.zip")},
                "seconds": 3, "notes": captures}
    locked = root / "notes.json"
    if args.lock.exists() and json.loads(args.lock.read_text(encoding="utf-8")) != manifest:
        raise ValueError("reference differs from the versioned cohort")
    if locked.exists() and json.loads(locked.read_text(encoding="utf-8")) != manifest:
        raise ValueError("reference differs from the frozen cohort")
    locked.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
