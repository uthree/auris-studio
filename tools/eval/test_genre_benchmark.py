import json
import shutil
from pathlib import Path

import genre_benchmark as benchmark
import pytest

ROOT = Path(__file__).parent
MANIFEST = ROOT / "references" / "genre-benchmark-v1.json"
RUNTIME = {"model": "song-eval-test", "preprocessing": "fixed"}


def test_manifest_has_two_sixteen_bar_cases_per_genre():
    value = benchmark.validate_manifest(MANIFEST)
    assert len(value["cases"]) == 14
    assert {(case["genre"], case["split"]) for case in value["cases"]} == {
        (genre, split)
        for genre in benchmark.GENRES
        for split in ("development", "holdout")
    }


def test_combined_pilot_and_second_repetition_keep_the_complete_genre_cohort(tmp_path):
    import composition_quality as quality

    original = tmp_path / "original"
    scores = _write_suite(
        original, "development", {axis: 3.0 for axis in benchmark.AXES}
    )
    complete = quality.read_json(original / "suite.json")
    complete.update(
        {
            "manifest": benchmark.validate_manifest(MANIFEST),
            "source_diff": "",
            "untracked_rust_sources": {},
            "runtime": {"agent.exe": "frozen"},
            "runner_sha256": "original-runner",
        }
    )
    quality.write_json(original / "suite.json", complete)
    inputs = []
    for repetition in (1, 2):
        root = tmp_path / f"repetition-{repetition}"
        root.mkdir()
        data = dict(complete)
        data["repetitions"] = 1
        data["trials"] = [
            trial
            for trial in complete["trials"]
            if trial["id"].endswith(f"-{repetition:02}")
        ]
        if repetition == 2:
            data["repetition_start"] = 2
            data["runner_sha256"] = "offset-capable-runner"
        for trial in data["trials"]:
            shutil.copytree(original / trial["id"], root / trial["id"])
        quality.write_json(root / "suite.json", data)
        inputs.append(root)
    with pytest.raises(ValueError, match="Runner drift"):
        quality.combine_repetitions(inputs, tmp_path / "refused")
    aggregate = tmp_path / "aggregate"
    quality.combine_repetitions(
        inputs, aggregate, "Add repetition offsets; frozen agent runtime unchanged"
    )
    result = benchmark.compare(original, aggregate, scores, scores, MANIFEST)
    assert result["acceptance"]["all_pairs_valid"]
    assert len(result["rows"]) == 14
    assert result["overall"]["mean_delta"] == 0


def _write_suite(
    root: Path,
    split: str,
    scores: dict[str, float],
    repetitions: int = 2,
    *,
    success: bool = True,
):
    root.mkdir()
    manifest = benchmark.validate_manifest(MANIFEST)
    cases = [case for case in manifest["cases"] if case["split"] == split]
    trials = []
    rows = []
    for case in cases:
        for repetition in range(1, repetitions + 1):
            trial = f"{case['id']}-{repetition:02}"
            folder = root / trial
            folder.mkdir()
            wav = folder / "result.wav"
            wav.write_bytes(trial.encode())
            midi = folder / "result.mid"
            midi.write_bytes((trial + ".mid").encode())
            trials.append(
                {
                    "id": trial,
                    "case": case["id"],
                    "transport_pass": success,
                    "delivery_present": success,
                    "artifacts": {
                        "result.wav": benchmark.digest(wav),
                        "result.mid": benchmark.digest(midi),
                    },
                    "encoded": {
                        "seconds": 16 * 4 * 60 / case["tempo"],
                        "sample_rate": 48000,
                        "full_scale_samples": 0,
                        "peak_db": -1.0,
                        "true_peak_db": -0.9,
                        "sha256": benchmark.digest(wav),
                    },
                }
            )
            if success:
                rows.append(
                    {
                        "id": trial,
                        "status": "scored",
                        "scores": scores,
                        "sha256": benchmark.digest(wav),
                        "diagnostics": {
                            "seconds": 16 * 4 * 60 / case["tempo"],
                            "sample_rate": 48000,
                            "admissible": True,
                        },
                    }
                )
    (root / "suite.json").write_text(
        json.dumps(
            {
                "manifest_sha256": benchmark.digest(MANIFEST),
                "model": "test",
                "backend": "codex",
                "split": split,
                "repetitions": repetitions,
                "prompt_profile": "legacy",
                "context_tokens": 65536,
                "candidate_tools_enabled": False,
                "runtime": RUNTIME,
                "source_commit": "source",
                "selected_cases": [case["id"] for case in cases],
                "trials": trials,
            }
        ),
        encoding="utf-8",
    )
    report = root / "scores.json"
    report.write_text(json.dumps({"runtime": RUNTIME, "rows": rows}), encoding="utf-8")
    return report


