# MIDI references in agent conversations

Use **Write melody…** in the agent composer to create a short score, **Attach
selected clip…** to copy the selected MIDI or singer clip, or **Attach MIDI…**
to open a `.mid`/`.midi` file. Dropped MIDI files appear among the attachments;
activate the attachment to edit it.

The same controls are available directly in **AI workshop**. Its
composer keeps separate references from the editor's agent panel. MIDI files
can also be dropped onto that screen. Submit a message such as “use this melody”
alongside the attachments; the director and every song candidate receive the
exact note, lyric, chord and instrument data. A failed file read leaves the
draft available for correction. Submitted reference contents are retained in
the conversation, so later changes to the source file do not change earlier
requests. Back keeps attached drafts; after reopening, activate **Reattach**
to explicitly include a saved local reference in the next message.

The inline editor uses the normal piano roll on an independent conversation
copy. Choose a part with the track arrows. Use the same note creation, movement,
resizing, selection, snapping, scrolling and zoom controls as in the song editor.
Undo and Redo affect only this copy. Double-click a note to enter its word or
syllable in the normal lyric prompt, including platform IME input. Imported notes
keep their original timing until edited.

**Attach score** keeps the edited copy with the message. **Cancel** discards
the editor's changes. Sending or switching conversations commits an open draft
first; a validation or storage error keeps it open. The source clip and MIDI
file are unchanged. Opening a sent MIDI attachment starts a new editable copy;
attaching that copy never changes the earlier message.

Enter confirms a lyric prompt; Escape dismisses it without attaching the score.
Use **Attach score** to keep the edited reference or **Cancel** to return to the
message field. The panel remains scrollable when the editor exceeds its available
space.

Each attachment contains up to 2,048 notes across 64 parts and at most 256 KiB
of encoded score data. Longer references should be shortened before attaching.
The model receives exact tick-based note positions, lengths, velocities, lyrics,
tempo and meter metadata as reference text. Note positions are relative to
`source_origin_ticks`; tempo and meter maps use the original source timeline.
This works through both API and CLI agent backends.

Immutable `.auris-midi` snapshots are retained in `conversation-midi` under
the Auris settings directory. They remain available to local queued and saved
conversations after the app closes. Moving a conversation to another computer
also requires copying its referenced files. MIDI lyric meta events at a note's
start are attached to the first matching note in that source track; a chord
does not duplicate a lyric over all its notes.


## Chord progressions

Choose **Attach chords…** and enter absolute chord names or roman numerals,
for example `| C/E G7 | vi | F |`. A vertical bar separates measures; chords
within one measure share its duration equally. The dialog shows the song's
initial key and meter, captured when it opens. The attachment includes resolved
chord names and exact tick positions, without writing to the harmony lane.
Invalid chords leave the dialog open for correction. References support up to
128 measures and 16 chords per measure.

## Sound choices

Choose **Choose sound…** to open the existing timbre map. Build the map with
**Rescan** if needed, then select a sound using the map, all-sounds list, or
nearest-neighbor list. Existing audition controls remain available when an
instrument track can play the preview. **Attach to conversation** adds the
chosen sound and returns to the originating conversation; it also works without a selected
track. Write the intended role in the message, such as “use this for the lead.”

The attachment identifies the instrument, SoundFont bank/program when present,
and loaded font metadata and path. It carries no audio or font binaries. The
map's **Use** action still applies a sound to the track; attaching a reference
does not change the song. Chord and sound references are immutable JSON files
in the same local snapshot directory as MIDI references.
