"""Safe output keys derived from corpus source paths."""

from __future__ import annotations

from pathlib import Path

__all__ = ["clip_output_key", "create_fresh_output"]


def create_fresh_output(path: str | Path) -> Path:
    """Atomically reserve a new preparation output directory.

    Preparation never merges into an existing tree: a changed split or a
    removed source would otherwise leave stale clips that preprocessing treats
    as current training data. A partial failed run is intentionally preserved
    for inspection and must be removed explicitly or replaced by a new path.
    """
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        path.mkdir()
    except FileExistsError as error:
        raise FileExistsError(
            f"output already exists: {path}; choose a new --output or remove the old "
            "generated directory explicitly"
        ) from error
    return path


def clip_output_key(
    source: str | Path,
    source_root: str | Path,
    clip_index: int,
    *,
    width: int = 3,
) -> Path:
    """Map a source-relative path and clip index to an injective output key.

    Keeping the relative parent directories is important: flattening to
    ``source.stem`` overwrites common names such as ``song.wav`` from separate
    corpus folders. Mirrored wav/text/duration roots can all use this key.
    """
    if clip_index < 0:
        raise ValueError("clip_index must be non-negative")
    source = Path(source).resolve()
    root = Path(source_root).resolve()
    try:
        relative = source.relative_to(root).with_suffix("")
    except ValueError as error:
        raise ValueError(f"{source} is outside source root {root}") from error
    if not relative.name:
        raise ValueError(f"source path has no file name: {source}")
    return relative.parent / f"{relative.name}_{clip_index:0{width}d}"
