# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Freeze and run real panel composition trials, then prepare blinded listening pairs.

Run with uv. A passing transport is recorded separately from unscored musical quality.
The executable must be the auris-gpui binary test target, not the desktop application.
"""

from __future__ import annotations

import argparse
import hashlib
import html
import json
import os
import random
import shutil
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = Path(__file__).with_name("references") / "composition-quality-v1.json"
TEST = "ui::agent_chat::composition_eval::codex_composes_and_revises_the_open_document"
AXES = ["request_fit", "melody", "form", "ensemble", "sound"]


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def write_json(path: Path, value: object) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    temporary.replace(path)


def read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


PROMPT_CONDITION_KEYS = {
    "name",
    "source_commit",
    "expected_prefix",
    "replacement_prefix",
    "skill_context",
}


def prompt_condition(path: Path) -> tuple[dict[str, str], bytes, str]:
    """Validate and freeze an optional before/after prompt condition file."""
    payload = path.read_bytes()
    value = json.loads(payload)
    if not isinstance(value, dict) or set(value) != PROMPT_CONDITION_KEYS:
        raise ValueError(
            "Prompt condition must contain exactly: "
            + ", ".join(sorted(PROMPT_CONDITION_KEYS))
        )
    if not all(isinstance(item, str) for item in value.values()):
        raise ValueError("Prompt condition values must all be strings")
    for key in ("expected_prefix", "replacement_prefix"):
        if not value[key]:
            raise ValueError(f"Prompt condition {key} must be nonempty")
    return value, payload, hashlib.sha256(payload).hexdigest()


def manifest(path: Path) -> dict:
    value = read_json(path)
    if value.get("schema_version") != 1 or value.get("axes") != AXES:
        raise ValueError("Unsupported manifest or evaluation axes")
    rubric = value.get("listening_rubric")
    if rubric is not None:
        if not isinstance(rubric, dict) or set(rubric) - set(AXES):
            raise ValueError("Listening rubric keys must be evaluation axes")
        for axis, questions in rubric.items():
            if (
                not isinstance(questions, list)
                or not questions
                or not all(
                    isinstance(question, str) and question.strip()
                    for question in questions
                )
            ):
                raise ValueError(
                    f"Listening rubric for {axis} needs nonempty string questions"
                )
    cases = value["cases"]
    ids = [case["id"] for case in cases]
    if len(ids) != len(set(ids)):
        raise ValueError("Duplicate case IDs")
    for case in cases:
        if not case["id"] or any(
            c not in "abcdefghijklmnopqrstuvwxyz0123456789_-" for c in case["id"]
        ):
            raise ValueError("Case IDs must be safe directory names")
        if case["split"] not in {"development", "holdout"}:
            raise ValueError("Invalid split")
        if case["fixture"] not in {
            "blank",
            "melody_six_eight",
            "melody_six_eight_eight_bars",
        }:
            raise ValueError("Unknown fixture")
        if not case["prompts"] or not all(
            isinstance(p, str) and p.strip() for p in case["prompts"]
        ):
            raise ValueError("Each case needs natural-language prompts")
    return value


def frozen_source(
    reference: Path, runtime: dict[str, str], snapshot: Path | None = None
) -> dict:
    """Bind a reused executable to previously captured source, never today's checkout."""
    payload = reference.read_bytes()
    prior = json.loads(payload)
    if prior.get("runtime") != runtime:
        raise ValueError("Frozen runtime does not match the source suite")
    if snapshot is not None:
        snapshot.write_bytes(payload)
    return {
        key: prior[key]
        for key in ("source_commit", "source_diff", "untracked_rust_sources")
    } | {"source_reference_sha256": hashlib.sha256(payload).hexdigest()}


