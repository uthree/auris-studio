import copy
import json
from pathlib import Path
from types import SimpleNamespace

import pytest
from melody_ab import replace_melody, write_comparison


def clip(name: str, role: str | None, *, start: int = 0) -> dict:
    result = {
        "id": start + 10,
        "name": name,
        "start": start,
        "length": 3840,
        "notes": [{"pitch": 60, "start": 0, "length": 960, "velocity": 0.8}],
        "transforms": [{"kind": "lean", "ticks": -2}],
    }
    if role:
        result["recipe"] = {"preset": role, "seed": 101, "text_digest": 10}
    return result


def project() -> dict:
    return {
        "format_version": 1,
        "sample_rate": 48000,
        "tempo_map": {"points": [{"tick": 0, "bpm": 120}]},
        "signatures": {"points": [{"tick": 0, "numerator": 4, "denominator": 4}]},
        "harmony": {"chords": [{"tick": 0, "chord": "I"}]},
        "sections": [{"start": 0, "length": 7680}],
        "tracks": [
            {
                "id": 1,
                "name": "lead",
                "mixer": {"gain_db": -4},
                "kind": {
                    "type": "instrument",
                    "instrument_id": "test",
                    "clips": [clip("verse", "lead"), clip("ending", None, start=3840)],
                },
            },
            {
                "id": 2,
                "name": "keys",
                "mixer": {"pan": 0.4},
                "kind": {"type": "instrument", "clips": [clip("verse", "chords")]},
            },
        ],
        "soundfonts": {},
    }


def candidate(source: dict) -> dict:
    result = copy.deepcopy(source)
    result["tracks"][0]["id"] = 91
    result["tracks"][0]["mixer"]["gain_db"] = -10
    for i, item in enumerate(result["tracks"][0]["kind"]["clips"]):
        item["id"] += 50
        item["transforms"] = []
        item["notes"][0]["pitch"] += 7 + i
        item["notes"].append(
            {"pitch": 64, "start": 1200, "length": 480, "velocity": 0.65, "bend": [0.2]}
        )
        if "recipe" in item:
            item["recipe"]["text_digest"] = 20
    result["tracks"][1]["kind"]["clips"][0]["notes"] = []
    return result


def test_replaces_complete_lead_text_and_digest_while_preserving_every_other_field():
    source = project()
    new = candidate(source)
    before_source, before_new = copy.deepcopy(source), copy.deepcopy(new)
    output, report = replace_melody(source, new)
    expected = copy.deepcopy(source)
    for old_clip, new_clip in zip(
        expected["tracks"][0]["kind"]["clips"], new["tracks"][0]["kind"]["clips"]
    ):
        old_clip["notes"] = new_clip["notes"]
        if "recipe" in old_clip:
            old_clip["recipe"]["text_digest"] = 20
    assert output == expected
    assert source == before_source and new == before_new
    assert report["preserved_source_sha256"] == report["preserved_output_sha256"]
    assert report["source_melody_sha256"] != report["output_melody_sha256"]
    assert len(report["clips"]) == 2
    assert sum(c["candidate_note_count"] for c in report["clips"]) == 4


@pytest.mark.parametrize(
    "field",
    ["harmony", "signatures", "tempo_map", "sections", "sample_rate", "loop_region"],
)
def test_rejects_context_mismatch(field):
    source, new = project(), project()
    new[field] = "different"
    with pytest.raises(ValueError, match=field):
        replace_melody(source, new)


@pytest.mark.parametrize("change", ["layout", "knobs", "bounds", "empty", "roles"])
def test_rejects_misaligned_or_invalid_candidate(change):
    source = project()
    new = candidate(source)
    item = new["tracks"][0]["kind"]["clips"][0]
    if change == "layout":
        item["start"] += 1
    elif change == "knobs":
        item["recipe"]["seed"] += 1
    elif change == "bounds":
        item["notes"][0]["length"] = 5000
    elif change == "empty":
        item["notes"] = []
    else:
        item["recipe"]["preset"] = "chords"
    with pytest.raises(ValueError):
        replace_melody(source, new)


