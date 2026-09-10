# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Generate a local, initially blinded seed-listening report from measured audio.

The HTML uses relative WAV URLs and native audio controls; it uploads nothing and
needs no server or remote scripts. A stable hash ordering assigns A-H independently
of model scores. Blinding is a presentation aid, not protection against inspecting
the HTML source. Ratings start blank and are never inferred from model scores.

    uv run tools/eval/seed_listening.py --manifest corpus.json \
        --aesthetics aesthetics.json --clap clap.json --output listening.html
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
from urllib.parse import quote

TEMPLATE = Path(__file__).with_suffix(".html")
SHUFFLE_VERSION = "auris-seed-listening-v1"


def canonical(value: object) -> bytes:
    return json.dumps(
        value,
        sort_keys=True,
        ensure_ascii=False,
        allow_nan=False,
        separators=(",", ":"),
    ).encode("utf-8")


def script_json(value: object) -> str:
    """Escape even closing-script text and Unicode line separators in embedded JSON."""
    return (
        canonical(value)
        .decode("utf-8")
        .replace("&", "\\u0026")
        .replace("<", "\\u003c")
        .replace(">", "\\u003e")
        .replace("\u2028", "\\u2028")
        .replace("\u2029", "\\u2029")
    )


def asset_url(value: dict, manifest_directory: Path, output_directory: Path) -> str:
    """Resolve a local asset and encode a relative URL without a file:// origin."""
    path = Path(value["path"])
    if not path.is_absolute():
        path = manifest_directory / path
    path = path.resolve(strict=True)
    if not path.is_file():
        raise ValueError(f"Asset is not a file: {path}")
    try:
        relative = os.path.relpath(path, output_directory)
    except ValueError as error:
        raise ValueError(
            "Report and assets must be on the same filesystem drive"
        ) from error
    return quote(Path(relative).as_posix(), safe="/")


def number(value: object) -> float | None:
    if value is None:
        return None
    if (
        isinstance(value, bool)
        or not isinstance(value, (float, int))
        or not math.isfinite(value)
    ):
        raise ValueError(f"Expected a finite numeric measurement, got {value!r}")
    return float(value)


def build_data(
    manifest: dict,
    aesthetics: dict,
    clap: dict,
    manifest_directory: Path,
    output_directory: Path,
) -> dict:
    """Validate the listening cohort and build the browser's local-only data payload."""
    if manifest.get("schema_version") != 1:
        raise ValueError("Unsupported listening manifest schema")
    presets, seeds = manifest["presets"], manifest["seeds"]
    if not presets or len(set(presets)) != len(presets):
        raise ValueError("Presets must be nonempty and unique")
    if not seeds or len(seeds) > 26 or len(set(seeds)) != len(seeds):
        raise ValueError("Provide 1 to 26 unique seeds")
    if any(not isinstance(seed, int) or isinstance(seed, bool) for seed in seeds):
        raise ValueError("Seeds must be integers")
    corpus = hashlib.sha256(canonical(manifest)).hexdigest()
    groups = {preset: {} for preset in presets}
    for label, entry in manifest["files"].items():
        preset, seed = entry["preset"], entry["seed"]
        if preset not in groups or seed not in seeds or seed in groups[preset]:
            raise ValueError(f"Unexpected or duplicate preset/seed: {label}")
        duration = number(entry["excerpt"]["duration_seconds"])
        if duration is None or duration <= 0:
            raise ValueError(f"Excerpt must have positive duration: {label}")
        scores = aesthetics.get(label, {})
        aggregate = clap.get("files", {}).get(label, {}).get("aggregate", {})
        identity = hashlib.sha256(
            f"{SHUFFLE_VERSION}\0{preset}\0{seed}".encode()
        ).hexdigest()
        groups[preset][seed] = {
            "id": identity[:20],
            "order": identity,
            "label": label,
            "seed": seed,
            "duration": duration,
            "excerpt": asset_url(
                entry["excerpt"], manifest_directory, output_directory
            ),
            "wav": asset_url(entry["wav"], manifest_directory, output_directory),
            "project": asset_url(
                entry["project"], manifest_directory, output_directory
            ),
            "scores": {
                "CE": number(scores.get("CE")),
                "PQ": number(scores.get("PQ")),
                "CLAP": number(aggregate.get("positive_cosine")),
            },
            "symbolic": entry.get("symbolic", {}),
            "excerpt_conditions": {
                key: value
                for key, value in entry["excerpt"].items()
                if key not in {"path", "sha256", "duration_seconds"}
            },
        }
    result = []
    for preset, entries in groups.items():
        if set(entries) != set(seeds):
            raise ValueError(f"Missing seed samples for {preset}")
        samples = sorted(entries.values(), key=lambda sample: sample["order"])
        durations = [sample["duration"] for sample in samples]
        if max(durations) - min(durations) > 0.05:
            raise ValueError(f"Unequal excerpt duration within {preset}")
        targets = [
            number(
                sample["excerpt_conditions"].get("normalization", {}).get("target_lufs")
            )
            for sample in samples
        ]
        level_matched = all(target is not None for target in targets)
        if level_matched and max(targets) - min(targets) > 0.05:
            raise ValueError(f"Unequal excerpt loudness targets within {preset}")
        for index, sample in enumerate(samples):
            sample.pop("order")
            sample["blind_label"] = chr(ord("A") + index)
        result.append(
            {"preset": preset, "samples": samples, "level_matched": level_matched}
        )
    return {
        "schema_version": 1,
        "corpus_sha256": corpus,
        "shuffle_version": SHUFFLE_VERSION,
        "groups": result,
        "conditions": manifest.get("listening_conditions", {}),
    }


def generate(
    manifest_path: Path, aesthetics_path: Path, clap_path: Path, output_html: Path
) -> Path:
    """Create a fresh HTML report; never overwrite a source, media file or existing report."""
    manifest_path, aesthetics_path, clap_path, output_html = (
        Path(path).resolve()
        for path in (manifest_path, aesthetics_path, clap_path, output_html)
    )
    if output_html.suffix.lower() != ".html":
        raise ValueError("Output must be an .html file")
    if output_html.exists():
        raise ValueError(f"Refusing to overwrite {output_html}")
    values = [
        json.loads(path.read_text(encoding="utf-8"))
        for path in (manifest_path, aesthetics_path, clap_path)
    ]
    data = build_data(*values, manifest_path.parent, output_html.parent)
    template = TEMPLATE.read_text(encoding="utf-8")
    marker = "/* LISTENING_DATA */ null"
    if template.count(marker) != 1:
        raise ValueError("Listening template must contain exactly one data placeholder")
    html = template.replace(marker, script_json(data))
    output_html.parent.mkdir(parents=True, exist_ok=True)
    with output_html.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(html)
    return output_html


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("manifest", "aesthetics", "clap"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        print(generate(args.manifest, args.aesthetics, args.clap, args.output))
    except (ValueError, OSError, KeyError, TypeError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
