"""Client for the `physical_fit_render` worker (development only).

The worker is `crates/auris-synth/examples/physical_fit_render.rs`: JSON lines in,
length-prefixed float32 mono PCM out. It plays the actual instrument, or passes recorded
PCM through the actual effects, and resets both between notes.
"""

import json
import subprocess

import numpy as np
from physical_mel import RATE


class Renderer:
    """A running worker and the factory parameters of every instrument it describes."""

    def __init__(self, executable):
        described = json.loads(subprocess.check_output([str(executable), "--describe"], text=True))
        # An archived worker may describe more than its instruments, under underscored keys.
        self.defaults = {name: parameters for name, parameters in described.items()
                         if not name.startswith("_")}
        self.process = subprocess.Popen([str(executable)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process.stdout.close()

    def render(self, model, notes, params, seconds, hold, rate=RATE, effects=(), inputs=()):
        """One row of mono PCM per note, each `seconds` long at `rate`."""
        request = {"model": model, "notes": [{key: note[key] for key in
                    ("pitch", "velocity", "tuning_cents", "hold", "bends", "expression")
                    if key in note} for note in notes],
                   "seconds": seconds, "hold": hold, "params": params}
        # The analysis rate is also the worker's default.
        if rate != RATE:
            request["sample_rate"] = rate
        if effects:
            request["effects"] = effects
        if inputs:
            request["inputs"] = [str(path) for path in inputs]
        self.process.stdin.write((json.dumps(request, allow_nan=False) + "\n").encode())
        self.process.stdin.flush()
        header = self.process.stdout.read(4)
        if len(header) != 4:
            raise RuntimeError("Rust renderer terminated before PCM header")
        count = int.from_bytes(header, "little")
        expected = round(seconds * rate) * len(notes)
        if count != expected:
            raise RuntimeError(f"unexpected PCM length: {count}, expected {expected}")
        raw = self.process.stdout.read(count * 4)
        if len(raw) != count * 4:
            raise RuntimeError("truncated PCM from Rust renderer")
        return np.frombuffer(raw, dtype="<f4").reshape(len(notes), -1).astype(np.float64)