def freeze_seed_project(source: Path, destination: Path) -> dict[str, object]:
    """Copy a project document and its project-local Audio directory for a trial suite.

    The original is never opened for writing by the harness.  The deliberately narrow copy
    excludes unrelated siblings; references to external assets in the project JSON remain
    references and must be audited/resolved in the same environment separately.
    """
    if source.is_symlink() or source.parent.is_symlink():
        raise ValueError("--seed-project must not use a symlink")
    source = source.resolve(strict=True)
    if source.suffix.lower() != ".auris" or not source.is_file():
        raise ValueError("--seed-project must name an existing .auris file")
    if source.parent.name != source.stem:
        raise ValueError("--seed-project must be in the project-named folder")
    audio = source.parent / "Audio"
    if audio.exists() and (not audio.is_dir() or audio.is_symlink()):
        raise ValueError("Seed project Audio must be a real directory")
    if audio.exists() and any(path.is_symlink() for path in audio.rglob("*")):
        raise ValueError("Seed project Audio must not contain symlinks")
    destination.mkdir(parents=True)
    frozen = destination / source.stem
    frozen.mkdir()
    frozen_file = frozen / source.name
    shutil.copy2(source, frozen_file)
    if audio.exists():
        shutil.copytree(audio, frozen / "Audio")
    files = {
        str(path.relative_to(destination)).replace("\\", "/"): digest(path)
        for path in sorted(destination.rglob("*"))
        if path.is_file()
    }
    return {
        "source": str(source),
        "source_sha256": digest(source),
        "frozen_root": str(destination.resolve()),
        "files": files,
    }


