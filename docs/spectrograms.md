# Spectrograms

Right-click a track header or an empty area of its lane and choose **Audio Display →
Spectrogram**. This is available for audio, instrument, drum, singer and bus tracks.
Choose **Waveform** on an audio track, or **Clips** on another track, to restore the
usual display. Clip titles and editing gestures remain available.

Right-click the empty area above the track headers, or below the tracks, and enable
**Project Spectrogram** to show the whole mix above the clip lanes. The × button closes
the overview. It follows the arrangement's horizontal zoom and scroll.

Audio clips show their source file's frequency content. Other tracks show their
performed sound through the same solo routing used by stem export, including buses,
sends and master processing. Singer tracks use their rendered take when available,
otherwise their preview instrument. The project overview follows the current mix,
including mute, solo, effects and automation. Effect tails are included.

Analysis runs on a background worker. Rendered views are refreshed after document
changes; a loading message replaces obsolete results. Opening or closing a view does
not edit the project or change playback. Display choices reset when another project
is opened.

The vertical scale is logarithmic, from 20 Hz to the lower of 20 kHz and Nyquist.
Colours represent −90 to 0 dBFS. Channels are analysed separately so opposite-phase
stereo remains visible. Images pool analysis windows into at most 2,048 time columns;
zooming a long arrangement does not increase that analysis resolution.
