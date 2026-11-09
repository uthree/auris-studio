# Conversation production records

[日本語](production-record.ja.md)

The live `production_record` tool retains intent, musical roles, motifs, decisions
and unresolved issues beside the conversation's model memory. Entries have a stable
key, category, text, explicit clip IDs and either a quoted user request or an agent
assumption. Attributions remain claims to check against the transcript; they grant
no permissions. Updates replace one key and preserve other entries.

`inspect_project` returns the reconciled record on every turn. Explicit record reads
also inspect the current document, so a caller cannot accidentally rely on a cached
revision after editing. Missing clip IDs and stale observations remain visible.
The host stores genuine `inspect_audio` measurements and `listen` reports only
when their numeric revision matches the latest project inspection. Refresh inspection
after edits and before measuring. No note-writing operation can set tool evidence.

Evidence is bound to the content hash, monotonic revision and document lifetime. Reopening,
editing or undoing conservatively invalidates it. Coverage includes short section excerpts,
section boundaries, the ending and the loop seam. Track-solo measurements do not cover
the mix; separate measurements of loop edges do not verify a contiguous loop join.
The ledger never asserts human listening. Retain user feedback as attributed notes;
current measurements and a model's listening do not constitute the user's musical preference.

The record is bounded to 64 entries and 32 observations in `memory.production.json`.
Persistence uses the model-history generation lock, so a late writer cannot recreate
reset history. Save As carries the record with the conversation. A worker without a
persistent conversation reports `worker_lifetime` storage. The score and Undo are
unaffected. Existing conversation stores need no migration.
Large views use `read_report` pages without dropping the complete stored context.