def run(args) -> None:
    suite = manifest(args.manifest)
    args.repetition_start = getattr(args, "repetition_start", 1)
    condition = None
    condition_payload = None
    condition_sha256 = None
    if args.prompt_condition is not None:
        condition, condition_payload, condition_sha256 = prompt_condition(
            args.prompt_condition.resolve(strict=True)
        )
    if not 1 <= args.repetition_start <= 20 or not 1 <= args.repetitions <= 20:
        raise ValueError("Use repetition start/count in 1..20")
    if args.repetition_start + args.repetitions - 1 > 20:
        raise ValueError("Repetition range must end at 20 or earlier")
    if not 1 <= args.timeout <= 3600:
        raise ValueError("Use 1..20 repetitions and 1..3600 seconds per turn")
    if not 8192 <= args.context_tokens <= 262144:
        raise ValueError("Use 8192..262144 context tokens")
    eligible = {case["id"] for case in suite["cases"] if case["split"] == args.split}
    if args.case and not set(args.case) <= eligible:
        raise ValueError("Requested case is not in the selected split")
    selected_cases = [
        case
        for case in suite["cases"]
        if case["split"] == args.split and (not args.case or case["id"] in args.case)
    ]
    if args.seed_project is not None and any(
        case["fixture"] != "blank" for case in selected_cases
    ):
        raise ValueError("--seed-project currently supports only blank fixtures")
    executable = args.executable.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    frozen = output / "runtime"
    frozen.mkdir()
    binary = frozen / executable.name
    shutil.copy2(executable, binary)
    # ORT and platform runtime DLLs may sit beside a Cargo test target or one level above.
    for folder in [executable.parent.parent, executable.parent]:
        for source in folder.glob("*.dll"):
            shutil.copy2(source, frozen / source.name)
    seed_provenance = None
    frozen_seed = None
    if args.seed_project is not None:
        frozen_seed = output / "seed-project"
        seed_provenance = freeze_seed_project(args.seed_project, frozen_seed)
    provenance = {
        "schema_version": 1,
        "manifest": suite,
        "manifest_sha256": digest(args.manifest),
        "model": args.model,
        "backend": getattr(args, "backend", "codex"),
        "prompt_profile": getattr(args, "prompt_profile", "legacy"),
        "candidate_tools_enabled": args.candidate_tools,
        "context_tokens": args.context_tokens,
        "runner_sha256": digest(Path(__file__)),
        "split": args.split,
        "repetitions": args.repetitions,
        "repetition_start": args.repetition_start,
        "repetition_end": args.repetition_start + args.repetitions - 1,
        "source_commit": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "source_diff": subprocess.check_output(
            ["git", "diff", "HEAD", "--", "crates", "docs"],
            cwd=ROOT,
            text=True,
            encoding="utf-8",
        ),
        "untracked_rust_sources": {
            name: (ROOT / name).read_text(encoding="utf-8")
            for name in subprocess.check_output(
                [
                    "git",
                    "ls-files",
                    "--others",
                    "--exclude-standard",
                    "-z",
                    "--",
                    "crates",
                ],
                cwd=ROOT,
                text=True,
                encoding="utf-8",
            ).split("\0")
            if name.endswith(".rs")
        },
        "selected_cases": [
            case["id"]
            for case in suite["cases"]
            if case["split"] == args.split
            and (not args.case or case["id"] in args.case)
        ],
        "runtime": {p.name: digest(p) for p in sorted(frozen.iterdir()) if p.is_file()},
        "started_unix": time.time(),
        "quality_status": "unscored; transport and structural measurements are not musical preference",
        "trials": [],
    }
    if seed_provenance is not None:
        provenance["seed_project"] = seed_provenance
        (output / "input-projects").mkdir()
    if condition is not None:
        provenance["prompt_condition"] = condition
        provenance["prompt_condition_sha256"] = condition_sha256
        (output / "prompt-condition.json").write_bytes(condition_payload)
    if getattr(args, "source_suite", None):
        provenance.update(
            frozen_source(
                args.source_suite, provenance["runtime"], output / "source-suite.json"
            )
        )
    write_json(output / "suite.json", provenance)
    for case in suite["cases"]:
        if case["split"] != args.split or (args.case and case["id"] not in args.case):
            continue
        for repetition in range(
            args.repetition_start, args.repetition_start + args.repetitions
        ):
            trial_id = f"{case['id']}-{repetition:02}"
            destination = output / trial_id
            config = {
                "model": args.model,
                "fixture": case["fixture"],
                "timeout_seconds": args.timeout,
                "prompts": [
                    suite["conditions"] + "\n" + case["prompts"][0],
                    *case["prompts"][1:],
                    suite["delivery_prompt"],
                ],
                "output": str(destination),
            }
            if frozen_seed is not None:
                input_root = output / "input-projects" / trial_id
                shutil.copytree(frozen_seed, input_root)
                config["seed_project"] = str(
                    (
                        input_root / args.seed_project.stem / args.seed_project.name
                    ).resolve()
                )
            if condition is not None:
                config["prompt_condition"] = {
                    key: condition[key]
                    for key in (
                        "expected_prefix",
                        "replacement_prefix",
                        "skill_context",
                    )
                }
            config_path = output / f"{trial_id}.config.json"
            if provenance["backend"] != "codex":
                config["backend"] = provenance["backend"]
                config["context_tokens"] = args.context_tokens
            write_json(config_path, config)
            environment = dict(
                os.environ,
                AURIS_COMPOSITION_EVAL=str(config_path),
                AURIS_COMPOSITION_WORKFLOWS="1"
                if provenance["prompt_profile"] == "modular"
                else "0",
                AURIS_ARRANGEMENT_HARNESS="0",
                AURIS_COMPOSITION_CANDIDATES="1" if args.candidate_tools else "0",
            )
            started = time.monotonic()
            with (output / f"{trial_id}.log").open("w", encoding="utf-8") as log:
                process = subprocess.run(
                    [
                        str(binary),
                        "--ignored",
                        "--exact",
                        TEST,
                        "--nocapture",
                        "--test-threads=1",
                    ],
                    env=environment,
                    cwd=ROOT,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    check=False,
                )
            report_path = destination / "report.json"
            report = read_json(report_path) if report_path.exists() else {}
            artifacts = {
                str(p.relative_to(destination)): digest(p)
                for p in sorted(destination.rglob("*"))
                if p.is_file()
            }
            if frozen_seed is not None:
                frozen_files = {
                    str(path.relative_to(frozen_seed)).replace("\\", "/"): digest(path)
                    for path in sorted(frozen_seed.rglob("*"))
                    if path.is_file()
                }
                if frozen_files != seed_provenance["files"]:
                    raise ValueError("Frozen seed project changed during trial")
            provenance["trials"].append(
                {
                    "id": trial_id,
                    "case": case["id"],
                    "repetition": repetition,
                    "elapsed_seconds": time.monotonic() - started,
                    "exit_code": process.returncode,
                    "transport_pass": process.returncode == 0
                    and len(report.get("turns", [])) == len(config["prompts"])
                    and all(t.get("failure") is None for t in report["turns"]),
                    "config_sha256": digest(config_path),
                    "delivery_present": "result.wav" in artifacts
                    and "result.mid" in artifacts,
                    "encoded": report.get("encoded"),
                    "tool_errors": report.get("argument_or_tool_errors"),
                    "artifacts": artifacts,
                    "ratings": {axis: None for axis in AXES},
                }
            )
            write_json(output / "suite.json", provenance)
            print(
                f"{trial_id}: exit={process.returncode}, {time.monotonic() - started:.1f}s",
                flush=True,
            )


