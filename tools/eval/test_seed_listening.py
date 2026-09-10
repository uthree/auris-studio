import copy
import json
from pathlib import Path
from urllib.parse import unquote

import pytest
from seed_listening import TEMPLATE, build_data, generate, script_json


def inputs(tmp_path: Path):
    asset_dir = tmp_path / "audio #日本語"
    asset_dir.mkdir()
    manifest = {
        "schema_version": 1,
        "presets": ["rock", "city-pop"],
        "seeds": list(range(101, 109)),
        "files": {},
    }
    aesthetics, clap = {}, {"files": {}}
    for preset in manifest["presets"]:
        for seed in manifest["seeds"]:
            label = f"{preset}-s{seed}"
            paths = {}
            for kind, suffix in [
                ("excerpt", ".excerpt.wav"),
                ("wav", ".wav"),
                ("project", ".auris"),
            ]:
                path = asset_dir / f"{label}{suffix}"
                path.write_bytes(b"test local asset")
                paths[kind] = {"path": str(path)}
            paths["excerpt"].update(
                {"duration_seconds": 20.0, "normalization": {"target_lufs": -20.0}}
            )
            manifest["files"][label] = {
                "preset": preset,
                "seed": seed,
                **paths,
                "symbolic": {"rhythm_signature": [0, 2, 3]},
            }
            aesthetics[label] = {"CE": 7.1, "PQ": 8.2}
            clap["files"][label] = {"aggregate": {"positive_cosine": 0.3}}
    return manifest, aesthetics, clap


def test_shuffling_is_repeatable_and_independent_of_model_scores_and_input_order(
    tmp_path,
):
    manifest, aesthetics, clap = inputs(tmp_path)
    original = copy.deepcopy(manifest)
    first = build_data(manifest, aesthetics, clap, tmp_path, tmp_path / "report")
    reordered = copy.deepcopy(manifest)
    reordered["files"] = dict(reversed(list(reordered["files"].items())))
    for value in aesthetics.values():
        value["CE"] = -999
    second = build_data(reordered, aesthetics, clap, tmp_path, tmp_path / "report")
    for before, after in zip(first["groups"], second["groups"]):
        assert [(s["seed"], s["blind_label"]) for s in before["samples"]] == [
            (s["seed"], s["blind_label"]) for s in after["samples"]
        ]
        assert [s["blind_label"] for s in before["samples"]] == list("ABCDEFGH")
        assert sorted(s["seed"] for s in before["samples"]) == list(range(101, 109))
    assert manifest == original
    assert first["corpus_sha256"] == second["corpus_sha256"]


def test_urls_are_encoded_relative_paths_and_link_to_original_lossless_assets(tmp_path):
    manifest, aesthetics, clap = inputs(tmp_path)
    output_directory = tmp_path / "report"
    data = build_data(manifest, aesthetics, clap, tmp_path, output_directory)
    for group in data["groups"]:
        for sample in group["samples"]:
            for field in ("excerpt", "wav", "project"):
                url = sample[field]
                assert url.startswith("../audio%20%23")
                assert "file:" not in url and "http" not in url
                assert (output_directory / unquote(url)).resolve().is_file()


def test_missing_scores_stay_unmeasured_and_never_become_ratings(tmp_path):
    manifest, _, _ = inputs(tmp_path)
    data = build_data(manifest, {}, {}, tmp_path, tmp_path)
    assert "ratings" not in data and "set_diversity" not in data
    for group in data["groups"]:
        for sample in group["samples"]:
            assert sample["scores"] == {"CE": None, "PQ": None, "CLAP": None}
            assert sample["symbolic"]["rhythm_signature"] == [0, 2, 3]


@pytest.mark.parametrize(
    "change", ["missing", "duplicate", "duration", "nonfinite", "badseed"]
)
def test_rejects_incomplete_or_misaligned_cohorts(tmp_path, change):
    manifest, aesthetics, clap = inputs(tmp_path)
    entry = manifest["files"]["rock-s101"]
    if change == "missing":
        del manifest["files"]["rock-s101"]
    elif change == "duplicate":
        manifest["files"]["duplicate"] = copy.deepcopy(entry)
    elif change == "duration":
        entry["excerpt"]["duration_seconds"] = 21.0
    elif change == "nonfinite":
        aesthetics["rock-s101"]["CE"] = float("nan")
    else:
        manifest["seeds"][0] = True
    with pytest.raises(ValueError):
        build_data(manifest, aesthetics, clap, tmp_path, tmp_path)


def test_script_json_cannot_break_out_of_script_context():
    value = {"text": "</script><img src=x onerror=alert(1)> & \u2028\u2029"}
    escaped = script_json(value)
    assert "</script>" not in escaped and "<" not in escaped and ">" not in escaped
    assert "\\u2028" in escaped and "\\u2029" in escaped
    assert json.loads(escaped) == value


def test_generate_writes_self_contained_report_without_modifying_inputs(tmp_path):
    manifest, aesthetics, clap = inputs(tmp_path)
    manifest["listening_conditions"] = {"note": "</script><script>alert('x')</script>"}
    paths = [
        tmp_path / name for name in ("manifest.json", "aesthetics.json", "clap.json")
    ]
    for path, value in zip(paths, (manifest, aesthetics, clap)):
        path.write_text(json.dumps(value), encoding="utf-8")
    originals = [path.read_bytes() for path in paths]
    output = generate(*paths, tmp_path / "report" / "listening.html")
    html = output.read_text(encoding="utf-8")
    payload = html.split("const DATA = ", 1)[1].split(";\ndocument.getElementById", 1)[
        0
    ]
    data = json.loads(payload)
    assert len(data["groups"]) == 2
    assert "/* LISTENING_DATA */" not in html
    assert "<script>alert('x')</script>" not in html
    assert [path.read_bytes() for path in paths] == originals
    with pytest.raises(ValueError, match="overwrite"):
        generate(*paths, output)


def test_template_keeps_storage_import_and_reveal_local_with_initially_blank_ratings():
    html = TEMPLATE.read_text(encoding="utf-8")
    assert "revealed = false" in html
    assert 'empty.value = ""' in html
    assert 'setEmpty.value = ""' in html
    assert "set_diversity: diversity" in html
    assert "localStorage.setItem" in html and "file.text()" in html
    assert "pauseOthers(audio)" in html
    assert "createObjectURL(blob)" in html
    assert "innerHTML" not in html
    assert "fetch(" not in html and "XMLHttpRequest" not in html
    assert "<script src=" not in html
