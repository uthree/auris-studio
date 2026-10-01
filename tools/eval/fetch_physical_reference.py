# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Download a bounded real-instrument cohort from University of Iowa MIS.

Recordings stay in the chosen development directory. The dataset's terms and
recording pages are saved alongside original audio; no samples enter the product.
"""

import argparse
import hashlib
import json
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import quote, unquote, urljoin, urlsplit, urlunsplit
from urllib.request import urlopen

BASE = "https://theremin.music.uiowa.edu/"
DYNAMICS = {"pp": 0.35, "mf": 0.65, "ff": 0.95}
LOCK = Path(__file__).parent / "references" / "iowa-captures.json"


class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.urls = []

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            self.urls.extend(value for key, value in attrs if key == "href")


def safe_url(url):
    parts = urlsplit(url)
    return urlunsplit(parts._replace(path=quote(unquote(parts.path), safe="/")))


def cohort():
    for dynamic, velocity in DYNAMICS.items():
        for note, pitch in (("C3", 48), ("E3", 52), ("G3", 55),
                            ("C4", 60), ("E4", 64), ("G4", 67)):
            yield "piano", f"Piano.{dynamic}.{note}.aiff", [pitch], velocity
        for string, span, pitches in (
            ("sulE", "E2B2", range(40, 48)),
            ("sulD", "D3B3", range(50, 60)),
            ("sul_E", "E4B4", range(64, 72)),
        ):
            yield "guitar", f"Guitar.{dynamic}.{string}.{span}.mono.aif", list(pitches), velocity
        for string, span, pitches in (
            ("sulG", "G3B3", range(55, 60)),
            ("sulD", "D4B4", range(62, 72)),
            ("sulE", "E5B5", range(76, 84)),
        ):
            yield "violin", f"Violin.arco.{dynamic}.{string}.{span}.mono.aif", list(pitches), velocity


def fetch(root):
    root.mkdir(parents=True, exist_ok=True)
    pages = {}
    for model, page in (("terms", "MIS.html"), ("piano", "MISpiano.html"),
                        ("guitar", "MISguitar.html"), ("violin", "MISviolin2012.html")):
        with urlopen(BASE + page, timeout=60) as stream:
            raw = stream.read()
        (root / page).write_bytes(raw)
        parser = Links()
        parser.feed(raw.decode("utf-8", errors="replace"))
        pages[model] = {
            Path(unquote(urlsplit(url).path)).name: safe_url(urljoin(BASE, url))
            for url in parser.urls
        }
    captures = []
    expected = {capture["path"]: capture for capture in
                json.loads(LOCK.read_text(encoding="utf-8"))["captures"]} if LOCK.exists() else {}
    for model, name, pitches, velocity in cohort():
        url = pages[model][name]
        path = root / name
        if not path.exists():
            print(f"download {name}", flush=True)
            with urlopen(url, timeout=90) as stream:
                raw = stream.read(40_000_001)
            if len(raw) > 40_000_000 or raw[:4] != b"FORM":
                raise ValueError(f"invalid or oversized AIFF: {url}")
            path.write_bytes(raw)
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if expected and (name not in expected or digest != expected[name]["sha256"]):
            raise ValueError(f"recording differs from frozen reference: {name}")
        captures.append({"model": model, "path": name, "url": url,
                         "pitches": pitches, "velocity": velocity,
                         "sha256": digest})
    manifest = {"source": BASE + "MIS.html", "creator": "Lawrence Fritts / University of Iowa",
                "terms": "May be downloaded and used for any projects, without restrictions.",
                "captures": captures}
    (root / "captures.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    fetch(parser.parse_args().directory)