def pairs(before: Path, after: Path, seed: int) -> list[dict]:
    left, right = read_json(before / "suite.json"), read_json(after / "suite.json")
    for field in ["manifest_sha256", "model", "backend", "split", "repetitions"]:
        if left[field] != right[field]:
            raise ValueError(f"Comparison conditions differ: {field}")
    for suite in [left, right]:
        selected = suite.get("pilot_selection", {}).get(
            "cases", suite.get("selected_cases")
        )
        if selected is None:
            selected = [
                case["id"]
                for case in suite["manifest"]["cases"]
                if case["split"] == suite["split"]
            ]
        expected = {
            f"{case}-{repetition:02}"
            for case in selected
            for repetition in range(1, suite["repetitions"] + 1)
        }
        if not expected or expected != {trial["id"] for trial in suite["trials"]}:
            raise ValueError(
                "Incomplete trial cohorts; collect or explicitly record every selected trial"
            )
    lookup = {trial["id"]: trial for trial in right["trials"]}
    if len(lookup) != len(right["trials"]) or len(
        {t["id"] for t in left["trials"]}
    ) != len(left["trials"]):
        raise ValueError("Duplicate trial IDs")
    if set(lookup) != {trial["id"] for trial in left["trials"]}:
        raise ValueError("Trial cohorts differ; do not silently discard missing runs")
    rng = random.Random(seed)
    result = []
    for trial in left["trials"]:
        other = lookup[trial["id"]]
        if not all(
            t["transport_pass"] and t["delivery_present"] for t in [trial, other]
        ):
            result.append(
                {"trial": trial["id"], "status": "failed_or_missing_delivery"}
            )
            continue
        sources = [(before, trial), (after, other)]
        rng.shuffle(sources)
        audio = []
        for root, record in sources:
            path = root / record["id"] / "result.wav"
            if digest(path) != record["artifacts"]["result.wav"]:
                raise ValueError(f"Changed listening artifact: {path}")
            audio.append(
                {
                    "path": str(path.resolve()),
                    "sha256": digest(path),
                    "condition": "before" if root == before else "after",
                }
            )
        result.append({"trial": trial["id"], "status": "ready", "audio": audio})
    rng.shuffle(result)
    return result


def _condition_fingerprint(suite: dict) -> dict:
    """Return fields that must remain identical when repetitions are combined."""
    return {
        key: suite.get(key)
        for key in (
            "manifest_sha256",
            "manifest",
            "model",
            "backend",
            "prompt_profile",
            "candidate_tools_enabled",
            "context_tokens",
            "source_commit",
            "source_diff",
            "untracked_rust_sources",
            "runtime",
            "prompt_condition_sha256",
            "prompt_condition",
            "selected_cases",
            "split",
        )
    }


def _is_link(path: Path) -> bool:
    return path.is_symlink() or getattr(path, "is_junction", lambda: False)()


def _copy_metadata(
    root: Path, output: Path, name: str, expected: str | None = None
) -> str | None:
    source = root / name
    if not source.exists():
        return None
    if _is_link(source) or source.resolve().parent != root or not source.is_file():
        raise ValueError(f"Unsafe metadata path: {source}")
    sha256 = digest(source)
    if expected is not None and sha256 != expected:
        raise ValueError(f"Changed metadata: {source}")
    target = output / name
    if not target.exists():
        shutil.copy2(source, target)
    if digest(target) != sha256:
        raise ValueError(f"Conflicting or changed metadata: {name}")
    return sha256