def _cohorts(
    tmp_path: Path,
    before_scores: dict[str, float],
    after_scores: dict[str, float],
    repetitions: int = 2,
):
    before = [tmp_path / "before-development", tmp_path / "before-holdout"]
    after = [tmp_path / "after-development", tmp_path / "after-holdout"]
    before_reports = [
        _write_suite(path, split, before_scores, repetitions)
        for path, split in zip(before, ("development", "holdout"), strict=True)
    ]
    after_reports = [
        _write_suite(path, split, after_scores, repetitions)
        for path, split in zip(after, ("development", "holdout"), strict=True)
    ]
    before_report = tmp_path / "before-scores.json"
    after_report = tmp_path / "after-scores.json"
    before_report.write_text(
        json.dumps(
            {
                "runtime": RUNTIME,
                "rows": [
                    row
                    for report in before_reports
                    for row in json.loads(report.read_text())["rows"]
                ],
            }
        )
    )
    after_report.write_text(
        json.dumps(
            {
                "runtime": RUNTIME,
                "rows": [
                    row
                    for report in after_reports
                    for row in json.loads(report.read_text())["rows"]
                ],
            }
        )
    )
    return before, after, before_report, after_report


def test_compare_requires_both_splits_and_multiple_repetitions(tmp_path):
    base = {axis: 3.0 for axis in benchmark.AXES}
    improved = {axis: 3.1 for axis in benchmark.AXES}
    before, after, before_report, after_report = _cohorts(tmp_path, base, improved)
    result = benchmark.compare(before, after, before_report, after_report, MANIFEST)
    assert result["acceptance"]["accepted"] is True
    assert result["selection_policy_version"] == "overall-mean-priority-v2"
    assert result["overall"]["count"] == 28
    assert result["acceptance"]["improved_genres"] == 7


def test_dev_only_suite_is_measured_but_never_adopted(tmp_path):
    base = {axis: 3.0 for axis in benchmark.AXES}
    report = _write_suite(tmp_path / "development", "development", base)
    result = benchmark.compare(
        [tmp_path / "development"],
        [tmp_path / "development"],
        report,
        report,
        MANIFEST,
    )
    assert result["acceptance"]["accepted"] is False
    assert result["acceptance"]["improved_genres"] == 0
    assert result["supplementary"]["mean_delta"] == 0


def test_one_repetition_can_screen_without_claiming_holdout_success(tmp_path):
    before, after = tmp_path / "before", tmp_path / "after"
    left = _write_suite(
        before, "development", {axis: 3.0 for axis in benchmark.AXES}, repetitions=1
    )
    right = _write_suite(
        after, "development", {axis: 3.1 for axis in benchmark.AXES}, repetitions=1
    )
    result = benchmark.compare(before, after, left, right, MANIFEST)
    assert result["acceptance"]["candidate_eligible"] is True
    assert result["acceptance"]["accepted"] is False
    assert result["supplementary_axes"] == list(benchmark.AXES[:-1])
    assert result["supplementary"]["mean_delta"] == 0.1


def test_input_manifest_keeps_trial_ids_and_lists_missing_audio(tmp_path):
    suite = tmp_path / "suite"
    _write_suite(
        suite, "development", {axis: 3.0 for axis in benchmark.AXES}, repetitions=1
    )
    payload = json.loads((suite / "suite.json").read_text())
    payload["trials"][0]["artifacts"] = {}
    shutil.rmtree(suite / payload["trials"][0]["id"])
    (suite / "suite.json").write_text(json.dumps(payload))
    result = benchmark.scoring_manifest([suite])
    assert len(result["inputs"]) == 6
    assert result["missing_audio"] == [
        {"id": "pop-development-01", "reason": "no_retained_audio"}
    ]
    assert result["inputs"][0]["id"] == "city_pop-development-01"


@pytest.mark.parametrize(
    "identifier", ["../outside", "..\\outside", "C:/outside", ".", ""]
)
def test_input_manifest_rejects_trial_path_escape(tmp_path, identifier):
    suite = tmp_path / "suite"
    _write_suite(
        suite, "development", {axis: 3.0 for axis in benchmark.AXES}, repetitions=1
    )
    suite_path = suite / "suite.json"
    payload = json.loads(suite_path.read_text())
    payload["trials"][0]["id"] = identifier
    payload["trials"][0]["artifacts"]["result.wav"] = "0" * 64
    suite_path.write_text(json.dumps(payload))
    with pytest.raises(ValueError, match="unsafe trial ID/path"):
        benchmark.scoring_manifest([suite])


