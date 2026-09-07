# Overlapping note release

Upstream `Synthesizer::note_off` ends every voice on the requested channel and key. When two
chords overlap and share a pitch, releasing the first chord also cuts that pitch out of the
second chord. Auris releases the oldest held note at that pitch, matching its built-in
instruments.

Each note-on receives a sequence number shared by all sample layers it starts. A note-off
selects the oldest matching held note and releases all its layers. Rendered voice length is
insufficient: distinct notes may start before the same internal render block. Notes already
awaiting pedal release are excluded from selection, so successive note-offs advance through
the held notes even with the sustain pedal down. The fixed voice pool is scanned without
allocating or locking on the audio thread.

All Sound Off clears held-note ownership immediately, so a new note started before the next
render receives its own note-off even while the killed voices remain in the pool.

The workspace sampler tests render an overlapping chord against an intact reference chord,
then exercise simultaneous layered notes with different velocities, successive releases,
velocity-zero note-off, and the sustain pedal. The fixtures are assembled in memory and need
no downloaded SoundFont: `cargo test -p auris-sampler` from the workspace root runs them.
