"""Tuning must validate independent objectives without treating training as evidence."""

import pytest
import tune


@pytest.mark.parametrize(
    "reward,identity,accepted",
    [
        (1.1, 0.4, True),
        (1.0, 0.5, True),
        (1.0, 0.4, False),
        (0.9, 0.5, False),
        (1.1, 0.3, False),
    ],
)
def test_acceptance_requires_pareto_improvement(reward, identity, accepted):
    assert (
        tune.dominates(
            {"tunejury_reward": reward, "positive_cosine": identity},
            {"tunejury_reward": 1.0, "positive_cosine": 0.4},
        )
        is accepted
    )


def test_trial_zero_keeps_the_complete_current_preset(monkeypatch):
    base = {name: (low + high) / 2 for name, (low, high) in tune.SPACE.items()}
    base.update(tempo=120.0, swing=50.0)
    monkeypatch.setattr(tune, "current_dials", lambda _: base.copy())

    class Scorer:
        predictor = type("Model", (), {"provenance": {"fixed": "model"}})()
        segments = 3

        def __init__(self):
            self.seeds = []

        def objective(self, preset, dials, seeds):
            assert dials == base
            self.seeds.append(seeds)
            return {key: 1.0 for key in tune.AXES}

    scorer = Scorer()
    result = tune.tune("rock", 1, scorer)
    assert result["best"] == base
    assert not result["validation"]["accepted"]
    assert set(tune.SEARCH_SEEDS).isdisjoint(tune.VALIDATION_SEEDS)
    assert scorer.seeds == [
        tune.SEARCH_SEEDS,
        tune.VALIDATION_SEEDS,
        tune.VALIDATION_SEEDS,
    ]


def test_focused_search_freezes_all_other_dials(monkeypatch):
    base = {name: (low + high) / 2 for name, (low, high) in tune.SPACE.items()}
    base.update(tempo=120.0, swing=58.0)
    monkeypatch.setattr(tune, "current_dials", lambda _: base.copy())

    class Scorer:
        predictor = type("Model", (), {"provenance": {}})()
        segments = 1

        def objective(self, preset, dials, seeds):
            assert {k: v for k, v in dials.items() if k != "humanize"} == {
                k: v for k, v in base.items() if k != "humanize"
            }
            return {key: 1.0 + dials["humanize"] for key in tune.AXES}

    result = tune.tune("rock", 3, Scorer(), ["humanize"])
    assert result["active_dials"] == ["humanize"]
    assert all(row["dials"]["swing"] == 58 for row in result["history"])
