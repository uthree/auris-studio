"""Tests for the source-control check script's condition table."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "check_source_control.py"
_spec = importlib.util.spec_from_file_location("check_source_control", SCRIPT)
check = importlib.util.module_from_spec(_spec)
sys.modules["check_source_control"] = check
_spec.loader.exec_module(check)


def test_reference_condition_comes_first_and_is_unmodified():
    conditions = check.build_conditions([3.0], [2.0])
    assert conditions[0] == ("reference", 1.0, 1.0)


def test_semitone_shifts_become_frequency_ratios():
    conditions = dict((name, ratio) for name, ratio, _ in check.build_conditions([12.0, -12.0], []))
    assert conditions["pitch_up_12st"] == pytest.approx(2.0)
    assert conditions["pitch_down_12st"] == pytest.approx(0.5)


def test_energy_scales_do_not_touch_pitch():
    for name, pitch_ratio, energy_scale in check.build_conditions([], [0.5, 2.0]):
        if name == "reference":
            continue
        assert pitch_ratio == 1.0
        assert energy_scale in (0.5, 2.0)


def test_no_op_modifications_are_dropped():
    """A 0-semitone shift or a 1.0 scale would just repeat the reference."""
    conditions = check.build_conditions([0.0], [1.0])
    assert [name for name, _, _ in conditions] == ["reference"]


def test_condition_names_are_unique():
    conditions = check.build_conditions([-5.0, -2.0, 3.0, 7.0], [0.5, 2.0])
    names = [name for name, _, _ in conditions]
    assert len(set(names)) == len(names)
