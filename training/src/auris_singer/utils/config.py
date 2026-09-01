"""Configuration loading built on OmegaConf."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from omegaconf import DictConfig, OmegaConf

__all__ = ["load_config", "save_config"]


def load_config(
    path: str | Path, overrides: list[str] | None = None
) -> DictConfig:
    """Load a YAML config, resolving an optional ``defaults`` include list.

    A config may start with::

        defaults:
          - presets.yml@model: base

    which merges the ``base`` entry of ``presets.yml`` under the ``model`` key
    before the rest of the file is applied.  Paths are resolved relative to the
    including file.

    Args:
        path: YAML file to load.
        overrides: dotlist overrides, e.g. ``["trainer.max_steps=1000"]``.
    """
    path = Path(path)
    raw = OmegaConf.load(path)
    if not isinstance(raw, DictConfig):
        raise TypeError(f"{path} must contain a mapping at the top level")

    defaults = raw.pop("defaults", None)
    merged = OmegaConf.create({})
    if defaults is not None:
        for entry in defaults:
            merged = OmegaConf.merge(merged, _load_default(path.parent, entry))
    merged = OmegaConf.merge(merged, raw)

    if overrides:
        merged = OmegaConf.merge(merged, OmegaConf.from_dotlist(list(overrides)))
    return merged  # type: ignore[return-value]


def _load_default(base_dir: Path, entry: Any) -> DictConfig:
    """Resolve one ``defaults`` entry into a config fragment."""
    if not isinstance(entry, DictConfig) and not isinstance(entry, dict):
        raise TypeError(
            f"each 'defaults' entry must be a mapping like "
            f"'presets.yml@model: base', got {entry!r}"
        )
    fragment = OmegaConf.create({})
    for spec, key in dict(entry).items():
        file_part, _, target = str(spec).partition("@")
        source = OmegaConf.load(base_dir / file_part)
        if key is not None:
            if key not in source:
                raise KeyError(f"preset {key!r} not found in {file_part}")
            source = source[key]
        fragment = OmegaConf.merge(
            fragment, OmegaConf.create({target: source}) if target else source
        )
    return fragment  # type: ignore[return-value]


def save_config(config: DictConfig, path: str | Path) -> None:
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(OmegaConf.to_yaml(config, resolve=True), encoding="utf-8")
