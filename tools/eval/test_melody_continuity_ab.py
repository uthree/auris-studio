import copy
import json
from pathlib import Path

import melody_continuity_ab as ab
import numpy as np
import pytest


def project(seed=102, pitch=60):
    return {
        "format_version": 1,
        "sample_rate": 48000,
        "tempo_map": {},
        "harmony": {},
        "tracks": [
            {
                "id": 1,
                "name": "lead",
                "mixer": {"gain_db": -4},
                "kind": {
                    "type": "instrument",
                    "clips": [
                        {
                            "id": 2,
                            "name": "chorus",
                            "start": 0,
                            "length": 3840,
                            "transforms": [{"kind": "lean", "ticks": 3}],
                            "notes": [
                                {
                                    "pitch": pitch,
                                    "start": 0,
                                    "length": 960,
                                    "velocity": 0.7,
                                }
                            ],
                            "recipe": {
                                "preset": "lead",
                                "seed": seed,
                                "text_digest": pitch,
                            },
                        }
                    ],
                },
            },
            {
                "id": 3,
                "name": "chords",
                "kind": {
                    "type": "instrument",
                    "clips": [
                        {
                            "id": 4,
                            "name": "chorus",
                            "start": 0,
                            "length": 3840,
                            "notes": [
                                {
                                    "pitch": 48,
                                    "start": 0,
                                    "length": 3840,
                                    "velocity": 0.7,
                                }
                            ],
                            "recipe": {"preset": "chords"},
                        }
                    ],
                },
            },
        ],
    }


def write(path: Path, value: dict):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


def experiment(tmp_path, monkeypatch):
    assets = tmp_path / "assets"
    font = assets / "SoundFonts" / "MuseScore_General.sf2"
    font.parent.mkdir(parents=True)
    font.write_bytes(b"fixed font")
    old_cli, new_cli, ffmpeg = (
        tmp_path / name for name in ("old.exe", "new.exe", "ffmpeg.exe")
    )
    old_cli.write_bytes(b"baseline renderer")
    new_cli.write_bytes(b"new composer")
    ffmpeg.write_bytes(b"loudness meter")
    source = write(tmp_path / "source" / "song.auris", project())
    wav = tmp_path / "source" / "song.wav"
    ab.sf.write(wav, np.full((48000, 2), 0.1, dtype=np.float32), 48000, subtype="FLOAT")
    reference = write(
        tmp_path / "reference.json",
        {
            "composer_revision": "frozen-revision",
            "cli": ab.artifact(old_cli),
            "soundfont": ab.artifact(font),
            "files": {
                "pop-band-s102": {
                    "project": ab.artifact(source),
                    "wav": ab.artifact(wav),
                }
            },
        },
    )
    renders = []

    def fake_compose(cli, out, preset, seed, env, folder="projects"):
        label = f"{preset}-s{seed}"
        value = project(seed, 67 if cli == new_cli else 60)
        if cli == new_cli:
            value["tracks"][0]["mixer"] = {"gain_db": 20}
            value["tracks"][0]["kind"]["clips"][0]["transforms"] = []
            value["tracks"][1]["kind"]["clips"][0]["notes"] = []
        return write(out / folder / label / f"{label}.auris", value)

    def fake_render(cli, score, target, log, env):
        renders.append((cli, score, copy.deepcopy(ab.read(score))))
        ab.sf.write(
            target, np.full((48000, 2), 0.2, dtype=np.float32), 48000, subtype="FLOAT"
        )

    def fake_measure(out, label, score, audio, meter):
        excerpt = out / "excerpts" / f"{label}.wav"
        excerpt.write_bytes(audio.read_bytes())
        return {
            "project": ab.artifact(score),
            "wav": {
                **ab.artifact(audio),
                "frames": 48000,
                "sample_rate": 48000,
                "channels": 2,
            },
            "excerpt": {**ab.artifact(excerpt), "duration_seconds": 1},
            "raw_excerpt": ab.artifact(excerpt),
        }

    monkeypatch.setattr(ab, "compose", fake_compose)
    monkeypatch.setattr(ab, "render", fake_render)
    monkeypatch.setattr(ab, "measure_audio", fake_measure)
    return assets, old_cli, new_cli, ffmpeg, reference, renders, source, wav


def test_prespecified_cohort_keeps_diagnostics_controls_and_all_heldout_cases():
    assert len(ab.CASES) == len({(preset, seed) for preset, seed, _ in ab.CASES}) == 11
    assert ("pop-band", 102, "diagnostic") in ab.CASES
    assert ("pop-band", 105, "reference") in ab.CASES
    assert ("rock", 107, "reference") in ab.CASES
    assert {
        (preset, seed) for preset, seed, cohort in ab.CASES if cohort == "held-out"
    } == {
        (preset, seed)
        for preset in ("rock", "city-pop", "pop-band")
        for seed in (201, 202)
    }