def combine_repetitions(
    inputs: list[Path], output: Path, runner_change_reason: str | None = None
) -> None:
    """Combine disjoint repetition suites without changing trial identities."""
    if len(inputs) < 2:
        raise ValueError("Provide at least two input suites")
    if output.exists():
        raise FileExistsError(f"Output already exists: {output}")
    if runner_change_reason is not None and not runner_change_reason.strip():
        raise ValueError("Runner change reason must be nonempty")
    suites = []
    for root in inputs:
        root = root.resolve(strict=True)
        data = read_json(root / "suite.json")
        if not isinstance(data.get("trials"), list) or not data["trials"]:
            raise ValueError(f"Suite has no trials: {root}")
        suites.append((root, data))
    required = {
        "manifest_sha256",
        "manifest",
        "model",
        "backend",
        "prompt_profile",
        "candidate_tools_enabled",
        "context_tokens",
        "split",
        "repetitions",
        "selected_cases",
        "runtime",
        "source_commit",
        "source_diff",
        "untracked_rust_sources",
        "runner_sha256",
    }
    for root, data in suites:
        missing = required - data.keys()
        if missing:
            raise ValueError(f"Missing provenance fields in {root}: {sorted(missing)}")
        if data.get("seed_project") is not None:
            raise ValueError("Seeded suites cannot be combined")
        if len(data["selected_cases"]) != len(set(data["selected_cases"])):
            raise ValueError("Duplicate selected cases")
        if "prompt_condition_sha256" in data and "prompt_condition" not in data:
            raise ValueError("Prompt condition hash has no payload")
    fingerprint = _condition_fingerprint(suites[0][1])
    if any(_condition_fingerprint(data) != fingerprint for _, data in suites[1:]):
        raise ValueError("Repetition suites have different frozen conditions")
    runner_hashes = {data["runner_sha256"] for _, data in suites}
    if len(runner_hashes) > 1 and not runner_change_reason:
        raise ValueError("Runner drift requires --runner-change-reason")
    selected = suites[0][1].get("selected_cases", [])
    if not selected:
        raise ValueError("Suite has no selected cases")
    all_trials: list[tuple[Path, dict]] = []
    seen_ids: set[str] = set()
    reps: set[int] = set()
    for root, data in suites:
        raw_start = data.get("repetition_start", 1)
        raw_count = data["repetitions"]
        if isinstance(raw_start, bool) or not isinstance(raw_start, int):
            raise TypeError(f"Invalid repetition start in {root}")
        if isinstance(raw_count, bool) or not isinstance(raw_count, int):
            raise TypeError(f"Invalid repetition count in {root}")
        start, count = raw_start, raw_count
        end = data.get("repetition_end", start + count - 1)
        if isinstance(end, bool) or not isinstance(end, int):
            raise TypeError(f"Invalid repetition end in {root}")
        if count <= 0 or end != start + count - 1 or start < 1 or end > 20:
            raise ValueError(f"Invalid repetition range in {root}")
        expected_ids = {
            f"{case}-{repetition:02}"
            for case in selected
            for repetition in range(start, end + 1)
        }
        actual_ids = {trial.get("id") for trial in data["trials"]}
        if actual_ids != expected_ids:
            raise ValueError(f"Noncanonical or incomplete trial IDs in {root}")
        for trial in data["trials"]:
            trial_id = trial.get("id", "")
            if any(character in trial_id for character in "/\\:"):
                raise ValueError(f"Unsafe trial path: {trial_id}")
            if trial_id in seen_ids:
                raise ValueError(f"Duplicate trial ID: {trial_id}")
            seen_ids.add(trial_id)
            try:
                case, suffix = trial_id.rsplit("-", 1)
                repetition = int(suffix)
            except (ValueError, AttributeError):
                raise ValueError(f"Invalid trial ID: {trial_id}") from None
            if case not in selected or not start <= repetition <= end:
                raise ValueError(f"Trial is outside selected cohort: {trial_id}")
            if repetition not in reps:
                reps.add(repetition)
            if trial.get("repetition") not in (None, repetition):
                raise ValueError(f"Trial repetition mismatch: {trial_id}")
            trial = dict(trial)
            trial["repetition"] = repetition
            artifacts = trial.get("artifacts", {})
            raw_trial = root / trial_id
            if _is_link(raw_trial):
                raise ValueError(f"Unsafe trial path: {trial_id}")
            source_trial = raw_trial.resolve()
            if root not in source_trial.parents:
                raise ValueError(f"Unsafe trial path: {trial_id}")
            source_audio = source_trial / "result.wav"
            if trial.get("delivery_present") and (
                not source_audio.is_file() or not artifacts.get("result.wav")
            ):
                raise ValueError(f"Missing delivered artifact: {trial_id}")
            for relative, expected_hash in artifacts.items():
                relative_path = Path(relative)
                if (
                    relative_path.is_absolute()
                    or ".." in relative_path.parts
                    or ":" in relative
                ):
                    raise ValueError(f"Unsafe artifact path: {trial_id}/{relative}")
                raw_artifact = source_trial / relative_path
                if _is_link(raw_artifact) or any(
                    _is_link(part) for part in [source_trial, *raw_artifact.parents]
                ):
                    raise ValueError(f"Unsafe artifact path: {trial_id}/{relative}")
                artifact_path = raw_artifact.resolve()
                if (
                    source_trial not in artifact_path.parents
                    or artifact_path.is_symlink()
                ):
                    raise ValueError(f"Unsafe artifact path: {trial_id}/{relative}")
                if (
                    not artifact_path.is_file()
                    or digest(artifact_path) != expected_hash
                ):
                    raise ValueError(
                        f"Changed or missing artifact: {trial_id}/{relative}"
                    )
            all_trials.append((root, trial))
    if reps != set(range(1, max(reps) + 1)):
        raise ValueError("Repetition IDs must form a contiguous range starting at 1")
    expected = len(selected) * len(reps)
    if len(all_trials) != expected:
        raise ValueError("Incomplete repetition cohort")
    output.mkdir(parents=True)
    base = dict(suites[0][1])
    base["repetitions"] = len(reps)
    base["repetition_start"] = 1
    base["repetition_end"] = max(reps)
    base["trials"] = []
    base["combined_from"] = []
    if runner_change_reason:
        base["runner_change_reason"] = runner_change_reason
        base["input_runner_sha256"] = sorted(runner_hashes)
    copied_metadata: dict[str, str] = {}
    for root, data in suites:
        base["combined_from"].append(
            {
                "path": str(root),
                "suite_sha256": digest(root / "suite.json"),
                "runner_sha256": data["runner_sha256"],
            }
        )
        for name, expected in (
            ("prompt-condition.json", data.get("prompt_condition_sha256")),
            ("source-suite.json", data.get("source_reference_sha256")),
        ):
            if copied_hash := _copy_metadata(root, output, name, expected):
                copied_metadata[name] = copied_hash
    for root, trial in sorted(all_trials, key=lambda item: item[1]["id"]):
        destination = output / trial["id"]
        destination.mkdir()
        for relative, expected_hash in trial.get("artifacts", {}).items():
            source = (root / trial["id"] / relative).resolve()
            copied = destination / relative
            copied.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, copied)
            if not copied.is_file() or digest(copied) != expected_hash:
                raise ValueError(f"Copied artifact mismatch: {trial['id']}/{relative}")
        for suffix in (".config.json", ".log"):
            name = f"{trial['id']}{suffix}"
            if copied_hash := _copy_metadata(root, output, name):
                copied_metadata[name] = copied_hash
        base["trials"].append(trial)
    if copied_metadata:
        base["copied_metadata"] = copied_metadata
    write_json(output / "suite.json", base)


