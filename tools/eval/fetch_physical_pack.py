# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Fetch bounded, original Iowa notes and Salamander drum recordings for local calibration."""

import argparse
import hashlib
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from urllib.parse import unquote, urljoin, urlsplit
from urllib.request import urlopen

from fetch_physical_reference import BASE, DYNAMICS, Links, safe_url

DRUM_URL = "https://archive.org/download/SalamanderDrumkit/salamanderDrumkit.tar.bz2"
DRUM_BYTES = 387611727
DRUM_MD5 = "af8e2067668a7f438e7d981877fb771f"


def download(path, url, limit=120_000_000):
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


def cohort():
    for dynamic, velocity in DYNAMICS.items():
        for string, span, pitches in (("sulE", "E1B1", range(28, 36)),
                                     ("sulA", "C2B2", range(36, 48)),
                                     ("sulD", "C3B3", range(48, 60))):
            yield "bass", "MISdoublebass.html", f"Bass.pizz.{dynamic}.{string}.{span}.aiff", list(pitches), velocity
        for span, pitches in (("C3B3", range(48, 60)), ("C4B4", range(60, 72))):
            yield "mallet", "Mismarimba.html", f"Marimba.yarn.{dynamic}.{span}.aif", list(pitches), velocity
        for span, pitches in (("C5B5", range(72, 84)), ("C6B6", range(84, 96))):
            yield "bell", "MISbells.html", f"bells.plastic.{dynamic}.{span}.aif", list(pitches), velocity


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
    pages = {}
    for page in ("MIS.html", "MISdoublebass.html", "Mismarimba.html", "MISbells.html"):
        raw = urlopen(BASE + page, timeout=60).read()
        (args.directory / page).write_bytes(raw)
        links = Links()
        links.feed(raw.decode("utf-8", errors="replace"))
        pages[page] = {Path(unquote(urlsplit(url).path)).name: safe_url(urljoin(BASE, url)) for url in links.urls}

    def fetch(record):
        model, page, name, pitches, velocity = record
        url = pages[page][name]
        digest = download(args.directory / name, url)
        print(name, flush=True)
        return {"model": model, "page": BASE + page, "path": name, "url": url,
                "pitches": pitches, "velocity": velocity, "sha256": digest}

    with ThreadPoolExecutor(max_workers=4) as pool:
        captures = list(pool.map(fetch, cohort()))
    archive = args.directory / "salamanderDrumkit.tar.bz2"
    print("download original Salamander archive", flush=True)
    sha = download(archive, DRUM_URL, DRUM_BYTES)
    if archive.stat().st_size != DRUM_BYTES or hashlib.md5(archive.read_bytes()).hexdigest() != DRUM_MD5:
        raise ValueError("original Salamander archive checksum mismatch")
    manifest = {"iowa": {"source": BASE + "MIS.html", "creator": "Lawrence Fritts / University of Iowa",
                  "terms": "Any projects, without restrictions"},
                "salamander": {"source": "https://github.com/endolith/Salamander-Drumkit",
                  "creator": "Alexander Holm", "url": DRUM_URL, "path": archive.name,
                  "sha256": sha, "bytes": DRUM_BYTES,
                  "terms": "Original CC BY-SA 3.0; author dedicated sampled instruments to public domain in 2022",
                  "author_statement": "https://rytmenpinne.wordpress.com/2022/03/04/good-news-everyone/"},
                "captures": captures}
    if previous and previous != manifest:
        raise ValueError("reference differs from frozen capture manifest")
    (args.directory / "captures.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
