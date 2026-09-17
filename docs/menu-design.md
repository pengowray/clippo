# Clipboard history window (Super+V)

Design for the libcosmic window that replaces the fuzzel picker. Status: proposal, not built.

The one fact the window must make immediate: **what Enter will paste**. Everything else
(search, macros, settings) is a step away from that.

## 1. Window

| | Decision |
|---|---|
| Surface | Layer-shell, `Overlay` layer, `KeyboardInteractivity::Exclusive`, no anchors, on the active output. Not tiled, no title bar. |
| Size | Fixed **800×500 logical px**. Both monitors are scale 2, so the logical viewports are 1280×720 and 960×540. 800×500 fits the smaller one with a 20 px margin and never needs to adapt. |
| Placement | Centred on the output that has the pointer (`IcedOutput::Active`). Not at the cursor: a fixed position keeps spatial memory. |
| Process | Resident. `clippo watch` keeps the iced app alive and maps or unmaps the surface; `clippo menu` becomes a socket message (`menu toggle`). Cold-starting an iced app per keypress costs a few hundred ms and would feel like a launcher, not Win+V. Fallback for no service: `clippo menu` runs the window in-process. |
| Toggle | Second `clippo menu` sends `menu toggle` to the socket; the window also handles Super+V itself in its key handler, because exclusive keyboard focus may stop the compositor shortcut firing. |
| Close and paste | Unmap the surface, `roundtrip`, wait `AFTER_MENU_WAIT` (50 ms, may need tuning to 80 to 120 ms for cosmic-comp to return focus), then copy + `paste::send`. |
| Theme | Follows COSMIC's theme (dark for this user). Selection colour is the accent; selection is also marked by a 3 px left bar so it does not rely on colour alone. |

## 2. Layout

```
+------------------------------------------------------------------------------------------------+
| [ Search history                                                    ]  [ 1000 items ] [ (gear) ]|
+---------------------------------------------------------------------------+--------------------+
| ON CLIPBOARD NOW                                                          |  Paste             |
|| fn main() {                                                              | +----------------+ |
||     println!("hello");                              [T] [x]              | | 14:00     Alt+1| |
||     let x = compute_stuff(&args);          3 more lines · 412 chars      | +----------------+ |
|---------------------------------------------------------------------------| +----------------+ |
|  +--------+  Image 1920×1080 · PNG                          [T] [x]       | | 2026-09-17Alt+2| |
|  |        |  Reading text...                                              | +----------------+ |
|  |  thumb |                                                               | +----------------+ |
|  +--------+                                                               | | 2026-09-17     | |
|---------------------------------------------------------------------------| | 14:00:05  Alt+3| |
|  +--------+  Image 640×480 · JPEG                              [T]        | +----------------+ |
|  |        |  Error 1002: connection refused by upstream                   |                    |
|  |  thumb |  at proxy.internal:8443 while fetching /api/v2/session        |                    |
|  +--------+                                                               |                    |
|---------------------------------------------------------------------------|                    |
|  https://github.com/pengowray/clippo/pull/12                    [T]       |                    |
|---------------------------------------------------------------------------|                    |
|  ## Release notes                                            [M] [T]      |                    |
|  - **Faster paste** on COSMIC                                             |                    |
|  - See [the changelog](https://example.com/log)              6 more lines |                    |
|---------------------------------------------------------------------------|                    |
|  Tuesday meeting notes                                          [T]       |                    |
|  - ship 0.2                                                               |                    |
|  - fix the paste delay                                       2 more lines |                    |
|---------------------------------------------------------------------------|                    |
|  +--------+  Image 800×600 · PNG                                [T]       |                    |
|  |        |  Couldn't read text                                           |                    |
|  +--------+                                                               |                    |
|---------------------------------------------------------------------------|                    |
|  v  412 older items (not used in the last day)                            |                    |
+---------------------------------------------------------------------------+--------------------+
| Enter paste · Shift+Enter paste as plain text · Alt+Enter without Markdown · Shift+Del delete   |
+------------------------------------------------------------------------------------------------+
```

In the mockup row 1 is selected and row 2 is hovered; those are the only rows that show the
`[x]` delete button. `[T]` (paste as plain text) is on every row, greyed on rows 4 and 6
where there is nothing to strip. `[M]` (paste without Markdown) appears only on row 5, the
one row where Markdown was detected. The last row is the collapsed older section (see 3.4).

Proportions: 100 columns × 33 rows ≈ 8×15 px per cell at 800×500. The list column is
~610 px, the macro column ~170 px, the search bar 40 px, the footer 24 px. Roughly 7 rows fit.

Regions, in reading order:

1. **Search** (top, focused on open). Placeholder `Search history`. Typing filters immediately.
   Right of it: item count, then the settings button (gear icon, tooltip `Settings`).
2. **List**. Most recently used first, same order as today. The first row is the entry that
   is on the clipboard now, and has the caption `ON CLIPBOARD NOW` above it (only while no
   search is active and the top entry matches the live clipboard; see 8.1). Entries not used
   in the last 24 hours are collapsed into one row at the bottom (3.4).
3. **Macro column** (right). Heading `Paste`, then one button per macro showing its live value.
4. **Footer**. Key hints. Also where transient feedback (undo, errors) appears; see 7.

