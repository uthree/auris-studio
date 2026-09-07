"""The optional worker must refuse missing consent before touching models or packages."""

import runpy
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

WORKER = Path(__file__).resolve().parents[2] / "crates/auris-session/src/session/muscriptor_worker.py"


def test_no_consent_is_rejected_before_loading_optional_dependencies(capsys):
    argv = [str(WORKER), "--model", "missing.safetensors", "--directory", "missing"]
    with patch.object(sys, "argv", argv), pytest.raises(SystemExit) as error:
        runpy.run_path(str(WORKER), run_name="__main__")
    assert error.value.code == 2
    assert "CC BY-NC 4.0" in capsys.readouterr().err


def test_other_model_config_is_refused_before_importing_runtime(tmp_path, capsys):
    model = tmp_path / "model.safetensors"
    model.write_bytes(b"unused")
    (tmp_path / "config.json").write_text('{"variant": "large"}')
    argv = [str(WORKER), "--acknowledge-noncommercial", "--model", str(model),
            "--directory", str(tmp_path)]
    with patch.object(sys, "argv", argv), pytest.raises(SystemExit) as error:
        runpy.run_path(str(WORKER), run_name="__main__")
    assert error.value.code == 2
    assert "original MuScriptor Small" in capsys.readouterr().err
