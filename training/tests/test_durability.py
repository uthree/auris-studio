"""Stable-storage ordering for atomically published artifacts."""

from pathlib import Path

from auris_singer.utils import durability


def test_tree_sync_flushes_payloads_before_directories(tmp_path, monkeypatch):
    root = tmp_path / "generation"
    nested = root / "samples"
    nested.mkdir(parents=True)
    (root / "metadata.jsonl").write_text("{}\n", encoding="utf-8")
    (nested / "00000000.npz").write_bytes(b"sample")
    events: list[tuple[str, Path]] = []

    monkeypatch.setattr(durability, "fsync_file", lambda path: events.append(("file", Path(path))))
    monkeypatch.setattr(
        durability,
        "fsync_directory",
        lambda path: events.append(("directory", Path(path))),
    )

    durability.fsync_tree(root)

    file_events = [path for kind, path in events if kind == "file"]
    assert set(file_events) == {
        root / "metadata.jsonl",
        nested / "00000000.npz",
    }
    first_directory = next(index for index, event in enumerate(events) if event[0] == "directory")
    assert all(kind == "file" for kind, _path in events[:first_directory])
    assert events[-2:] == [("directory", nested), ("directory", root)]
