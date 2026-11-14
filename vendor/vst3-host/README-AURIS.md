# Auris VST3 host fork

This directory contains `vst3-host` 0.9.0 from crates.io.

Auris owns each render instance on one audio thread. The published crate's bus-aware path still
locks its host event, parameter and meter storage on every block, and its event enqueue reports
success after a fixed queue has dropped an event. That can turn a dense note-on/note-off block
into a hanging note.

The fork keeps the ordinary shared API unchanged and adds an explicit single-owner realtime
mode. That mode transfers the preallocated input event and parameter queues to exclusive access,
disconnects processor output events/parameters, skips editor feedback and level metering,
rejects a batch before partially queuing it, and leaves plugin teardown on the thread that
created the runner. It also exposes the VST3-defined realtime reset transition
(`setProcessing(false)` then `setProcessing(true)`) so an Auris panic can clear effect DSP
history without calling the control-thread-only component activation API. The plugin's own
`process` and `setProcessing` implementations remain responsible for their realtime behaviour.

Remove the fork once an upstream release provides an equivalent bus-aware, bounded,
allocation-free and lock-free owner API.

`get_detailed_plugin_info` also reports each class's own sub-categories (`ClassInfo::sub_categories`,
from `PClassInfo2`/`PClassInfoW`). The bundle-level category describes only the first audio class,
which classified every class of a shell bundle such as WaveShell as whatever that first one was.

On macOS, loads of one bundle are shared and counted (`module_loader::shared`). `CFBundleCreate`
returns the same bundle object for the same URL and `CFBundleLoadExecutable` is not counted, so
the published crate's unload on every handle drop let a discovery pass unmap the code of a live
instance of the same plugin; the next call into it crashed. The last handle now unloads.

A `moduleinfo.json` that is empty or only whitespace is treated as absent, so the factory is
asked instead. Some shipping bundles carry a zero-byte file, which the published crate rejected as
invalid JSON and so refused to load the plugin at all.

A note-on can carry a tuning. VST3's note-on event has a per-note tuning field in cents, which
the MIDI-shaped `MidiEvent` has nowhere to put, so the realtime path always wrote zero there.
`Plugin::send_tuned_note_on_at` is that path with the field filled in, for Auris's tunings: the
note is tracked and released as an ordinary note-on at its key, so a `MidiEvent::NoteOff` for
the key ends it. A process-isolated plugin is sent the key untuned.