@pytest.mark.parametrize(
    "entrypoint,target",
    [
        ("inputs", "trial"),
        ("inputs", "result.wav"),
        ("compare", "trial"),
        ("compare", "result.wav"),
        ("compare", "result.mid"),
    ],
)
def test_linked_trial_artifacts_are_rejected(tmp_path, monkeypatch, entrypoint, target):
    suite = tmp_path / "suite"
    scores = _write_suite(
        suite, "development", {axis: 3.0 for axis in benchmark.AXES}, repetitions=1
    )
    trial = suite / "pop-development-01"
    source = trial if target == "trial" else trial / target
    outside = tmp_path / "external"
    source.rename(outside)
    try:
        source.symlink_to(outside, target_is_directory=target == "trial")
    except OSError:
        # Windows can deny link creation. Exercise the filesystem link flag at
        # the same boundary without requiring developer mode or elevation.
        outside.rename(source)
        original = Path.is_symlink
        monkeypatch.setattr(
            Path, "is_symlink", lambda path: path == source or original(path)
        )
    with pytest.raises(ValueError, match="unsafe"):
        if entrypoint == "inputs":
            benchmark.scoring_manifest([suite])
        else:
            benchmark.compare(suite, suite, scores, scores, MANIFEST)


@pytest.mark.parametrize(
    "field,value,expected",
    [
        ("seconds", 1.0, "render_duration_mismatch"),
        ("full_scale_samples", 1, "encoded_clipping"),
        ("true_peak_db", 0.1, "invalid_true_peak_db"),
    ],
)
def test_invalid_render_cannot_be_selected(tmp_path, field, value, expected):
    root = tmp_path / "suite"
    score_path = _write_suite(
        root, "development", {axis: 3.0 for axis in benchmark.AXES}
    )
    trial = json.loads((root / "suite.json").read_text())["trials"][0]
    trial["encoded"][field] = value
    score = benchmark._scores(score_path)[trial["id"]]
    case = benchmark.validate_manifest(MANIFEST)["cases"][0]
    assert expected in benchmark.delivery_issues(trial, score, case)


@pytest.mark.parametrize("value", [float("inf"), -1, 5.01, True])
def test_invalid_scores_cannot_enter_an_average(tmp_path, value):
    base = {axis: 3.0 for axis in benchmark.AXES}
    report = _write_suite(tmp_path / "suite", "development", base)
    payload = json.loads(report.read_text())
    payload["rows"][0]["scores"]["Coherence"] = value
    report.write_text(json.dumps(payload))
    with pytest.raises(ValueError, match="finite"):
        benchmark._scores(report)


def test_failed_row_cannot_be_shadowed_by_a_duplicate_success(tmp_path):
    report = _write_suite(
        tmp_path / "suite", "development", {axis: 3.0 for axis in benchmark.AXES}
    )
    payload = json.loads(report.read_text())
    payload["rows"].insert(0, {"id": payload["rows"][0]["id"], "status": "failed"})
    report.write_text(json.dumps(payload))
    with pytest.raises(ValueError, match="duplicate"):
        benchmark._scores(report)


def test_failed_delivery_without_audio_stays_in_cohort(tmp_path):
    before, after = tmp_path / "before", tmp_path / "after"
    left = _write_suite(before, "development", {axis: 3.0 for axis in benchmark.AXES})
    right = _write_suite(after, "development", {axis: 3.2 for axis in benchmark.AXES})
    payload = json.loads((after / "suite.json").read_text())
    trial = payload["trials"][0]
    trial.update(transport_pass=False, delivery_present=False, artifacts={})
    (after / trial["id"] / "result.wav").unlink()
    (after / "suite.json").write_text(json.dumps(payload))
    scores = json.loads(right.read_text())
    scores["rows"] = [row for row in scores["rows"] if row["id"] != trial["id"]]
    right.write_text(json.dumps(scores))
    result = benchmark.compare(before, after, left, right, MANIFEST)
    assert result["acceptance"]["candidate_eligible"] is False
    assert len(result["rows"]) == 14
    assert sum(row["status"] == "failed_or_missing" for row in result["rows"]) == 1


