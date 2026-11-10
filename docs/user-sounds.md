# Reusable sounds

[日本語](user-sounds.ja.md)

Select an instrument or drum track, adjust its instrument and insert effects, then choose
**Save as user sound…** in the track inspector. Enter a new name. Saved sounds appear in the
library's **User sounds** section and in library search, and remain available after restarting.
Names must be nonempty and unique; an existing sound is never overwritten implicitly.

Right-click a user sound in the library or search results to:

- **Overwrite from selected track…**: select an instrument or drum track first. Its current
  instrument settings and inserts are captured, then a confirmation names the saved sound and
  source track. Confirming retains the saved name and replaces its contents.
- **Rename sound…**: enter a nonempty name that is not used by another user sound.
- **Duplicate sound…**: save the sound's complete settings and insert chain under a new name.
  The name field suggests an unused copy name; you can change it before saving.
- **Delete sound…**: confirm removal from the library. Overwrite and deletion cannot be undone.

Rename, duplication and deletion also work without a track selected. These library edits do not change
settings already applied to projects, and factory presets remain available. If another window
changes the same saved sound before confirmation, the edit is refused and the library is refreshed.

Choose a saved sound with a destination instrument track selected. The command replaces its
instrument settings and complete insert chain, including effect order, parameters and bypass
switches. Undo restores the previous sound and its automation in one step. Notes, track level,
pan, sends and routing stay in the song. Existing instrument and insert automation is removed
when applying a sound; sidechain sources are not carried between tracks.

SoundFont sounds remember the library file and bank/program. Hosted CLAP and VST3 sounds also
remember their binary paths and opaque plugin state. Those libraries must remain installed.
Applied settings are embedded in the project, so moving or deleting the user sound library
does not change an existing project.

Every physical instrument has a **Presets** disclosure beneath its basic instrument row.
Open it to choose a starting patch, or search directly by its name. Each can be edited,
combined with effects and saved as a new user sound.

| Basic instrument | Starting patches |
| --- | --- |
| Strings | String section, Violin, Viola, Cello, Contrabass, Sul ponticello, Sul tasto |
| Piano | Grand piano, Felt piano, Bright piano |
| Guitar | Acoustic guitar, Nylon guitar, Steel guitar, Muted guitar, Piezo guitar |
| Electric guitar | Clean electric guitar, Warm electric guitar, Bright electric guitar |
| Bass | Finger bass, Pick bass, Muted bass, Upright bass |
| Bell | Classic bell, Soft bell, Bright bell, Tubular bells, Handbell, Singing bowl |
| Mallet | Marimba, Vibraphone, Xylophone, Glockenspiel, Kalimba, Music box, Hard mallet, Muted mallet |
| Reed winds | Clarinet, Soft clarinet, Breathy clarinet |
| Hammered strings | Hammered dulcimer, Bright dulcimer, Muted dulcimer |
| Whistles | Tin whistle, Soft whistle, Airy whistle, Recorder |
| Choir | Mixed choir, Soprano choir, Tenor choir, Bass choir |
| Drums | Natural drum kit, Dry drum kit, Resonant drum kit |

These are editable parameter variants of their existing physical models. The first patch in
each family keeps its previous default parameters. Names describe the intended timbre or
playing style; the variants share the same underlying model. They do not transpose the score,
so write notes in the intended register. Existing projects keep their stored settings.
