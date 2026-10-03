# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Fetch three prespecified SMD v2 MIDI/audio pairs with bounded ZIP ranges."""

import argparse
import hashlib
import json
import zlib
from pathlib import Path

import fetch_violin_transitions as archive

SOURCE = "https://www.audiolabs-erlangen.de/resources/MIR/SMD/midi"
URL = "https://zenodo.org/records/13753319/files/SMD-piano_v2.zip"
CASES = {"Chopin_Op028-04_003_20100611-SMD": "train",
         "Chopin_Op028-15_006_20100611-SMD": "validation",
         "Beethoven_Op027No1-03_003_20090916-SMD": "validation"}


def fetch(root, lock=None):
    archive.URL = URL
    entries, directory_hash = archive.archive_entries()
    root.mkdir(parents=True, exist_ok=True)
    records = []
    for stem, split in CASES.items():
        for folder, suffix in (("wav_22050_mono", ".wav"), ("midi", ".mid")):
            names = [name for name in entries if name.startswith(folder + "/") and name.endswith(stem + suffix)]
            if len(names) != 1:
                raise ValueError(f"missing original SMD member {folder}/{stem}{suffix}: {names}")
            name = names[0]
            path = root / (stem + suffix)
            data = path.read_bytes() if path.exists() else archive.fetch_entry(entries[name])
            if len(data) != entries[name]["size"] or zlib.crc32(data) != entries[name]["crc"]:
                raise ValueError(f"cached SMD reference changed: {name}")
            path.write_bytes(data)
            records.append({"path": path.name, "member": name, "split": split,
                            "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)})
            print(name, len(data), flush=True)
    manifest = {"source": SOURCE, "archive": URL, "directory_sha256": directory_hash,
                "citation": "Müller, Konz, Bogler, Arifi-Müller: Saarland Music Data, ISMIR 2011",
                "terms": "CC BY-NC-SA 3.0; recordings remain local research inputs",
                "policy": "first 12 seconds; one training work, two prespecified validation works; no time warp",
                "files": records}
    path = root / "captures.json"
    if lock and json.loads(lock.read_text(encoding="utf-8")) != manifest:
        raise ValueError("SMD capture differs from the archived cohort lock")
    if path.exists() and json.loads(path.read_text(encoding="utf-8")) != manifest:
        raise ValueError("SMD capture differs from previous download")
    path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--lock", type=Path, help="verify original captures against the archived manifest")
    args = parser.parse_args()
    fetch(args.directory, args.lock)