def test_missing_suite_runtime_metadata_is_rejected(tmp_path):
    report = _write_suite(
        tmp_path / "suite", "development", {axis: 3.0 for axis in benchmark.AXES}
    )
    suite_path = report.parent / "suite.json"
    payload = json.loads(suite_path.read_text())
    payload.pop("runtime")
    suite_path.write_text(json.dumps(payload))
    with pytest.raises(ValueError, match="runtime"):
        benchmark.compare([report.parent], [report.parent], report, report, MANIFEST)


@pytest.mark.parametrize("runtime", [None, {}, []])
def test_empty_or_invalid_agent_runtime_is_rejected(tmp_path, runtime):
    report = _write_suite(
        tmp_path / "suite", "development", {axis: 3.0 for axis in benchmark.AXES}
    )
    suite_path = report.parent / "suite.json"
    payload = json.loads(suite_path.read_text())
    payload["runtime"] = runtime
    suite_path.write_text(json.dumps(payload))
    with pytest.raises(ValueError, match="runtime metadata"):
        benchmark.compare(report.parent, report.parent, report, report, MANIFEST)


def test_treatment_can_change_source_and_binary_but_remains_frozen_within_each_side(
    tmp_path,
):
    before, after, before_report, after_report = _cohorts(
        tmp_path,
        {axis: 3.0 for axis in benchmark.AXES},
        {axis: 3.1 for axis in benchmark.AXES},
    )
    for root in after:
        path = root / "suite.json"
        payload = json.loads(path.read_text())
        payload.update(
            source_commit="candidate-source", runtime={"agent.exe": "candidate-binary"}
        )
        path.write_text(json.dumps(payload))
    result = benchmark.compare(before, after, before_report, after_report, MANIFEST)
    assert result["acceptance"]["accepted"] is True
    assert result["agent_runtimes"]["before"] != result["agent_runtimes"]["after"]


@pytest.mark.parametrize("change", ["delete", "change", "remove_hash"])
def test_successful_delivery_requires_intact_midi(tmp_path, change):
    before, after, before_report, after_report = _cohorts(
        tmp_path,
        {axis: 3.0 for axis in benchmark.AXES},
        {axis: 3.1 for axis in benchmark.AXES},
    )
    payload = json.loads((after[0] / "suite.json").read_text())
    trial = payload["trials"][0]
    midi = after[0] / trial["id"] / "result.mid"
    if change == "delete":
        midi.unlink()
    elif change == "change":
        midi.write_bytes(b"changed")
    else:
        del trial["artifacts"]["result.mid"]
        (after[0] / "suite.json").write_text(json.dumps(payload))
    with pytest.raises(ValueError, match="MIDI artifact|result.mid SHA-256"):
        benchmark.compare(
            [before[0], before[1]],
            [after[0], after[1]],
            before_report,
            after_report,
            MANIFEST,
        )


def test_stale_encoded_wav_measurement_cannot_be_selected(tmp_path):
    before2, after2, before_report2, after_report2 = _cohorts(
        tmp_path,
        {axis: 3.0 for axis in benchmark.AXES},
        {axis: 3.1 for axis in benchmark.AXES},
    )
    payload = json.loads((after2[0] / "suite.json").read_text())
    payload["trials"][0]["encoded"]["sha256"] = "0" * 64
    (after2[0] / "suite.json").write_text(json.dumps(payload))
    result = benchmark.compare(
        [before2[0], before2[1]],
        [after2[0], after2[1]],
        before_report2,
        after_report2,
        MANIFEST,
    )
    row = next(
        item for item in result["rows"] if item["trial"] == payload["trials"][0]["id"]
    )
    assert "encoded_artifact_hash_mismatch" in row["issues"]["after"]
    assert row["status"] == "failed_or_missing"


def test_incomplete_cohort_is_rejected(tmp_path):
    base = {axis: 3.0 for axis in benchmark.AXES}
    suite = tmp_path / "development"
    report = _write_suite(suite, "development", base)
    holdout = tmp_path / "holdout"
    _write_suite(holdout, "holdout", base)
    payload = json.loads((suite / "suite.json").read_text())
    payload["trials"].pop()
    (suite / "suite.json").write_text(json.dumps(payload))
    with pytest.raises(ValueError, match="incomplete"):
        benchmark.compare([suite, holdout], [suite, holdout], report, report, MANIFEST)


def test_runtime_mismatch_is_rejected(tmp_path):
    base = {axis: 3.0 for axis in benchmark.AXES}
    before, after, before_report, after_report = _cohorts(tmp_path, base, base)
    after_report.write_text(
        json.dumps(
            {
                "runtime": {"model": "changed"},
                "rows": json.loads(after_report.read_text())["rows"],
            }
        )
    )
    with pytest.raises(ValueError, match="runtime"):
        benchmark.compare(before, after, before_report, after_report, MANIFEST)