Nothing else. Delete all history lives in the settings page: it is rare and destructive,
and does not earn a spot on the main surface.

## 3. Rows

Common to all rows: 12 px padding, 1 px separator, minimum height 40 px, click anywhere
pastes. Selected row: accent background at 20% plus the left bar. Hovered row: 8% highlight.

Actions strip at the row's top right, icon buttons 28×28 px, left to right:

| Button | Shown | Tooltip | Action | Key |
|---|---|---|---|---|
| `M` (Markdown icon) | only when Markdown is detected in a text entry (see 3.5) | `Paste without Markdown` | Paste the plain text with Markdown syntax removed | Alt+Enter |
| `T` (text icon) | every row | `Paste as plain text` | Paste the plain text (image: recognised text) | Shift+Enter |
| `x` | selected and hovered rows only | `Delete` | Delete the entry, with undo | Shift+Delete |

`T` is **enabled** when the entry has something to strip: a text entry with a stored rich
format (HTML or RTF, see 11.2), or an image whose OCR is done and found text. Otherwise it
is greyed with a tooltip saying why:

| Entry | Greyed tooltip |
|---|---|
| Plain text with no rich format stored | `Already plain text` |
| Image, OCR pending | `Reading text, try again in a moment` |
| Image, OCR found nothing | `No text in this image` |
| Image, OCR failed | `Couldn't read text in this image` |
| Image, OCR off | `Text recognition is off. Turn it on in Settings` |

`T` on every row is deliberate: it is the second most used action and the user asked for
it to be visible. `M` only appears where it applies, so its presence is itself the signal
"this row is Markdown". `x` stays hover-only because it is destructive and rare.

### 3.1 Text rows

