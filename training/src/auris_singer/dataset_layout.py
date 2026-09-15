"""Versioned publication for preprocessed datasets.

A preprocessing run writes a complete immutable generation before replacing
the small ``CURRENT`` pointer. Readers resolve that pointer once and therefore
never combine feature files or metadata from different runs. Directories
without a pointer retain the original flat layout for backwards compatibility.
"""

from __future__ import annotations

import os
import tempfile
import threading
from contextlib import contextmanager
from pathlib import Path

from auris_singer.utils.durability import fsync_directory, fsync_tree

__all__ = [
    "CURRENT_FILE",
    "GENERATIONS_DIR",
    "publish_dataset_generation",
    "resolve_dataset_root",
]

CURRENT_FILE = "CURRENT"
GENERATIONS_DIR = "generations"
_PUBLICATION_MUTEX = threading.Lock()


def _write_pointer(root: Path, contents: bytes) -> Path:
    """Write and flush a private ``CURRENT`` candidate in ``root``."""
    descriptor, temporary_name = tempfile.mkstemp(prefix=".CURRENT-", suffix=".tmp", dir=root)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(contents)
            stream.flush()
            os.fsync(stream.fileno())
    except BaseException:
        _remove_temporary(temporary)
        raise
    return temporary


def _remove_temporary(path: Path | None) -> None:
    """Remove a private pointer without changing a completed transaction's result."""
    if path is None:
        return
    try:
        path.unlink(missing_ok=True)
    except OSError:
        # The dot-prefixed file is never read. Antivirus software can briefly
        # hold it on Windows; a later preprocessing run may safely ignore it.
        pass


@contextmanager
def _publication_lock(root: Path):
    """Serialize ``CURRENT`` commit and rollback within and between processes."""
    with _PUBLICATION_MUTEX:
        lock_path = root / ".CURRENT.lock"
        with lock_path.open("a+b") as stream:
            if stream.seek(0, os.SEEK_END) == 0:
                stream.write(b"\0")
                stream.flush()
            stream.seek(0)
            if os.name == "nt":
                import msvcrt

                msvcrt.locking(stream.fileno(), msvcrt.LK_LOCK, 1)
                try:
                    yield
                finally:
                    stream.seek(0)
                    msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl

                fcntl.flock(stream.fileno(), fcntl.LOCK_EX)
                try:
                    yield
                finally:
                    fcntl.flock(stream.fileno(), fcntl.LOCK_UN)


def resolve_dataset_root(root: str | Path) -> Path:
    """Return the one immutable generation selected by ``root/CURRENT``.

    A legacy dataset with no pointer resolves to ``root`` itself. The pointer
    may contain only a generated directory name; malformed or dangling values
    fail closed instead of escaping the dataset directory or falling back to
    stale top-level files.
    """
    root = Path(root)
    current = root / CURRENT_FILE
    if not current.exists():
        return root
    generation = current.read_text(encoding="utf-8").strip()
    if (
        not generation
        or generation in {".", ".."}
        or "/" in generation
        or "\\" in generation
        or Path(generation).name != generation
    ):
        raise ValueError(f"invalid dataset generation in {current}: {generation!r}")
    generations = root / GENERATIONS_DIR
    selected = generations / generation
    if not selected.is_dir():
        raise FileNotFoundError(
            f"dataset generation {generation!r} selected by {current} does not exist"
        )
    if selected.resolve().parent != generations.resolve():
        raise ValueError(
            f"dataset generation {generation!r} selected by {current} escapes {generations}"
        )
    return selected


def publish_dataset_generation(root: str | Path, generation: Path) -> Path:
    """Publish a completed staging directory and return its final path.

    ``generation`` must be an immediate child of ``root``. It is first renamed
    into the immutable generations directory, then an fsynced temporary pointer
    is atomically replaced over ``CURRENT``. If pointer publication fails, an
    earlier generation remains selected.
    """
    root = Path(root)
    generation = Path(generation)
    if (
        generation.parent != root
        or not generation.is_dir()
        or generation.is_symlink()
        or generation.resolve().parent != root.resolve()
    ):
        raise ValueError("dataset staging generation must be a directory directly under root")

    # A pointer may select this tree immediately after publication. Flush the
    # complete payload first so it cannot name a generation whose manifest or
    # sample files existed only in the page cache at the time of a crash.
    fsync_tree(generation)

    final_parent = root / GENERATIONS_DIR
    final_parent.mkdir(parents=True, exist_ok=True)
    fsync_directory(root)
    name = generation.name.removeprefix(".staging-")
    if not name or name in {".", ".."} or "/" in name or "\\" in name:
        raise ValueError(f"invalid dataset generation name: {name!r}")
    final = final_parent / name
    generation.replace(final)
    fsync_directory(final_parent)

    with _publication_lock(root):
        current = root / CURRENT_FILE
        previous = current.read_bytes() if current.exists() else None
        temporary: Path | None = None
        rollback_temporary: Path | None = None
        attempted = False
        try:
            temporary = _write_pointer(root, (name + "\n").encode())
            # Mark the destination before the syscall: an injected interruption can
            # arrive after the kernel completed the rename but before Python sees a
            # return value. Restoring the old bytes is harmless if it did not move.
            attempted = True
            os.replace(temporary, current)
            temporary = None
            fsync_directory(root)
        except BaseException:
            if attempted:
                try:
                    if previous is None:
                        current.unlink(missing_ok=True)
                    elif not current.exists() or current.read_bytes() != previous:
                        rollback_temporary = _write_pointer(root, previous)
                        os.replace(rollback_temporary, current)
                        rollback_temporary = None
                    fsync_directory(root)
                except BaseException as rollback_error:
                    raise RuntimeError(
                        "dataset generation publication failed and CURRENT could not be restored"
                    ) from rollback_error
            raise
        finally:
            _remove_temporary(temporary)
            _remove_temporary(rollback_temporary)
    return final
