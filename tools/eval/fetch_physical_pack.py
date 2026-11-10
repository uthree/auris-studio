# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Fetch the original Salamander drum recordings for local calibration."""

import argparse
import hashlib
import json
from pathlib import Path
from urllib.request import urlopen

DRUM_URL = "https://archive.org/download/SalamanderDrumkit/salamanderDrumkit.tar.bz2"
DRUM_BYTES = 387611727
DRUM_MD5 = "af8e2067668a7f438e7d981877fb771f"


def download(path, url, limit):
    if not path.exists():
        partial = path.with_suffix(path.suffix + ".part")
        with urlopen(url, timeout=60) as response, partial.open("wb") as output:
            size = 0
            while chunk := response.read(1 << 20):
                size += len(chunk)
                if size > limit:
                    raise ValueError("reference exceeds download budget")
                output.write(chunk)
        partial.replace(path)
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--lock", type=Path)
    args = parser.parse_args()
    args.directory.mkdir(parents=True, exist_ok=True)
    if args.lock:
        previous = json.loads(args.lock.read_text(encoding="utf-8"))
    elif (args.directory / "captures.json").exists():
        previous = json.loads((args.directory / "captures.json").read_text(encoding="utf-8"))
    else:
        previous = None
    archive = args.directory / "salamanderDrumkit.tar.bz2"
    print("download original Salamander archive", flush=True)
    sha = download(archive, DRUM_URL, DRUM_BYTES)
    if archive.stat().st_size != DRUM_BYTES or hashlib.md5(archive.read_bytes()).hexdigest() != DRUM_MD5:
        raise ValueError("original Salamander archive checksum mismatch")
    manifest = {"salamander": {"source": "https://github.com/endolith/Salamander-Drumkit",
                  "creator": "Alexander Holm", "url": DRUM_URL, "path": archive.name,
                  "sha256": sha, "bytes": DRUM_BYTES,
                  "terms": "Original CC BY-SA 3.0; author dedicated sampled instruments to public domain in 2022",
                  "author_statement": "https://rytmenpinne.wordpress.com/2022/03/04/good-news-everyone/"}}
    if previous and previous != manifest:
        raise ValueError("reference differs from frozen capture manifest")
    (args.directory / "captures.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