def blind(args) -> None:
    comparisons = pairs(args.before, args.after, args.seed)
    args.output.mkdir(parents=True, exist_ok=False)
    suite = read_json(args.before / "suite.json")
    rubric = suite["manifest"].get("listening_rubric", {})
    sections = []
    failed = sum(pair["status"] != "ready" for pair in comparisons)
    for index, pair in enumerate(comparisons, 1):
        if pair["status"] != "ready":
            continue
        pair["blind_id"] = f"pair-{index:03}"
        players = []
        for label, source in zip("AB", pair["audio"], strict=True):
            filename = f"{pair['blind_id']}-{label}.wav"
            shutil.copy2(source["path"], args.output / filename)
            players.append(f'<p>{label} <audio controls src="{filename}"></audio></p>')
        questions = "".join(
            f'<label><span class="axis">{html.escape(axis)}</span>'
            + (
                "<ul>"
                + "".join(
                    f"<li>{html.escape(question)}</li>"
                    for question in rubric.get(axis, [])
                )
                + "</ul>"
                if rubric.get(axis)
                else ""
            )
            + f' <select data-axis="{html.escape(axis, quote=True)}"><option value="">Unrated</option>'
            "<option>A</option><option>B</option><option>tie</option><option>cannot_judge</option></select></label> "
            for axis in AXES
        )
        case = next(
            c
            for c in suite["manifest"]["cases"]
            if pair["trial"].rsplit("-", 1)[0] == c["id"]
        )
        brief = html.escape("\n".join(case["prompts"]))
        sections.append(
            f'<section data-id="{pair["blind_id"]}"><h2>{pair["blind_id"]}</h2><pre>{brief}</pre>{"".join(players)}{questions}<p><textarea placeholder="Evidence, timestamps, unresolved issues"></textarea></p></section>'
        )
    page = (
        """<!doctype html><meta charset="utf-8"><title>Composition comparison</title>
<style>body{font:16px system-ui;max-width:1000px;margin:40px auto}section{border-top:1px solid #aaa;padding:20px 0}pre{white-space:pre-wrap}textarea{width:95%;min-height:60px}audio{vertical-align:middle}</style>
<h1>Composition comparison</h1><p>Independent model runs under matching conditions. Original output levels are retained; assess loudness separately from musical preference. A/B identities are in the separate key file.</p>
<button onclick="save()">Download ratings</button>"""
        + f"<p>{len(comparisons)} total trials; {failed} failed or missing delivery. Failures remain in the separate key and must be included in reporting.</p>"
        + "".join(sections)
        + """<script>
function save(){const ratings=Array.from(document.querySelectorAll('section')).map(s=>({id:s.dataset.id,ratings:Object.fromEntries(Array.from(s.querySelectorAll('select')).map(x=>[x.dataset.axis,x.value||null])),evidence:s.querySelector('textarea').value}));const a=document.createElement('a');a.href=URL.createObjectURL(new Blob([JSON.stringify({schema_version:1,ratings},null,2)],{type:'application/json'}));a.download='ratings.json';a.click();setTimeout(()=>URL.revokeObjectURL(a.href),1000)}
</script>"""
    )
    (args.output / "listening.html").write_text(page, encoding="utf-8")
    write_json(
        args.output / "private-key.json", {"seed": args.seed, "pairs": comparisons}
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    runner = commands.add_parser("run")
    runner.add_argument("--executable", type=Path, required=True)
    runner.add_argument("--output", type=Path, required=True)
    runner.add_argument("--manifest", type=Path, default=MANIFEST)
    runner.add_argument("--model", default="gpt-6.1-sol")
    runner.add_argument(
        "--backend", choices=["codex", "claude", "rig"], default="codex"
    )
    runner.add_argument(
        "--prompt-profile", choices=["legacy", "modular"], default="legacy"
    )
    runner.add_argument(
        "--candidate-tools",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Advertise candidate tools (use --no-candidate-tools for the default app catalog)",
    )
    runner.add_argument(
        "--source-suite",
        type=Path,
        help="Previous suite binding an identical frozen runtime to its original source",
    )
    runner.add_argument(
        "--seed-project",
        type=Path,
        help="Read-only source .auris project to freeze and use as every trial's initial state",
    )
    runner.add_argument(
        "--prompt-condition",
        type=Path,
        help="JSON file freezing the before/after prompt condition and its provenance",
    )
    runner.add_argument(
        "--split", choices=["development", "holdout"], default="development"
    )
    runner.add_argument("--repetitions", type=int, default=3)
    runner.add_argument(
        "--repetition-start",
        type=int,
        default=1,
        help="First repetition ID to run; use this to extend a frozen pilot",
    )
    runner.add_argument("--timeout", type=int, default=600)
    runner.add_argument("--context-tokens", type=int, default=65536)
    runner.add_argument("--case", action="append")
    comparison = commands.add_parser("blind")
    comparison.add_argument("--before", type=Path, required=True)
    comparison.add_argument("--after", type=Path, required=True)
    comparison.add_argument("--output", type=Path, required=True)
    comparison.add_argument("--seed", type=int, default=20261024)
    combine = commands.add_parser(
        "combine-repetitions", help="Combine disjoint frozen repetition suites"
    )
    combine.add_argument(
        "--input", type=Path, action="append", required=True, dest="inputs"
    )
    combine.add_argument("--output", type=Path, required=True)
    combine.add_argument("--runner-change-reason")
    args = parser.parse_args()
    if args.command == "run":
        run(args)
    elif args.command == "blind":
        blind(args)
    else:
        combine_repetitions(args.inputs, args.output, args.runner_change_reason)


if __name__ == "__main__":
    main()