def test_changed_or_missing_sha_is_rejected(tmp_path):
    base = {axis: 3.0 for axis in benchmark.AXES}
    before, after, before_report, after_report = _cohorts(tmp_path, base, base)
    wav = before[0] / "pop-development-01" / "result.wav"
    wav.write_bytes(b"changed")
    with pytest.raises(ValueError, match="missing or changed"):
        benchmark.compare(before, after, before_report, after_report, MANIFEST)


def test_duplicate_and_nan_scores_are_rejected(tmp_path):
    base = {axis: 3.0 for axis in benchmark.AXES}
    before, after, before_report, after_report = _cohorts(tmp_path, base, base)
    payload = json.loads(before_report.read_text())
    payload["rows"][0]["scores"]["Clarity"] = float("nan")
    payload["rows"].append(payload["rows"][0])
    before_report.write_text(json.dumps(payload, allow_nan=True))
    with pytest.raises(ValueError, match="finite"):
        benchmark.compare(before, after, before_report, after_report, MANIFEST)


def test_holdout_decline_prevents_acceptance(tmp_path):
    base = {axis: 3.0 for axis in benchmark.AXES}
    dev_gain = {axis: 3.2 for axis in benchmark.AXES}
    holdout_loss = {axis: 2.8 for axis in benchmark.AXES}
    before, after, before_report, after_report = _cohorts(tmp_path, base, dev_gain)
    payload = json.loads(after_report.read_text())
    payload["rows"] = [
        {**row, "scores": holdout_loss} if "holdout" in row["id"] else row
        for row in payload["rows"]
    ]
    after_report.write_text(json.dumps(payload))
    result = benchmark.compare(before, after, before_report, after_report, MANIFEST)
    assert result["acceptance"]["holdout_mean_delta"] == -0.2
    assert result["acceptance"]["accepted"] is False


def test_genre_regression_is_reported_but_not_a_veto_after_holdout(tmp_path):
    base = {axis: 3.0 for axis in benchmark.AXES}
    before, after, before_report, after_report = _cohorts(
        tmp_path, base, {axis: 3.25 for axis in benchmark.AXES}
    )
    payload = json.loads(after_report.read_text())
    payload["rows"] = [
        {
            **row,
            "scores": (
                {axis: 2.0 for axis in benchmark.AXES}
                if row["id"].startswith("pop-development")
                else row["scores"]
            ),
        }
        for row in payload["rows"]
    ]
    after_report.write_text(json.dumps(payload))
    result = benchmark.compare(before, after, before_report, after_report, MANIFEST)
    assert result["acceptance"]["worst_genre_delta"] < -0.1
    assert result["acceptance"]["improved_genres"] == 6
    assert result["development"]["mean_delta"] >= 0.03
    assert result["holdout"]["mean_delta"] > 0
    assert result["acceptance"]["accepted"] is True


def test_overall_priority_does_not_require_a_majority_of_improved_genres(tmp_path):
    base = {axis: 3.0 for axis in benchmark.AXES}
    before, after, before_report, after_report = _cohorts(tmp_path, base, base)
    payload = json.loads(after_report.read_text())
    for row in payload["rows"]:
        value = 4.0 if row["id"].startswith(("pop-", "city_pop-")) else 2.9
        row["scores"] = {axis: value for axis in benchmark.AXES}
    after_report.write_text(json.dumps(payload))
    result = benchmark.compare(before, after, before_report, after_report, MANIFEST)
    assert result["acceptance"]["improved_genres"] == 2
    assert result["acceptance"]["accepted"] is True


@pytest.mark.parametrize(
    "development_gain,holdout_gain,eligible,accepted",
    [(0.02, 0.5, False, False), (0.04, 0.01, True, False), (0.1, 0.0, True, False)],
)
def test_overall_priority_preserves_independent_mean_gates(
    tmp_path, development_gain, holdout_gain, eligible, accepted
):
    base = {axis: 3.0 for axis in benchmark.AXES}
    before, after, before_report, after_report = _cohorts(tmp_path, base, base)
    payload = json.loads(after_report.read_text())
    for row in payload["rows"]:
        gain = holdout_gain if "holdout" in row["id"] else development_gain
        row["scores"] = {axis: 3.0 + gain for axis in benchmark.AXES}
    after_report.write_text(json.dumps(payload))
    result = benchmark.compare(before, after, before_report, after_report, MANIFEST)
    assert result["acceptance"]["candidate_eligible"] is eligible
    assert result["acceptance"]["accepted"] is accepted
