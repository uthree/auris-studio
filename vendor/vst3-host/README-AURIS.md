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