def test_baseline_exact_copy_and_candidate_keep_old_backing_mixer_and_renderer(
    tmp_path, monkeypatch
):
    assets, old, new, ffmpeg, reference, renders, source, wav = experiment(
        tmp_path, monkeypatch
    )
    original_score, original_wav = source.read_bytes(), wav.read_bytes()
    cases = (("pop-band", 102, "diagnostic"), ("rock", 201, "held-out"))
    baseline_path = ab.prepare_baseline(
        tmp_path / "before", old, reference, assets, ffmpeg, cases
    )
    baseline = ab.read(baseline_path)
    assert baseline["complete"]
    assert baseline["composer_revision"] == "frozen-revision"
    assert (
        Path(baseline["files"]["pop-band-s102"]["project"]["path"]).read_bytes()
        == original_score
    )
    assert (
        Path(baseline["files"]["pop-band-s102"]["wav"]["path"]).read_bytes()
        == original_wav
    )
    result_path = ab.prepare_candidate(tmp_path / "after", new, baseline_path, cases)
    result = ab.read(result_path)
    assert result["complete"] and len(result["files"]) == 2
    assert all(cli == old for cli, _, _ in renders)
    for label, row in result["files"].items():
        score = ab.read(Path(row["project"]["path"]))
        before = ab.read(Path(baseline["files"][label]["project"]["path"]))
        assert score["tracks"][0]["mixer"] == before["tracks"][0]["mixer"]
        assert score["tracks"][1] == before["tracks"][1]
        assert score["tracks"][0]["kind"]["clips"][0]["transforms"] == [
            {"kind": "lean", "ticks": 3}
        ]
        assert score["tracks"][0]["kind"]["clips"][0]["notes"][0]["pitch"] == 67
        assert row["source_melody_sha256"] != row["output_melody_sha256"]
    assert (source.read_bytes(), wav.read_bytes()) == (original_score, original_wav)


def test_reference_renderer_change_is_rejected_before_creating_outputs(
    tmp_path, monkeypatch
):
    assets, old, _, ffmpeg, reference, _, _, _ = experiment(tmp_path, monkeypatch)
    old.write_bytes(b"silently rebuilt renderer")
    out = tmp_path / "before"
    with pytest.raises(ValueError, match="CLI"):
        ab.prepare_baseline(out, old, reference, assets, ffmpeg)
    assert not out.exists()


def test_changed_baseline_project_or_incomplete_manifest_is_rejected(
    tmp_path, monkeypatch
):
    assets, old, new, ffmpeg, reference, _, _, _ = experiment(tmp_path, monkeypatch)
    baseline_path = ab.prepare_baseline(
        tmp_path / "before",
        old,
        reference,
        assets,
        ffmpeg,
        (("pop-band", 102, "diagnostic"),),
    )
    baseline = ab.read(baseline_path)
    changed = Path(baseline["files"]["pop-band-s102"]["project"]["path"])
    changed.write_bytes(b"changed baseline")
    with pytest.raises(ValueError, match="Artifact changed"):
        ab.prepare_candidate(
            tmp_path / "after", new, baseline_path, (("pop-band", 102, "diagnostic"),)
        )
    baseline["complete"] = False
    write(baseline_path, baseline)
    with pytest.raises(ValueError, match="complete baseline"):
        ab.prepare_candidate(tmp_path / "incomplete", new, baseline_path)
    assert not (tmp_path / "incomplete").exists()


def test_normalization_is_only_fixed_linear_gain_and_never_mutates_source():
    audio = np.array([[0.25, -0.1], [0.2, 0.3]], dtype=np.float32)
    source = audio.copy()
    output, gain = ab.normalize(audio, {"lufs": -13, "true_peak_dbfs": -2})
    assert gain == -10
    np.testing.assert_array_equal(audio, source)
    np.testing.assert_allclose(output, audio * 10 ** (-10 / 20), rtol=1e-6)


@pytest.mark.parametrize(
    "change", ["dropped-case", "extra-file", "changed-wav", "wrong-frames"]
)
def test_candidate_preflight_rejects_corrupt_complete_baseline(
    tmp_path, monkeypatch, change
):
    assets, old, new, ffmpeg, reference, _, _, _ = experiment(tmp_path, monkeypatch)
    cases = (("pop-band", 102, "diagnostic"),)
    baseline_path = ab.prepare_baseline(
        tmp_path / "before", old, reference, assets, ffmpeg, cases
    )
    baseline = ab.read(baseline_path)
    if change == "dropped-case":
        baseline["cases"] = []
    elif change == "extra-file":
        baseline["files"]["unplanned"] = copy.deepcopy(
            baseline["files"]["pop-band-s102"]
        )
    elif change == "changed-wav":
        Path(baseline["files"]["pop-band-s102"]["wav"]["path"]).write_bytes(b"changed")
    else:
        baseline["files"]["pop-band-s102"]["wav"]["frames"] = 1
    write(baseline_path, baseline)
    with pytest.raises(ValueError):
        ab.prepare_candidate(tmp_path / "after", new, baseline_path, cases)
    assert not (tmp_path / "after").exists()


@pytest.mark.parametrize(
    "measured",
    [{"lufs": -30, "true_peak_dbfs": -2}, {"lufs": float("nan"), "true_peak_dbfs": -2}],
)
def test_impossible_loudness_or_nonfinite_measurement_is_reported(measured):
    with pytest.raises(ValueError):
        ab.normalize(np.ones((10, 2), dtype=np.float32) * 0.1, measured)


def test_existing_output_is_never_reused_or_overwritten(tmp_path):
    output = ab.create_output(tmp_path / "experiment")
    sentinel = output / "keep.txt"
    sentinel.write_text("unchanged")
    with pytest.raises(FileExistsError):
        ab.create_output(output)
    assert sentinel.read_text() == "unchanged"