- Up to **3 lines**, verbatim (leading blank lines skipped). No code detection and no
  monospace; always the UI font. Tabs render as 4 spaces. Whitespace is not collapsed
  (today's fuzzel label collapses it; that hides structure).
- Long single line: one line, ellipsis at the end.
- Overflow indicator at bottom right, muted: `N more lines` when lines were cut, and
  `· N chars` when the preview is longer than 200 chars. Example: `3 more lines · 412 chars`.
  Needs content length from the store (see 11).
- The 400-char preview from the store is enough for display; never load the full blob for
  the list.
- The preview is always the plain-text form. Stored HTML or RTF is never rendered in the
  list; the enabled `T` button is the only sign that formatting exists. A rendered preview
  was considered and rejected: it would make rows unequal in height and slow to lay out.

### 3.2 Image rows

- Thumbnail box **96×96 px**, contain-fit, from the cached thumbnail (`thumbs.rs`, 256 px
  max, already generated by the OCR worker). Very wide or tall images letterbox inside the
  box on a slightly darker background; that is the honest shape and the dimensions text
  beside it says the real size.
- Right of the thumbnail, line 1: `Image 1920×1080 · PNG` (dimensions always present when
  known, format from the mime).
- Lines 2 to 4: OCR text, verbatim first lines, ellipsis, by OCR status:

| Status | Line 2 | Style |
|---|---|---|
| done, text | first 3 lines of OCR text | normal |
| done, empty | `No text found` | muted |
| pending | `Reading text...` | muted, with a small spinner |
| failed | `Couldn't read text` | muted, warning icon |
| none (OCR off) | nothing | |
| pending, no engine installed | `Waiting for a text engine` | muted; shown with the 7.3 banner |

- A pending row updates in place when OCR finishes. The window subscribes to the socket
  (`ocr done <id>`) rather than polling SQLite; fallback: poll every 500 ms while any
  visible row is pending.
- Thumbnail missing (not generated yet): show a grey box with the format label inside;
  request generation and swap in when ready.

### 3.3 Ordering and state

Order is `last_used` desc, as now. No grouping by day: a clipboard is used by recency, and
headings would push the important rows down. Relative time is not shown per row (it is
rarely the question); it appears in the row's tooltip on hover after 700 ms:
`Copied 3 min ago`. Needs `last_used` in `Summary` (see 11).

### 3.4 Older entries

Entries whose `last_used` is more than 24 hours ago are **not mixed into the list**. They
sit behind one row at the bottom:

```
|  v  412 older items (not used in the last day)                            |
```

- The row is a normal list row: Down reaches it, Enter or click expands it. Expanded, the
  older entries follow in the same order and format, and the row text changes to
  `^  Hide 412 older items`. Expanded state lasts while the window is open and resets on the
  next open, so the default view is always the recent set.
- Not shown when there are no older entries. When there are no recent entries but older ones
  exist, the list shows the row alone with the empty-state text above it (section 8).
- Older entries are still kept up to `max_items` and are still deduped: re-copying one moves
  it to the top and into the recent set.
- **Search always includes older entries.** Matches are listed recent first, then a
  non-interactive divider row `Older (not used in the last day)`, then older matches. The
  count reads `12 of 1000 match`. The divider is not shown when there are no older matches.
- Optional expiry: setting `Delete items not used for` (section 10, History), default
  **off**. Recommended default is off: the item cap already bounds the database, and a
  clipboard manager that quietly loses things is worse than one that keeps them behind a
  fold. When turned on, the field defaults to 30 days. Expiry runs in `ingest` alongside
  `enforce_cap`.

### 3.5 Markdown detection

A text entry counts as Markdown when its plain text contains **two or more** of:

- a line starting with `#` to `######` followed by a space
- a fenced code block (a line starting with three backticks or `~~~`)
- an inline link or image, `[text](url)` or `![alt](url)`
- paired emphasis on one line, `**x**`, `__x__`, `*x*`, `_x_` or `~~x~~`, where the opening
  marker is followed by a non-space and the closing marker is preceded by a non-space
- two or more lines starting with `- `, `* `, `+ ` or `1. ` (any number)
- a line starting with `> `
- a pipe table separator line such as `|---|---|`

Detection runs on the full text once at insert time and is stored (`is_markdown`, see
11.1) so the list does not re-scan 1000 entries per open. Two hits are required so that a
single `*` or a lone dash list does not flag ordinary text.

## 4. Search

- Filters on text preview, OCR text, and the `Image W×H` label, case-insensitive,
  substring, all words must match (fuzzy is not needed and produces surprises).
- Matches are highlighted in bold in the preview. For an image where the match is in OCR
  text past line 3, the OCR excerpt scrolls to the first match instead of showing the top.
- The count at the top changes from `1000 items` to `12 of 1000 match`.
- Selection moves to the first match on every keystroke.
- Escape with a non-empty search clears the search; Escape again closes.
- Search performs on the in-memory list (1000 entries × 400 chars is trivial). SQLite FTS is
  not needed.
- Older entries are searched too (3.4).

## 5. Keyboard

The window is keyboard-first. Search has focus the whole time; arrows and Enter act on the
list regardless of focus, so there is no tab-switching between search and list.

| Key | Action |
|---|---|
| Super+V | Toggle the window (handled by the window when it has focus) |
| type | Filter |
| Up / Down | Move selection; no wrap at the ends (predictable) |
| Page Up / Page Down | Move by a page |
| Ctrl+Home / Ctrl+End | First / last row (plain Home and End move the caret in the search field) |
| Enter | Paste selected entry with all its stored formats (close first). On the older-items row: expand or collapse it |
| Shift+Enter | Paste as plain text (image: OCR text). On a row where `T` is greyed, the footer shows the greyed tooltip text instead |
| Alt+Enter | Paste without Markdown (text entries only; works whether or not Markdown was detected, detection only controls the button) |
| Ctrl+Enter | Copy to clipboard only, no paste, window stays open. For when paste_on_select is unwanted once |
| Shift+Delete | Delete selected entry; footer shows undo for 6 s (plain Delete edits the search text) |
| Ctrl+Z | Undo the last delete (while the footer shows it). iced's text input may swallow Ctrl+Z; verify against the libcosmic pin. The footer `Undo` button is the guaranteed path |
| Alt+1 to Alt+9 | Paste macro N (shown on the buttons) |
| Ctrl+, | Open settings |
| Escape | Clear search, or close if search is empty. In settings: back to the list |

Selection on open is always the top row, so `Super+V, Enter` re-pastes the current
clipboard, and `Super+V, Down, Enter` pastes the previous one. That is the Windows habit and
it must stay a two-key habit.

With `Paste after picking an item` off: Enter, click, and the context menu `Paste` copy the
entry and close without pasting; the footer hint reads `Enter copy · Shift+Enter copy as
plain text · Alt+Enter copy without Markdown · Shift+Del delete`. Macros always paste;
inserting text is their only purpose.

Shift+Enter on an image whose OCR is still pending: wait up to 3 s for it, footer shows
`Reading text...`; on failure show 7.2.

## 6. Mouse

- Click a row: paste it.
- Hover a row: highlight and reveal `x`. Click `M`, `T` or `x` does that action without a
  normal paste.
- Right-click a row: context menu with `Paste`, `Paste as plain text`, `Paste without
  Markdown` (text entries only), `Copy only`, `Delete`. Items that are unavailable are
  greyed, not hidden, so the menu has a stable shape.
- Scroll wheel scrolls the list; the selected row does not follow the scroll.
- Click outside the window: nothing (exclusive overlay, there is no "outside" to click; the
  compositor gives us the whole output). Escape or Super+V closes.
- Macro buttons: click pastes. Hover tooltip shows the format string, e.g. `%Y-%m-%d`.

## 7. Feedback and errors

The window is closed by the time a paste succeeds or fails, so paste feedback cannot live
in the window. Rules:

- Anything that happens while the window is open is reported in the **footer**, replacing
  the key hints for 6 s (or until the next keypress for errors).
- Anything after close goes to a **desktop notification** via `notify-send` / the
  `org.freedesktop.Notifications` D-Bus interface (libcosmic apps have zbus available).

### 7.1 Delete and undo

Footer: `Deleted. Undo (Ctrl+Z)` with `Undo` as a link button. Held 6 s. The entry is
removed from the DB immediately; undo restores it at its original position (see 11,
`deleted_at`). No confirmation dialog for single delete.

### 7.2 Errors while open

| Situation | Footer text |
|---|---|
| Shift+Enter on a row where `T` is greyed | the greyed tooltip text from section 3 |
| OCR engine missing when Shift+Enter needs it | `No text recognition engine. Set one up in Settings` |
| Alt+Enter on an image | `Only text entries can have Markdown removed` |
| DB read fails | `Couldn't read history: <error>` |

### 7.3 Banner states (above the list, replaces `ON CLIPBOARD NOW`)

These are the deviations worth a loud mark. One banner at a time, most severe wins.

| Condition | Banner | Detected by |
|---|---|---|
| `clippo watch` not running | `Not recording. Start the clippo service to save new copies` | Socket connect fails |
| OCR is on but no engine is installed | `Text in images is not being read. Set up an engine in Settings` (link) | Socket `status` reply |

With the second banner, pending rows show `Waiting for a text engine` instead of
`Reading text...`, so 1000 rows do not each claim to be busy.

### 7.4 After close (notifications)

| Situation | Notification |
|---|---|
| Paste failed, copy succeeded | title `Copied, but couldn't paste` body `<error>. Press Shift+Insert to paste it yourself.` (the key named is the configured one) |
| Copy failed | title `Couldn't copy` body `<error>` |
| Macro paste failed | same as the first row |

Success is silent. The paste itself is the feedback.

## 8. Empty states

| State | List shows |
|---|---|
| No history at all | Centred: `Nothing copied yet` and below it, muted: `Text and images you copy will show up here` |
| No recent entries, older ones exist | Centred: `Nothing copied in the last day`, with the older-items row below it |
| No history and service not running | Same as the first, with the 7.3 banner above |
| Search with no matches | Centred: `No matches for "foo"` and a `Clear search` link button (Escape does the same) |

The macro column and settings stay usable in every empty state.

### 8.1 `ON CLIPBOARD NOW` caption

Shown above the top row only when the live clipboard matches it (hash compare against
`wl-paste` at open; cost is one process spawn, fine). If the clipboard holds something
clippo did not record (a secret, or clippo was not running), the caption is not shown and
nothing claims otherwise.

## 9. Macros

Right column, heading `Paste`. Three by default, top to bottom in order of expected use:

| # | Default format | Example (button label) | Key |
|---|---|---|---|
| 1 | `%H:%M` | `14:00` | Alt+1 |
| 2 | `%Y-%m-%d` | `2026-09-17` | Alt+2 |
| 3 | `%Y-%m-%d %H:%M:%S` | `2026-09-17 14:00:05` (wraps to 2 lines) | Alt+3 |

- The button label **is the live value**, updated once a second while the window is open, so
  the user sees what they will get. Accelerator printed small at the right (`Alt+1`).
- Tooltip: the format string and a short hint: `%Y-%m-%d · Change in Settings`.
- Clicking or Alt+N: close window, put the text on the clipboard, paste.
- **Not recorded in history.** Mechanism: before copying, the window writes the content hash
  to `$XDG_RUNTIME_DIR/clippo/skip` (one hash per line, entries older than 10 s ignored);
  `ingest` skips a copy whose hash is listed and removes the line. Cross-process, no protocol
  change, works whether or not `watch` is the parent. The hash must match what `ingest`
  computes: `content_hash(TEXT_MIME, bytes)` where bytes are exactly what `wl-paste` will
  deliver (no trailing newline added; copy with `wl-copy --type text/plain;charset=utf-8`).
  If that proves fragile, the blunt fallback is a line `skip-next` that makes `ingest` drop
  the next text copy within 2 s.
- **Previous clipboard restored** after the paste (default on, setting in 10): after
  `paste::send` returns, wait 300 ms, then re-copy the entry that was on the clipboard
  before (the top history entry, if it matched; otherwise do nothing, since we cannot restore
  what we did not record). Restoring re-copies the same content, so dedupe keeps history
  unchanged.
- Configurable: `[[macros]]` array in config with `label` (optional; default is the live
  value) and `format` (strftime via `chrono`). Up to 9 (Alt+1 to Alt+9). Editable in Settings.
  A macro with a `label` shows `label` on line 1 and the live value muted on line 2.

Typing text through the virtual keyboard instead of the clipboard was considered and
rejected: `vkbd.rs` sends a fixed keymap and key combos only; arbitrary Unicode typing means
per-character keymap construction and does not work in every app.

## 10. Settings

Opened by the gear button or Ctrl+,. **Not a separate window**: it replaces the list and
macro column inside the same layer surface (libcosmic dialogs and secondary windows on a
layer-shell app are awkward; a page swap is reliable). Header: back arrow + `Settings`, and
`Reset to defaults` at the right. Escape or the back arrow returns to the list. The search
bar is hidden while the page is shown. Four sections do not fit in ~440 px, so the page
scrolls; sections keep their order.

Sections in order of how often they are touched. Each control shows the current value; the
default is shown muted in the label's help text where it is not obvious from the control.

### History

| Label | Control | Default | Config key |
|---|---|---|---|
| Keep up to | number field, 10 to 100000, suffix `items` | `1000` | `max_items` |
| Delete items not used for | toggle, then number field with suffix `days` (enabled when on) | off; 30 when turned on | `expire_days` (new; absent or 0 = off) |
| Delete all history | button, destructive style | | |

Help text under `Delete items not used for`: `Off keeps everything up to the item limit.`

`Delete all history` opens an inline confirmation in place of the button:
`Delete all 1,000 items? This can't be undone.` with `Delete all` (destructive) and `Cancel`.
The count is real.

### Paste

| Label | Control | Default | Config key |
|---|---|---|---|
| Paste after picking an item | toggle | on | `paste.paste_on_select` |
| Paste after Paste as plain text (Super+Alt+V) | toggle | on | `paste.auto_paste` |
| Super+Alt+V also removes Markdown syntax | toggle | on | `paste.plain_strips_markdown` (new) |
| Paste by pressing | dropdown: `Shift+Insert (works in most apps and terminals)`, `Ctrl+V`, `Ctrl+Shift+V` | Shift+Insert | `paste.keys` |
| Restore the previous clipboard after a macro | toggle | on | `macros.restore_clipboard` (new) |

Help text under `Super+Alt+V also removes Markdown syntax`: `Only when the text looks like
Markdown. Super+V's "Paste as plain text" never removes Markdown; use "Paste without
Markdown" there.` Recommended default **on**: the detection gate (3.5) makes false positives
rare, and the point of Super+Alt+V is "give me the text, not the markup". The user can turn
it off if they paste Markdown source often.

Advanced (collapsed disclosure, `Advanced`):

| Label | Control | Default | Config key |
|---|---|---|---|
| How keys are sent | dropdown: `Wayland virtual keyboard`, `Virtual input device (uinput)`, `Wayland first, then uinput` | Wayland | `paste.method` |
| Wait before pasting (uinput only) | number, ms | `100` | `paste.delay_ms` |
| Release Super and Alt first (uinput only) | toggle | on | `paste.release_modifiers` |

Help text under `How keys are sent`: `uinput needs /dev/uinput to be writable. On Pop!_OS,
install the steam-devices package.` The two uinput-only rows are disabled (greyed, with
`uinput only` in their label) unless the method includes uinput.

### Text in images (OCR)

| Label | Control | Default | Config key |
|---|---|---|---|
| Read text in copied images | dropdown: `Automatic (built-in if set up, else Tesseract)`, `Built-in (ocrs)`, `Tesseract`, `Off` | Automatic | `ocr.engine` |
| Status line | text, not a control | | |
| Set up built-in engine | button, only when ocrs models are missing | | runs `setup-ocr` |
| Tesseract language | text field | `eng` | `ocr.tesseract_lang` |

Status line shows what is actually in use, which is the thing the user cannot see today:
`Using built-in engine` / `Using Tesseract (eng)` / `No engine installed. Images are kept,
text is read once an engine is set up.` / `Off`.

`Set up built-in engine` downloads ~12 MB; the button shows `Downloading...` with progress,
then `Installed`. Errors show under the button.

### Macros

A list of rows, each: `Format` text field, live preview to the right, `Label` text field
(optional), up and down arrows, `x`. `Add macro` below, up to 9. Format help link:
`Format codes` opening the strftime reference in the browser.

### Saving

- Changes save on each control change (no Save button), written with `toml_edit` so
  comments and unknown keys in the user's file survive.
- After writing, the window sends `reload` on the socket; `clippo watch` re-reads config and
  restarts the OCR worker with the new engine. Footer: `Saved`. If the socket is not
  reachable: `Saved. Restart the clippo service to apply paste and OCR settings`.
- `Reset to defaults`: inline confirmation `Reset all settings to defaults?` with `Reset`
  and `Cancel`. Writes an empty config (defaults are the absence of keys).

## 11. Changes to the data model and behaviour

### 11.1 Entries

1. `Summary` gains `last_used: i64`, `content_len: usize` (bytes for images, chars for
   text), `is_markdown: bool`, and `has_rich: bool` (a rich format is stored, 11.2). Add to
   `SUMMARY_COLS`. `is_markdown` is computed at insert by the rules in 3.5.
2. Store a `line_count` for text entries at insert time (or compute from the preview plus
   `content_len`; exact count needs the full text). Cheap to store, so store it.
3. `Store::restore(id, last_used)` for undo, or keep a `deleted_at` column and hard-delete
   on cap enforcement. Prefer `deleted_at`: undo is then a single UPDATE and the blob never
   leaves the DB. Filter `deleted_at IS NULL` in `list`.
4. `expire_days`: `ingest` deletes entries with `last_used` older than N days after
   `enforce_cap`. `list` takes a `since` split so the window gets recent and older sets
   without two queries.
5. `Config` gets `Serialize`; add `expire_days`, `paste.plain_strips_markdown`, `[macros]`
   (`restore_clipboard`, `[[macros.items]]` with `format`, `label`). Writer uses `toml_edit`.
6. Socket protocol (`paste.rs::serve`) gains: `menu toggle`, `reload`, `status` (reply:
   `ocr=<engine|none> watching=1`), `copy <id> [plain|nomd]` (11.2), and a push line
   `ocr done <id>` to connected menu clients. This turns the socket into the one channel
   between service and window.
7. `ingest` reads the skip file (section 9) before upserting.
8. Text preview: keep the first 400 chars but do not trim leading whitespace inside lines;
   the window needs the real shape.
9. `menu::label` and its whitespace collapsing stay for `clippo list` output only.
10. Distinguish `Pending` with no engine from `Pending` in progress at the source: `ingest`
    already knows the engine kind; the window learns the real availability from `status`.

### 11.2 Rich formats

Today `ingest` keeps one type per copy. To make "paste as plain text" mean something and to
give apps their formatting back on a normal paste, store the rich types alongside.

**Storage.** New table `formats(entry_id, mime, content)`, one row per extra type. The
`entries.content` column stays the primary form (plain text, or the image) and the
dedupe hash is computed on the primary only. Re-copying the same text from a different app
replaces the stored formats with the new copy's (newest wins).

**What is kept**, from the types the clipboard owner offers:

| Primary | Extra formats kept |
|---|---|
| Text | `text/html` if it has real formatting (below); `text/rtf` or `application/rtf` if offered |
| Image | `text/html` if offered (a browser image copy carries an `<img>` tag with its source URL and alt text; keeping it lets a paste into a rich editor embed the image instead of a bitmap) |

Everything else (`text/uri-list`, app-private types, `x-kde-*`) is not stored.

**"Real formatting" test for HTML.** Browsers wrap every copy in HTML, so storing all of it
would make `T` enabled on nearly every row and mean nothing. Store `text/html` only when it
contains at least one of: `<b`, `<strong`, `<i`, `<em`, `<u`, `<s`, `<a `, `<h1` to `<h6`,
`<ul`, `<ol`, `<table`, `<img`, `<code`, `<pre`, `<blockquote`, or a `style=` attribute.
Case-insensitive. Otherwise the copy is treated as plain text and `has_rich` is false.

**Size.** Formats are capped at 1 MB each; larger ones are dropped and the copy is stored
as plain. The 400-char preview and the row layout are unaffected.

**Pasting.** A normal paste (Enter, click) offers the primary type plus every stored format
so the target app picks what it understands. `Paste as plain text` offers only
`text/plain;charset=utf-8` (images: the OCR text). `Paste without Markdown` offers only
`text/plain` after stripping (section 12).

**Offering several types at once is the implementation risk.** `wl-copy` serves one type
per process. Options, in order of preference:

1. `wl-clipboard-rs` crate, `copy::Options` with several `MimeSource`s. It supports
   `zwlr_data_control`; check whether the pinned version also supports
   `ext_data_control_v1` (COSMIC offers both today, so `zwlr` is enough for now). The source
   must stay alive while it owns the clipboard, so the resident `clippo watch` process serves
   it from a thread, on the socket request `copy <id>`. Without the service, the window
   falls back to `wl-copy` with the primary type only and formatting is lost for that paste.
2. A small data-control source of our own using `wayland-client` (already a dependency for
   `vkbd.rs`). More code, no new crate.
3. Run one `wl-copy` per type. Does not work: each takes ownership from the last.

**Watcher feedback loop.** When clippo serves a multi-type copy, `wl-paste --watch` sees it
and `ingest` runs. The primary hash matches an existing entry, so it is a bump to the top
(correct) and the stored formats are replaced with identical ones. No skip needed.

**`clippo plain` (Super+Alt+V)** keeps working on the live clipboard as now, and gains the
Markdown step when `paste.plain_strips_markdown` is on and detection (3.5) fires.

## 12. Removing Markdown

Used by `Paste without Markdown` (Alt+Enter, the `M` button, the context menu) and by
`clippo plain` when the setting is on. Input is the plain text; output is plain text. The
goal is readable prose, not a Markdown parser: rules are line-based and regex-friendly.
Apply in this order.

1. **Fenced code.** From a line starting with three backticks or `~~~` to the next such
   line: remove the fence lines, keep the contents verbatim and skip every other rule for
   them. An unclosed fence runs to the end.
2. **Indented code** (4 spaces or a tab, after a blank line): keep verbatim, skip other
   rules.
3. **Headings.** Remove a leading `#` to `######` plus its space, and trailing spaces plus
   `#`s. Setext: a line of only `=` or `-` (3 or more) directly under a text line is removed.
4. **Horizontal rules.** A line of only `***`, `---` or `___` (3 or more, spaces allowed)
   is removed.
5. **Block quotes.** Remove a leading `> ` (or `>`), repeatedly for nested quotes.
6. **Lists.** A leading `- `, `* ` or `+ ` (after optional indent) becomes `- ` with the
   same indent. Task markers `[ ] ` and `[x] ` after it are removed. Ordered items
   (`1. `, `1) `) are kept as written.
7. **Tables.** A line whose trimmed form starts and ends with `|`: remove the outer pipes,
   replace each inner ` | ` with two spaces, trim. A separator line (`|---|:--:|`) is removed.
8. **Reference definitions.** A line matching `[label]: url ...` is removed.
9. **Images.** `![alt](url)` becomes `alt`; `![alt][ref]` becomes `alt`.
10. **Links.** `[text](url)` and `[text][ref]` become `text`; `<http://x>` and
    `<mailto:x>` become the address. Bare URLs are untouched.
11. **Inline code.** `` `x` `` becomes `x` (also double-backtick spans).
12. **Emphasis.** `**x**`, `__x__`, `*x*`, `_x_`, `~~x~~` become `x`, only when the opening
    marker is followed by a non-space and the closing marker is preceded by a non-space, on
    the same line. Underscores inside words (`snake_case_name`) are untouched. Unpaired
    markers stay.
13. **Escapes.** `\` before any of ``\`*_{}[]()#+-.!|>~`` is removed.
14. **Hard breaks.** Two or more trailing spaces are trimmed; a trailing backslash line
    break is removed.
15. **Blank lines.** Runs of three or more blank lines collapse to two.

Not handled, on purpose: HTML tags inside Markdown (left as is), footnotes (`[^1]` left as
is), nested emphasis across lines. If the output equals the input, the action still
"succeeds" silently; nothing claims it changed something.

Tests to write: each rule alone, code fence protecting a heading inside it, `5 * 3 * 2`
untouched, `a_b_c` untouched, a link inside bold, a table with a separator.

## 13. libcosmic notes and risks

- **Resident process**: strongly recommended (section 1). libcosmic's layer-shell support
  (`cosmic::iced::platform_specific::shell::commands::layer_surface`) can create and destroy
  surfaces on demand from a running app. This is the applet-popup pattern. It is now also
  required for multi-type pastes (11.2), which need a process that stays alive to serve the
  clipboard.
- **Exclusive keyboard**: while the overlay is mapped, other compositor shortcuts may not
  fire. Super+V must be caught in-window. If cosmic-comp still delivers the global shortcut,
  the socket toggle handles it; both paths lead to the same close.
- **Focus return timing**: the 50 ms `AFTER_MENU_WAIT` was tuned for fuzzel. Unmapping a
  layer surface and getting focus back to the previous toplevel may need more; make it a
  constant and test.
- **Per-row hover reveal**: `mouse_area` per row plus a `hovered: Option<usize>` in state.
  Fine, just per-row wiring.
- **Images**: `cosmic::widget::image` with `image::Handle::from_path` for thumbnails; cache
  handles per id so scrolling does not re-decode.
- **Settings inside the surface**: page swap, not `cosmic::dialog` or a second window.
  Multi-window on layer-shell is the least tested corner of libcosmic.
- **Text highlighting of search matches**: iced `rich_text`/`span` exists in recent iced;
  check the libcosmic pin. Fallback: no bold, rely on the row being a match.
- **Notifications**: `notify-rust` or `zbus` directly; both are small.
- **1 s macro tick**: `iced::time::every(1s)` subscription only while mapped.
- **Multi-type clipboard source** (11.2): `wl-clipboard-rs` version and protocol support
  need checking before committing to it.

## 14. Open questions

The previous revision's questions were accepted. Remaining, with recommendations:

| Question | Recommendation |
|---|---|
| Expiry default | Off. Turning it on defaults to 30 days. |
| Should the older-items row remember being expanded between opens? | No. The recent set is the point of the fold; one Enter reopens it. |
| Should `T` be enabled on plain-only text (as a no-op) to keep rows uniform? | No. A greyed button with `Already plain text` tells the user something true; an always-on button tells them nothing. |
| Keep `text/rtf`? | Yes, only if offered; it is rare on Wayland and costs nothing. |
| Keep `text/html` for browser image copies? | Yes. Rich editors then embed the image with its source; the bitmap is still there for everything else. |
| `Paste without Markdown` for images (OCR text)? | No. OCR output is not Markdown; keep the action to text entries so its meaning stays fixed. |
| Super+Alt+V strips Markdown by default? | On, gated by detection. |
| Bullet character after stripping | `- `. ASCII, survives every target, reads as a list. |

## 15. User-visible strings

| Where | String | Note |
|---|---|---|
| Search field, placeholder | `Search history` | |
| Header, count | `1000 items` | number is live |
| Header, count while searching | `12 of 1000 match` | |
| Header, settings button tooltip | `Settings` | |
| List, caption above top row | `ON CLIPBOARD NOW` | small caps, muted |
| List, older row, collapsed | `412 older items (not used in the last day)` | with a chevron; count is live |
| List, older row, expanded | `Hide 412 older items` | |
| List, divider in search results | `Older (not used in the last day)` | not selectable |
| Image row, line 1 | `Image 1920×1080 · PNG` | format from mime, upper-case |
| Image row, no dimensions | `Image · PNG` | |
| Image row, OCR pending | `Reading text...` | with spinner |
| Image row, OCR pending, no engine | `Waiting for a text engine` | only with the engine banner |
| Image row, OCR done, empty | `No text found` | muted |
| Image row, OCR failed | `Couldn't read text` | muted, warning icon |
| Text row, overflow | `3 more lines · 412 chars` | either part may be absent |
| Text row, overflow, lines only | `6 more lines` | |
| Row hover tooltip | `Copied 3 min ago` | relative; `just now`, `N min ago`, `N h ago`, `Yesterday 14:00`, `12 Sep 14:00` |
| Row hover tooltip, image | `Copied 3 min ago · 1.2 MB` | |
| Row action, tooltip | `Paste without Markdown` | `M` button, only on detected rows |
| Row action, tooltip | `Paste as plain text` | `T` button, enabled |
| Row action, greyed tooltip | `Already plain text` | `T`, plain text with no rich format |
| Row action, greyed tooltip | `Reading text, try again in a moment` | `T`, image, OCR pending |
| Row action, greyed tooltip | `No text in this image` | `T`, image, OCR empty |
| Row action, greyed tooltip | `Couldn't read text in this image` | `T`, image, OCR failed |
| Row action, greyed tooltip | `Text recognition is off. Turn it on in Settings` | `T`, image, OCR off |
| Row action, tooltip | `Delete` | |
| Row context menu | `Paste` / `Paste as plain text` / `Paste without Markdown` / `Copy only` / `Delete` | unavailable items greyed |
| Macro column heading | `Paste` | |
| Macro button | `14:00` / `2026-09-17` / `2026-09-17 14:00:05` | live values |
| Macro button, accelerator | `Alt+1` | small |
| Macro tooltip | `%Y-%m-%d · Change in Settings` | |
| Footer, hints | `Enter paste · Shift+Enter paste as plain text · Alt+Enter without Markdown · Shift+Del delete` | Esc hint dropped for width; Escape is universal |
| Footer, hints, paste on pick off | `Enter copy · Shift+Enter copy as plain text · Alt+Enter copy without Markdown · Shift+Del delete` | |
| Footer, after delete | `Deleted. Undo (Ctrl+Z)` | `Undo` is a link button |
| Footer, waiting for OCR on Shift+Enter | `Reading text...` | |
| Footer, error | any of the greyed `T` tooltips above | Shift+Enter on a greyed row |
| Footer, error | `No text recognition engine. Set one up in Settings` | `Settings` is a link |
| Footer, error | `Only text entries can have Markdown removed` | Alt+Enter on an image |
| Footer, error | `Couldn't read history: <error>` | |
| Footer, settings saved | `Saved` | |
| Footer, settings saved, no service | `Saved. Restart the clippo service to apply paste and OCR settings` | |
| Banner | `Not recording. Start the clippo service to save new copies` | |
| Banner | `Text in images is not being read. Set up an engine in Settings` | `Settings` is a link |
| Empty state, title | `Nothing copied yet` | |
| Empty state, body | `Text and images you copy will show up here` | |
| Empty state, only older entries | `Nothing copied in the last day` | older row shown below |
| Empty search, title | `No matches for "foo"` | |
| Empty search, button | `Clear search` | |
| Notification, paste failed | title `Copied, but couldn't paste` body `<error>. Press Shift+Insert to paste it yourself.` | key name follows config |
| Notification, copy failed | title `Couldn't copy` body `<error>` | |
| Settings, header | `Settings` | |
| Settings, back button tooltip | `Back` | |
| Settings, header button | `Reset to defaults` | |
| Settings, reset confirm | `Reset all settings to defaults?` + `Reset` / `Cancel` | |
| Settings, section | `History` | |
| Settings, History | `Keep up to` + `items` suffix | |
| Settings, History | `Delete items not used for` + `days` suffix | toggle + number |
| Settings, History, help | `Off keeps everything up to the item limit.` | |
| Settings, History | `Delete all history` | destructive button |
| Settings, delete confirm | `Delete all 1,000 items? This can't be undone.` + `Delete all` / `Cancel` | count is live |
| Settings, section | `Paste` | |
| Settings, Paste | `Paste after picking an item` | |
| Settings, Paste | `Paste after Paste as plain text (Super+Alt+V)` | |
| Settings, Paste | `Super+Alt+V also removes Markdown syntax` | |
| Settings, Paste, help | `Only when the text looks like Markdown. Super+V's "Paste as plain text" never removes Markdown; use "Paste without Markdown" there.` | |
| Settings, Paste | `Paste by pressing` | |
| Settings, Paste, options | `Shift+Insert (works in most apps and terminals)` / `Ctrl+V` / `Ctrl+Shift+V` | |
| Settings, Paste | `Restore the previous clipboard after a macro` | |
| Settings, disclosure | `Advanced` | |
| Settings, Advanced | `How keys are sent` | |
| Settings, Advanced, options | `Wayland virtual keyboard` / `Virtual input device (uinput)` / `Wayland first, then uinput` | |
| Settings, Advanced, help | `uinput needs /dev/uinput to be writable. On Pop!_OS, install the steam-devices package.` | |
| Settings, Advanced | `Wait before pasting (uinput only)` + `ms` suffix | |
| Settings, Advanced | `Release Super and Alt first (uinput only)` | |
| Settings, section | `Text in images (OCR)` | |
| Settings, OCR | `Read text in copied images` | |
| Settings, OCR, options | `Automatic (built-in if set up, else Tesseract)` / `Built-in (ocrs)` / `Tesseract` / `Off` | |
| Settings, OCR, status | `Using built-in engine` / `Using Tesseract (eng)` / `No engine installed. Images are kept, text is read once an engine is set up.` / `Off` | |
| Settings, OCR | `Set up built-in engine` | button; `Downloading...` then `Installed` |
| Settings, OCR | `Tesseract language` | |
| Settings, section | `Macros` | |
| Settings, Macros | `Format` / `Label` / `Add macro` / `Format codes` | |
