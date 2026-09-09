"""The export package pins its source and never silently truncates a target."""

import hashlib
import json
from unittest.mock import Mock

import export_clap as export
import pytest


@pytest.mark.parametrize("revision", ["main", "v1", "0" * 39, "A" * 40, "../model"])
def test_revision_must_be_immutable(revision):
    with pytest.raises(ValueError, match="commit hash"):
        export.validate_revision(revision)


@pytest.mark.parametrize("prompt", ["", " \n\t"])
def test_empty_prompt_is_rejected_without_tokenizing(prompt):
    tokenizer = Mock()
    with pytest.raises(ValueError, match="empty"):
        export.token_inputs(tokenizer, prompt)
    tokenizer.assert_not_called()


def test_long_prompt_is_not_silently_truncated():
    tokenizer = Mock(return_value={"input_ids": list(range(78))})
    with pytest.raises(ValueError, match="77-token"):
        export.token_inputs(tokenizer, "A long prompt")
    tokenizer.assert_called_once_with("A long prompt", truncation=False, padding=False)


def test_exact_limit_is_accepted_and_right_padding_requested():
    tokenizer = Mock(side_effect=[{"input_ids": list(range(77))}, {"prepared": True}])
    assert export.token_inputs(tokenizer, "Boundary prompt") == {"prepared": True}
    tokenizer.assert_called_with(
        "Boundary prompt",
        truncation=False,
        padding="max_length",
        max_length=77,
        return_tensors="pt",
    )


def test_manifest_hashes_every_runtime_input(tmp_path):
    for name in export.FILES:
        (tmp_path / name).write_bytes(name.encode())
    manifest = export.write_manifest(tmp_path, export.REVISION)
    assert manifest["format"] == "auris-clap-htsat-unfused-v1"
    assert manifest["source_revision"] == export.REVISION
    assert manifest["model_id"] == "laion/clap-htsat-unfused"
    assert manifest["license"] == "Apache-2.0"
    assert manifest["files"] == {
        name: hashlib.sha256(name.encode()).hexdigest() for name in export.FILES
    }
    assert json.loads((tmp_path / "manifest.json").read_text()) == manifest
    with pytest.raises(FileExistsError):
        export.write_manifest(tmp_path, export.REVISION)


def test_incomplete_package_is_not_published(tmp_path):
    (tmp_path / "audio.onnx").write_bytes(b"incomplete")
    with pytest.raises(FileNotFoundError):
        export.write_manifest(tmp_path, export.REVISION)
    assert not (tmp_path / "manifest.json").exists()
