# Conversational songwriting

Choose **Create with AI…** in the project chooser. Describe the music in the message
box, including instruments, mood, duration, voices, lyrics and any material to preserve.
Enter sends a message; Shift+Enter inserts a line break. Confirming Japanese IME
composition does not send the message. Provider/model and reasoning effort are local
to this workshop; changing them does not overwrite the application's agent settings.

The director asks a focused question when a consequential detail is unclear. Once
the direction is sufficiently clear, it assigns three alternatives to independent
composer workers. Each worker owns its own headless session and edits real music
through the session tools. Progress reports observed work: harmony, melody,
arrangement, singing and audition rendering. Model requests run in parallel;
voice processing and final rendering share a conservative audio-work limit.
Candidates are bounded to five minutes plus an ending, and each worker has a time
and tool-call budget.

Once a worker has playable notes, its card offers an audition marked **Work in progress**.
Draft audio is captured at most once every ten seconds after score changes and rendered
on a background worker, sharing the voice/render work limit. The previous audition stays
available while a newer one is being prepared. Playback, seeking and volume work during
composition. New audio does not interrupt a playing snapshot; **Play latest audio** switches
to the newest one from the beginning. Unfinished singer parts may still use a guide sound.
Draft auditions are temporary and cannot be selected or saved as finished candidates.

Completed alternatives can also be auditioned while other workers continue. Stop applies
to one candidate or the entire batch. Regenerating a completed candidate creates a new artifact identity and leaves
earlier results available. Selecting **Continue with this song** establishes the
exact score for the next request without closing the conversation. Ask for revisions
or new alternatives from that score. Earlier comparisons remain in the transcript.
Selection appears in the chat immediately while the director prepares three to five
numbered arrangement or improvement suggestions. Choose an option in the message box
or describe another change. Suggestions alone do not launch another production batch.
The choice is recorded automatically when **Use my preferences** is enabled, before
requesting the director's response, so a provider failure does not lose the choice.
Explicitly protected note tracks are checked before a descendant is published.

**Open editor** adopts the selected document as an undoable edit and opens a normal
agent conversation containing the songwriting history and selected direction. The
editor's model continues against the current document. Candidate documents and
auditions remain available in the workshop history.

## Preference evidence

**Use my preferences** controls recording and retrieval of cross-project choices.
**Selection history** shows the selected direction and its request context. Remove
an individual choice if it is misleading, or clear the history. Disabling preferences
keeps existing records but neither records new votes nor includes them in prompts.

A choice is weak, contextual evidence. It does not prove why a song was preferred,
and an unselected song is not automatically disliked. Re-selecting within the same
comparison replaces its vote. A choice with only one alternative is recorded as an
individual choice, not a comparative preference. Current explicit instructions take priority, and the director
is asked to retain an exploratory alternative. This is prompt personalization using
recorded choices, not model-weight training or a claim of measured musical improvement.

Evidence is stored locally in `song-preferences.json` in the application configuration
directory. At most eight recent observations, with their request context and candidate
descriptions, are included in a director request when personalization is enabled.

## Storage and recovery

Successful operations checkpoint the private score and its owned assets. Recoverable
candidate failures restart the composer from that checkpoint up to three times, with
2, 4 and 8 second cancellable waits. The composer receives the last failure and must
inspect existing work before continuing. Each attempt has its own time and tool-call
budget. Other candidates continue independently. Cancellation, denied operations,
protected-note violations and checkpoint storage failures stop automatic recovery.

Candidate cards show the recovery count and the latest error. After the automatic
limit, **Resume recovery** explicitly starts another attempt with a fresh three-recovery
budget and the currently selected provider/model. It retains the same candidate,
assignment and checkpoint. Before the first checkpoint exists, recovery starts from
the original parent or a blank score. Fix unavailable resources before requesting
recovery. Completed candidates keep the separate regeneration action.

The workshop owns a sidecar under the project's conversation store. Unsaved documents
use the same persistent recovery identity as ordinary agent conversations. The sidecar
contains the readable transcript, draft, comparison identities, parent links, exact
candidate projects and audition WAVs. Save As copies these with conversation storage;
candidate-local assets retain relative references on disk.

Only one window may own a workshop store at a time. Reopening retains finished work
and marks incomplete candidates as interrupted; it does not automatically restart
model requests. Storage errors are reported rather than replacing unreadable history.
Audition loading is limited to the viewed comparison and selected baseline.
