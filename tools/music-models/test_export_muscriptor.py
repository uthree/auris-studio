"""Conversion is explicit, local, Small-only and preserves noncommercial acknowledgement."""

import json
from pathlib import Path

import export_muscriptor as export
import pytest


def test_no_consent_refuses_before_any_file_is_opened():
    with pytest.raises(ValueError, match="CC BY-NC 4.0"):
        export.check_input(Path("absent.safetensors"), False)


def test_only_original_small_config_is_accepted(tmp_path):
    checkpoint = tmp_path / "model.safetensors"
    checkpoint.write_bytes(b"test fixture, never loaded")
    config = tmp_path / "config.json"
    config.write_text('{"variant":"large"}')
    with pytest.raises(ValueError, match="original MuScriptor Small"):
        export.check_input(checkpoint, True)
    config.write_text(json.dumps(export.CONFIG))
    assert export.check_input(checkpoint, True) == checkpoint.resolve()


def test_output_directory_is_never_replaced(tmp_path, monkeypatch):
    checkpoint = tmp_path / "model.safetensors"
    checkpoint.write_bytes(b"unused")
    (tmp_path / "config.json").write_text(json.dumps(export.CONFIG))
    output = tmp_path / "output"
    output.mkdir()
    sentinel = output / "keep.txt"
    sentinel.write_text("user data")
    monkeypatch.setattr(
        "sys.argv",
        [
            "export",
            "--checkpoint",
            str(checkpoint),
            "--output-directory",
            str(output),
            "--acknowledge-noncommercial",
        ],
    )
    with pytest.raises(ValueError, match="already exists"):
        export.main()
    assert sentinel.read_text() == "user data"
