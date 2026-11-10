"""Guards for the whole-keyboard piano measurements: they must read known signals back."""

import numpy as np
import piano_register as register
import soundfile as sf

RATE = register.RATE


def struck(f0, seconds=9.0, prompt=30.0, after=3.0, delay=0.25, noise=1e-5):
    """A synthetic two-stage note: partials falling at prompt then after dB/s, with a floor."""
    t = np.arange(int(seconds * RATE)) / RATE
    rng = np.random.default_rng(20261026)
    x = rng.normal(0.0, noise, len(t))
    late = t - delay
    on = late >= 0
    for n in range(1, 7):
        stage = 10 ** (-prompt * late / 20) + 0.03 * 10 ** (-after * late / 20)
        x[on] += 0.5 / n * stage[on] * np.sin(2 * np.pi * f0 * n * late[on])
    return x


def test_onset_finds_the_strike_within_a_few_milliseconds():
    x = struck(261.63)
    assert abs(register.onset(x) / RATE - 0.25) < 0.005


def test_partial_trajectories_recover_a_known_decay():
    x = struck(261.63, prompt=20.0, after=20.0)[int(0.25 * RATE) :]
    freqs = register.find_partials(x, 261.63, 3)
    assert abs(freqs[0] - 261.63) < 0.5
    times, levels = register.trajectories(x, freqs, register.window_size(freqs[0]))
    mask = (times > 0.5) & (times < 2.0)
    slope = -np.polyfit(times[mask], levels[0, mask], 1)[0]
    assert abs(slope - 20.0) < 1.0


def test_knock_bands_below_the_noise_floor_are_unmeasurable():
    floor = np.full(len(register.KNOCK_EDGES), 1.0)
    quiet = np.zeros(RATE)
    bands = register.knock_bands(quiet, 4000.0, floor)
    assert all(band is None for band in bands)
    # Bands reaching above 0.6 f0 belong to the string, not the knock.
    bands = register.knock_bands(np.ones(RATE), 300.0)
    assert bands[1] is not None and bands[2] is None


def test_a_noiseless_render_of_the_recorded_note_has_no_loss(tmp_path):
    path = tmp_path / "Piano.mf.C4.aiff"
    # Long enough for the file's last seconds to be its noise floor, as a recording's are.
    recorded = struck(261.63, seconds=14.0, after=8.0)
    sf.write(path, recorded, RATE, format="AIFF", subtype="FLOAT")
    note = register.Note(path, {"pitch": 60, "velocity": 0.65, "dynamic": "mf"})
    # A renderer has no noise floor; the comparison lends it the recording's.
    clean = struck(261.63, seconds=14.0, after=8.0, noise=0.0)
    losses = register.note_losses(note, clean[register.onset(clean) :], 0.0)
    assert losses["decay"] < 0.3, losses
    assert losses["envelope"] < 0.3, losses
    assert losses["spectrum"] < 0.3, losses
    assert losses["level"] < 0.3, losses


def test_a_faster_rendered_decay_is_penalized():
    recorded = struck(261.63, prompt=30.0, after=3.0)
    faster = struck(261.63, prompt=30.0, after=9.0)

    class Recorded(register.Note):
        def __init__(self, x):
            self.pitch, self.velocity, self.dynamic, self.label = (
                60,
                0.65,
                "mf",
                "mf.C4",
            )
            start = register.onset(x)
            tail = x[-int(1.5 * RATE) :]
            self.level = register.early_level(x[start:])
            x = x[start : start + int(register.SECONDS * RATE)]
            self.f0 = 261.63
            self.freqs = register.find_partials(x, self.f0, 6)
            self.size = register.window_size(self.freqs[0])
            self.times, self.levels = register.trajectories(x, self.freqs, self.size)
            self.floor = np.median(
                register.trajectories(tail, self.freqs, self.size)[1], axis=1
            )
            self.i0 = int(np.searchsorted(self.times, register.T0))
            self.broadband = register.broadband(x)
            self.broadband_floor = float(np.median(register.broadband(tail)))
            self.knock = [None] * (
                len(register.KNOCK_EDGES) * len(register.KNOCK_TIMES)
            )
            self.use = list(range(len(self.freqs)))

    note = Recorded(recorded)
    same = register.note_losses(note, recorded[register.onset(recorded) :], None)
    worse = register.note_losses(note, faster[register.onset(faster) :], None)
    assert worse["decay"] > same["decay"] + 3.0


def test_a_note_tuned_well_away_from_equal_temperament_is_still_found():
    # The recorded C8 lies 56 cents under equal temperament and a piano's own top note two
    # percent above it. Either way its partials are followed from where its first one is.
    for cents in (-56.0, 40.0):
        tuned = 1046.5 * 2 ** (cents / 1200)
        x = struck(tuned, seconds=3.0)[int(0.25 * RATE) :]
        found = register.find_partials(x, 1046.5, 3)
        for n, frequency in enumerate(found, start=1):
            assert abs(1200 * np.log2(frequency / (n * tuned))) < 1.0
