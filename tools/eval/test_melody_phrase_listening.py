import json
from pathlib import Path

import melody_phrase_ab as ab
import melody_phrase_listening as listening
import pytest
from test_melody_phrase_ab import CASES, fixture, write


def corpus(tmp_path, monkeypatch):
    source, spec, _, _, _ = fixture(tmp_path, monkeypatch)
    return ab.prepare_experiment(source, spec, tmp_path / "comparison", CASES)


def score_files(manifest_path):
    manifest = ab.read(manifest_path)
    spec = {"schema_version": 1, "variants": {}}
    for name, value in manifest["variants"].items():
        condition = ab.read(ab.verified(value))
        folder = Path(value["path"]).parent
        aesthetics = {label: {"CE": 7, "PQ": 8} for label in condition["files"]}
        clap = {
            "prompts_sha256": "same prompts",
            "prompts": {"one": "fixed"},
            "model": {"sha256": "same model"},
            "preprocessing": {"requested_segments": 1},
            "files": {
                label: {
                    "sha256": row["excerpt"]["sha256"],
                    "aggregate": {"positive_cosine": 0.3},
                }
                for label, row in condition["files"].items()
            },
        }
        a_path, c_path = (
            write(folder / "aesthetics.json", aesthetics),
            write(folder / "clap.json", clap),
        )
        spec["variants"][name] = {
            "aesthetics": ab.artifact(a_path),
            "clap": ab.artifact(c_path),
            "excerpt_sha256": {
                label: row["excerpt"]["sha256"]
                for label, row in condition["files"].items()
            },
        }
    return write(manifest_path.parent / "scores.json", spec)


def test_local_report_has_all_conditions_and_missing_scores_remain_blank(
    tmp_path, monkeypatch
):
    manifest = corpus(tmp_path, monkeypatch)
    output = manifest.parent / "listening.html"
    data = listening.listening_data(manifest, output)
    assert len(data["groups"]) == 2
    for group in data["groups"]:
        for case in group["cases"]:
            assert set(case["conditions"]) == set(listening.CONDITIONS)
            for row in case["conditions"].values():
                assert row["scores"] == {"CE": None, "PQ": None, "CLAP": None}
                assert not row["excerpt"].startswith(("http", "file:", "/"))
                assert row["project"].endswith(".auris")
    listening.generate(manifest, output)
    text = output.read_text(encoding="utf-8")
    assert "/* DATA */" not in text
    assert "metrics.hidden=true" in text
    assert "pauseOthers(audio)" in text
    assert "fetch(" not in text and "localStorage" not in text
    with pytest.raises(ValueError, match="overwrite"):
        listening.generate(manifest, output)


def test_model_scores_must_match_exact_audio_and_frozen_conditions(
    tmp_path, monkeypatch
):
    manifest = corpus(tmp_path, monkeypatch)
    scores = score_files(manifest)
    data = listening.listening_data(manifest, manifest.parent / "index.html", scores)
    assert data["groups"][0]["cases"][0]["conditions"]["pitch"]["scores"] == {
        "CE": 7,
        "PQ": 8,
        "CLAP": 0.3,
    }
    spec = ab.read(scores)
    spec["variants"]["pitch"]["excerpt_sha256"]["rock-s201"] = "changed"
    write(scores, spec)
    with pytest.raises(ValueError, match="Scored audio"):
        listening.generate(manifest, manifest.parent / "wrong.html", scores)
    assert not (manifest.parent / "wrong.html").exists()


@pytest.mark.parametrize(
    "change", ["prompts", "segments", "audio", "missing-label", "nan"]
)
def test_inconsistent_model_measurements_are_rejected(tmp_path, monkeypatch, change):
    manifest = corpus(tmp_path, monkeypatch)
    scores = score_files(manifest)
    spec = ab.read(scores)
    entry = spec["variants"]["combined"]
    path = Path(entry["clap"]["path"])
    clap = ab.read(path)
    if change == "prompts":
        clap["prompts_sha256"] = "new prompts"
    elif change == "segments":
        clap["preprocessing"]["requested_segments"] = 3
    elif change == "audio":
        clap["files"]["rock-s201"]["sha256"] = "different audio"
    elif change == "missing-label":
        clap["files"].pop("rock-s201")
    else:
        clap["files"]["rock-s201"]["aggregate"]["positive_cosine"] = float("nan")
    write(path, clap)
    entry["clap"] = ab.artifact(path)
    write(scores, spec)
    with pytest.raises(ValueError):
        listening.generate(manifest, manifest.parent / "wrong.html", scores)


def test_json_embedding_is_script_safe_without_altering_text():
    value = {"label": "</script><img src=x onerror=alert(1)>\u2028&"}
    escaped = listening.script_json(value)
    assert "</script>" not in escaped and "<img" not in escaped
    assert json.loads(escaped) == value