def test_matches_reordered_tracks_and_clips_without_using_ids():
    source = project()
    new = candidate(source)
    new["tracks"][0]["kind"]["clips"].reverse()
    new["tracks"].reverse()
    output, _ = replace_melody(source, new)
    assert [t["id"] for t in output["tracks"]] == [1, 2]
    assert [c["notes"][0]["pitch"] for c in output["tracks"][0]["kind"]["clips"]] == [
        67,
        68,
    ]


def test_duplicate_track_or_clip_match_is_rejected():
    for duplicate_track in (True, False):
        source = project()
        collection = (
            source["tracks"]
            if duplicate_track
            else source["tracks"][0]["kind"]["clips"]
        )
        collection.append(copy.deepcopy(collection[0]))
        with pytest.raises(ValueError, match="Ambiguous"):
            replace_melody(source, source)


def test_singers_and_explicit_chord_clips_on_mixed_track_are_preserved():
    source = project()
    source["tracks"][0]["kind"]["clips"][1]["recipe"] = {"preset": "chords"}
    singer = copy.deepcopy(source["tracks"][0])
    singer["name"], singer["kind"]["type"] = "Melody", "singer"
    source["tracks"].append(singer)
    new = copy.deepcopy(source)
    for track in new["tracks"]:
        for item in track["kind"]["clips"]:
            item["notes"][0]["pitch"] = 72
    output, report = replace_melody(source, new)
    assert len(report["clips"]) == 1
    assert (
        output["tracks"][0]["kind"]["clips"][1]
        == source["tracks"][0]["kind"]["clips"][1]
    )
    assert output["tracks"][2] == source["tracks"][2]


def write_inputs(tmp_path):
    source, new = tmp_path / "source.auris", tmp_path / "candidate.auris"
    source.write_text(json.dumps(project()), encoding="utf-8")
    new.write_text(json.dumps(candidate(project())), encoding="utf-8")
    return source, new, tmp_path / "output" / "output.auris"


def test_writes_fresh_editable_project_and_hash_manifest_without_overwriting(tmp_path):
    source, new, output = write_inputs(tmp_path)
    original_bytes = source.read_bytes(), new.read_bytes()
    manifest = write_comparison(source, new, output)
    result = json.loads(output.read_text(encoding="utf-8"))
    assert result["tracks"][0]["kind"]["clips"][0]["recipe"]["text_digest"] == 20
    assert len(json.loads(manifest.read_text(encoding="utf-8"))["inputs"]) == 2
    for destination in (source, new, output):
        with pytest.raises(ValueError, match="overwrite"):
            write_comparison(source, new, destination)
    assert original_bytes == (source.read_bytes(), new.read_bytes())


def test_rejects_relative_assets_before_writing_any_output(tmp_path):
    source, new, output = write_inputs(tmp_path)
    value = json.loads(source.read_text(encoding="utf-8"))
    value["soundfonts"] = {"1": {"path": {"inside": "Audio/font.sf2"}}}
    source.write_text(json.dumps(value), encoding="utf-8")
    with pytest.raises(ValueError, match="Relative assets"):
        write_comparison(source, new, output)
    assert not output.exists()


def test_optional_render_uses_source_mixer_and_records_failure(tmp_path, monkeypatch):
    source, new, output = write_inputs(tmp_path)
    (tmp_path / "auris").write_bytes(b"test renderer")
    seen = []

    def render(command, **kwargs):
        seen.append(command)
        value = json.loads(Path(command[2]).read_text(encoding="utf-8"))
        assert value["tracks"][0]["mixer"]["gain_db"] == -4
        return SimpleNamespace(returncode=1, stdout="", stderr="renderer failed")

    monkeypatch.setattr("melody_ab.subprocess.run", render)
    with pytest.raises(RuntimeError, match="CLI render failed"):
        write_comparison(source, new, output, cli=tmp_path / "auris")
    assert seen[0][3:6] == ["--bit-depth", "32", "--no-tail"]
    manifest = json.loads(
        output.with_suffix(".melody-ab.json").read_text(encoding="utf-8")
    )
    assert manifest["render"]["returncode"] == 1
    assert source.exists() and new.exists()


def test_wav_requires_cli_even_when_called_as_a_library(tmp_path):
    source, new, output = write_inputs(tmp_path)
    with pytest.raises(ValueError, match="requires --cli"):
        write_comparison(source, new, output, wav=tmp_path / "file.wav")
    assert not output.exists()
