"""Small, explicit physical-model experiments; no recording enters the runtime."""

import numpy as np
from physical_mel import RATE
from scipy.signal import sosfilt

MODE_HZ = (120.0, 240.0, 480.0, 960.0, 1920.0, 3840.0)
MODE_Q = 8.0
REGISTER_CENTER = {"piano": 60.0, "guitar": 55.0, "violin": 69.0}
REGISTER_BOUNDS = ((-0.2, 0.2), (-0.8, 0.8), (-0.08, 0.08))


def mode_sections(rate=RATE):
    """Constant-peak bandpasses, with the same float32 coefficients as Rust."""
    sections = []
    for frequency in MODE_HZ:
        omega = 2 * np.pi * frequency / rate
        alpha = np.sin(omega) / (2 * MODE_Q)
        sections.append(np.array([alpha, 0, -alpha, 1 + alpha,
                                  -2 * np.cos(omega), 1 - alpha]) / (1 + alpha))
    return np.array(sections, dtype=np.float32).astype(np.float64)


def mode_basis(audio, rate=RATE):
    """Six independent, causal resonances of the already colored signal."""
    return np.array([sosfilt(section[None, :], audio, axis=-1)
                     for section in mode_sections(rate)])


def resonant_body(audio, gains, notes, response=0.0, rate=RATE):
    """Apply modes before output expression, preserving Rust's causal ordering."""
    # Undo only output expression; the vibrating string keeps the supplied bow motion.
    expression = np.ones_like(audio)
    from scipy.signal import lfilter

    for index, note in enumerate(notes):
        points = note.get("expression", [])
        if not points:
            continue
        frames = np.floor(np.array([p["seconds"] for p in points], dtype=np.float32)
                          * np.float32(rate) + 0.5).astype(int)
        values = np.r_[1.0, [p["value"] for p in points]]
        expression[index] = values[np.searchsorted(frames, np.arange(audio.shape[1]), side="right")]
        if response > 0:
            step = float(np.float32(1 - np.exp(np.float32(-1 / (rate * response)))))
            expression[index], _ = lfilter([step], [1, -(1 - step)], expression[index],
                                           zi=[expression[index, 0] * (1 - step)])
    if np.any(expression <= 0):
        raise ValueError("resonance surrogate needs a positive expression guide")
    dry = audio / expression
    return (dry + np.einsum("m,mnt->nt", gains, mode_basis(dry, rate))) * expression


def register_params(defaults, model, pitch, slopes):
    """Bounded continuous contact and loss changes, per two octaves."""
    x = np.clip((pitch - REGISTER_CENTER[model]) / 24.0, -1.5, 1.5)
    return {"hardness": float(np.clip(defaults["hardness"] + slopes[0] * x, 0, 1)),
            "decay": float(np.clip(defaults["decay"] * np.exp(slopes[1] * x), 0.1, 12)),
            "damping": float(np.clip(defaults["damping"] + slopes[2] * x, 0, 1))}


def gesture(note, amount):
    """Duration-conditioned bow swell; uses no measured reference envelope."""
    hold = note["hold"]
    count = max(2, int(np.ceil(hold / 0.05)))
    times = np.arange(count) * hold / count
    return [{"seconds": float(t), "value": float(1 - amount * (1 - np.sin(np.pi * t / hold)))}
            for t in times]


def render_extension(corpus, renderer, model, profile, rate=RATE, automatic=False):
    """Actual Rust excitation/loss and bow controls; causal modal surrogate."""
    from scipy.signal import resample_poly

    result = []
    for group in corpus.groups:
        notes = []
        for original in group["notes"]:
            note = original.copy()
            if model == "violin":
                if automatic:
                    note["expression"] = gesture(note, profile.get("gesture", 0.0))
                else:
                    note["expression"] = [{"seconds": p["seconds"],
                                           "value": p["value"] ** profile.get("bow_exponent", 1.0)}
                                          for p in note.get("expression", [])]
            notes.append(note)
        slopes = profile.get("register", [0, 0, 0])
        if any(slopes):
            audio = np.concatenate([renderer.render(model, [note], register_params(
                renderer.defaults[model], model, note["pitch"], slopes),
                group["seconds"], group["seconds"], rate) for note in notes])
        else:
            audio = renderer.render(model, notes, {}, group["seconds"], group["seconds"], rate)
        gains = profile.get("modes", [0] * len(MODE_HZ))
        if any(gains):
            audio = resonant_body(audio, gains, notes, corpus.response(renderer, {}), rate)
        if rate != RATE:
            audio = resample_poly(audio, 1, rate // RATE, axis=-1)[:, :group["reference"].shape[1]]
        result.append(audio)
    return result
