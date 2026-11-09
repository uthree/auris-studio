# Conversation commands

The **AI workshop** conversation and the editor's **Agent** panel
share these local slash commands:

| Command | Action |
| --- | --- |
| `/model <name>` | Select a model; partial names and provider-qualified suggestions are supported. |
| `/effort <level>` | Set reasoning effort: `default`, `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. OpenCode uses the selected model variant instead. |
| `/help` | Show the commands available in the current conversation. |

Type `/` to see suggestions. Up and Down select a suggestion. Tab or a click
fills the input without executing it. Enter completes and executes a complete
command; a command that needs an argument remains in the input. Escape dismisses
suggestions while keeping the input focused. Shift+Enter inserts a newline.

Commands run locally and keep draft attachments. Song conversation model choices
apply to subsequent requests in that conversation; they do not change the editor's
saved model settings. Stop running song generation before changing those choices.

The Agent panel also provides approval modes (`/mode` and its aliases), permission
rules (`/permissions`, `/allow`, `/deny`, `/default`), and context summarization
(`/compact`). Its `/help` lists these alongside the shared commands.

## Song request examples and predictions

An empty song composer cycles through six localized examples every eight seconds.
Opening it again starts with a different example. Typing keeps the current example
from rotating underneath the draft.

For configured providers with isolated text generation, pausing at the end of a
draft of 4–2048 characters requests a short continuation. Requests wait for one
second of inactivity, run at most once every four seconds, and time out after
15 seconds. Only the draft is sent to the selected model; project data, history,
and attachments are excluded. No tools are available to the prediction request.

Tab or a click appends a displayed prediction without sending the message. Escape
dismisses it. Slash completion takes priority. Edits, provider changes, and IME
composition invalidate predictions. A failing provider stops automatic prediction
until its settings change or the song screen is reopened.
