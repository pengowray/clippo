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
| Close and paste | Unmap the surface, `roundtrip`, wait `AFTER_MENU_WAIT` (50 ms, may need tuning to 80 to 120 ms for cosmic-comp to return focus), then `copy_entry` + `paste::send`. |
| Theme | Follows COSMIC's theme (dark for this user). Selection colour is the accent; selection is also marked by a 3 px left bar so it does not rely on colour alone. |

## 2. Layout

```
+------------------------------------------------------------------------------------------------+
| [ Search history                                                    ]  [ 1000 items ] [ (gear) ]|
+---------------------------------------------------------------------------+--------------------+
| ON CLIPBOARD NOW                                                          |  Paste             |
|| fn main() {                                                              | +----------------+ |
||     println!("hello");                              [T] [x]              | | 14:00      Alt1| |
||     let x = compute_stuff(&args);          3 more lines · 412 chars      | +----------------+ |
|---------------------------------------------------------------------------| +----------------+ |
|  +--------+  Image 1920×1080 · PNG                          [T] [x]       | | 2026-09-17 Alt2| |
|  |        |  Reading text...                                              | +----------------+ |
|  |  thumb |                                                               | +----------------+ |
|  +--------+                                                               | | 2026-09-17     | |
|---------------------------------------------------------------------------| | 14:00:05   Alt3| |
|  +--------+  Image 640×480 · JPEG                                         | +----------------+ |
|  |        |  Error 1002: connection refused by upstream                   |                    |
|  |  thumb |  at proxy.internal:8443 while fetching /api/v2/session        |                    |
|  +--------+                                                               |                    |
|---------------------------------------------------------------------------|                    |
|  https://github.com/pengowray/clippo/pull/12                              |                    |
|---------------------------------------------------------------------------|                    |
|  +--------+  Image 4000×300 · PNG                                         |                    |
|  |========|  No text found                                                |                    |
|  +--------+                                                               |                    |
|---------------------------------------------------------------------------|                    |
|  Tuesday meeting notes                                                    |                    |
|  - ship 0.2                                                               |                    |
|  - fix the paste delay                                            9 lines |                    |
|---------------------------------------------------------------------------|                    |
|  +--------+  Image 800×600 · PNG                                          |                    |
|  |        |  Couldn't read text                                           |                    |
|  |  thumb |                                                               |                    |
|  +--------+                                                               |                    |
+---------------------------------------------------------------------------+--------------------+
| Enter paste · Shift+Enter paste as text · Del delete · Esc close                                |
+------------------------------------------------------------------------------------------------+
```

Proportions: 100 columns × 32 rows ≈ 8×16 px per cell at 800×500. The list column is
~610 px, the macro column ~170 px, the search bar 40 px, the footer 24 px. Roughly 7 rows fit.

Regions, in reading order:

1. **Search** (top, focused on open). Placeholder `Search history`. Typing filters immediately.
   Right of it: item count, then the settings button (gear icon, tooltip `Settings`).
2. **List**. Most recently used first, same order as today. The first row is the entry that
   is on the clipboard now, and has the caption `ON CLIPBOARD NOW` above it (only while no
   search is active and the top entry matches the live clipboard; see 8.1).
3. **Macro column** (right). Heading `Paste`, then one button per macro showing its live value.
4. **Footer**. Key hints. Also where transient feedback (undo, errors) appears; see 7.

Nothing else. Delete all history lives in the settings page: it is rare and destructive,
and does not earn a spot on the main surface.

## 3. Rows

Common to all rows: 12 px padding, 1 px separator, minimum height 40 px, click anywhere
pastes. Selected row: accent background at 20% plus the left bar. Hovered row: 8% highlight.

Actions strip (`[T]` and `[x]` in the mockup) sits at the row's top right and is shown for
the **selected** row and the **hovered** row only, so keyboard users see it too. Two icon
buttons, 28×28 px, with tooltips:

| Button | Tooltip | Action | Key |
|---|---|---|---|
| `T` (text icon) | `Paste as text` | Paste the plain text (for images, the recognised text) | Shift+Enter |
| `x` | `Delete` | Delete the entry, with undo | Delete |

For a text entry `Paste as text` is still shown, since `text/html` and rich copies exist in
principle; for `text/plain` it does the same as `Paste`, which is fine.

### 3.1 Text rows

- Up to **3 lines**, verbatim (leading blank lines skipped), monospace only if the content
  looks like code is *not* attempted; use the UI font. Tabs render as 4 spaces. Whitespace is
  not collapsed (today's fuzzel label collapses it; that hides structure).
- Long single line: one line, ellipsis at the end.
- Overflow indicator at bottom right, muted: `N more lines` when lines were cut, and
  `· N chars` when the preview is longer than 200 chars. Example: `3 more lines · 412 chars`.
  Needs content length from the store (see 9).
- The 400-char preview from the store is enough for display; never load the full blob for
  the list.

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
| pending, but no engine installed | `Reading text...` is wrong here; see 7.3 | |

- A pending row updates in place when OCR finishes. The window subscribes to the socket
  (`ocr done <id>`) rather than polling SQLite; fallback: poll every 500 ms while any
  visible row is pending.
- Thumbnail missing (not generated yet): show a grey box with the format label inside;
  request generation and swap in when ready.

### 3.3 Ordering and state

Order is `last_used` desc, as now. No grouping by day: a clipboard is used by recency, and
headings would push the important rows down. Relative time is not shown per row (it is
rarely the question); it appears in the row's tooltip on hover after 700 ms:
`Copied 3 min ago`. Needs `last_used` in `Summary` (see 9).

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

## 5. Keyboard

The window is keyboard-first. Search has focus the whole time; arrows and Enter act on the
list regardless of focus, so there is no tab-switching between search and list.

| Key | Action |
|---|---|
| Super+V | Toggle the window (handled by the window when it has focus) |
| type | Filter |
| Up / Down | Move selection; wraps at the ends is off (predictable) |
| Page Up / Page Down, Home / End | Move by a page, or to first/last |
| Enter | Paste selected entry (close first) |
| Shift+Enter | Paste as plain text (image: OCR text) |
| Ctrl+Enter | Copy to clipboard only, no paste, window stays open. For when paste_on_select is unwanted once. |
| Delete | Delete selected entry; footer shows undo for 6 s |
| Ctrl+Z | Undo the last delete (while the footer shows it) |
| Alt+1, Alt+2, Alt+3 | Paste macro 1 to 3 (shown on the buttons) |
| Ctrl+, | Open settings |
| Escape | Clear search, or close if search is empty. In settings: back to the list. |

Selection on open is always the top row, so `Super+V, Enter` re-pastes the current
clipboard, and `Super+V, Down, Enter` pastes the previous one. That is the Windows habit and
it must stay a two-key habit.

Shift+Enter on an image whose OCR is still pending: wait up to 3 s for it, footer shows
`Reading text...`; on failure show 7.2.

## 6. Mouse

- Click a row: paste it.
- Hover a row: highlight and reveal the action strip. Click `T` or `x` does the action
  without pasting.
- Right-click a row: context menu with `Paste`, `Paste as text`, `Copy only`, `Delete`. Same
  four verbs as the keyboard; no extras.
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
removed from the DB immediately; undo restores it at its original position (see 9,
`restore`). No confirmation dialog for single delete.

### 7.2 Errors while open

| Situation | Footer text |
|---|---|
| Paste-as-text on an image with no OCR text (failed or empty) | `No text in this image` |
| OCR engine missing when Shift+Enter needs it | `No text recognition engine. Set one up in Settings` |
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
| No history and service not running | Same, with the 7.3 banner above |
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
  change, works whether or not `watch` is the parent.
- **Previous clipboard restored** after the paste (default on, setting in 10): after
  `paste::send` returns, wait 300 ms, then `copy_entry` the entry that was on the clipboard
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
`Reset to defaults` at the right. Escape or the back arrow returns to the list.

Sections in order of how often they are touched. Each control shows the current value; the
default is shown muted in the label's help text where it is not obvious from the control.

### History

| Label | Control | Default | Config key |
|---|---|---|---|
| Keep up to | number field, 10 to 100000, suffix `items` | `1000` | `max_items` |
| Delete all history | button, destructive style | | |

`Delete all history` opens an inline confirmation in place of the button:
`Delete all 1,000 items? This can't be undone.` with `Delete all` (destructive) and `Cancel`.
The count is real.

### Paste

| Label | Control | Default | Config key |
|---|---|---|---|
| Paste after picking an item | toggle | on | `paste.paste_on_select` |
| Paste after Paste as plain text (Super+Alt+V) | toggle | on | `paste.auto_paste` |
| Paste by pressing | dropdown: `Shift+Insert (works in most apps and terminals)`, `Ctrl+V`, `Ctrl+Shift+V` | Shift+Insert | `paste.keys` |
| Restore the previous clipboard after a macro | toggle | on | `macros.restore_clipboard` (new) |

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

Needed by the design; each is small.

1. `Summary` gains `last_used: i64` and `content_len: usize` (bytes for images, chars for
   text). Add to `SUMMARY_COLS`. Used for the hover tooltip and the `N chars` overflow hint.
2. Store a `line_count` for text entries at insert time (or compute from the preview plus
   `content_len`; exact count needs the full text). Cheap to store, so store it.
3. `Store::restore(id, last_used)` for undo, or keep a `deleted_at` column and hard-delete
   on cap enforcement. Prefer `deleted_at`: undo is then a single UPDATE and the blob never
   leaves the DB. Filter `deleted_at IS NULL` in `list`.
4. `Config` gets `Serialize`; add `[macros]` (`restore_clipboard`, `[[macros.items]]` with
   `format`, `label`). Writer uses `toml_edit`.
5. Socket protocol (`paste.rs::serve`) gains: `menu toggle`, `reload`, `status` (reply:
   `ocr=<engine|none> watching=1`), and a push line `ocr done <id>` to connected menu
   clients. This turns the socket into the one channel between service and window.
6. `ingest` reads the skip file (section 9) before upserting.
7. Text preview: keep the first 400 chars but do not trim leading whitespace inside lines;
   the window needs the real shape.
8. `menu::label` and its whitespace collapsing stay for `clippo list` output only.
9. Distinguish `Pending` with no engine from `Pending` in progress at the source: `ingest`
   already knows the engine kind; the window learns the real availability from `status`.

## 12. libcosmic notes and risks

- **Resident process**: strongly recommended (section 1). libcosmic's layer-shell support
  (`cosmic::iced::platform_specific::shell::commands::layer_surface`) can create and destroy
  surfaces on demand from a running app. This is the applet-popup pattern.
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

## 13. Open questions

| Question | Recommendation |
|---|---|
| Fixed 800×500, or larger on the 1440p monitor? | Fixed. Stable size beats 20% more rows. Revisit if the list feels cramped. |
| Should Enter on the top row (already on clipboard) still re-copy? | Yes. It is harmless and makes the behaviour uniform. |
| Macro paste restores the previous clipboard by default? | Yes, on. The macro is a one-off insert; losing the clipboard is the surprise. |
| Macros in history? | No. A timestamp in history is noise, and dedupe would not help since the value changes every second. |
| Shift+Enter or Ctrl+Enter for paste as text? | Shift+Enter (matches "paste as plain text" being Ctrl+Shift+V in most apps, so Shift is the plain-text modifier). Ctrl+Enter = copy only. |
| Group rows by day? | No. Recency order with hover time is enough for a clipboard. |
| Confirmation for single delete? | No, undo instead. Confirmation only for delete all and reset. |
| Show file size for images? | Only in the hover tooltip (`1.2 MB`). Dimensions are the useful number. |
| Should `clippo plain` (Super+Alt+V) move into this window? | Keep it as the separate shortcut; it is the fast path and does not need a window. |

## 14. User-visible strings

| Where | String | Note |
|---|---|---|
| Search field, placeholder | `Search history` | |
| Header, count | `1000 items` | number is live |
| Header, count while searching | `12 of 1000 match` | |
| Header, settings button tooltip | `Settings` | |
| List, caption above top row | `ON CLIPBOARD NOW` | small caps, muted |
| Image row, line 1 | `Image 1920×1080 · PNG` | format from mime, upper-case |
| Image row, no dimensions | `Image · PNG` | |
| Image row, OCR pending | `Reading text...` | with spinner |
| Image row, OCR pending, no engine | `Waiting for a text engine` | only with the engine banner |
| Image row, OCR done, empty | `No text found` | muted |
| Image row, OCR failed | `Couldn't read text` | muted, warning icon |
| Text row, overflow | `3 more lines · 412 chars` | either part may be absent |
| Text row, overflow, lines only | `9 more lines` | |
| Row hover tooltip | `Copied 3 min ago` | relative; `just now`, `N min ago`, `N h ago`, `Yesterday 14:00`, `12 Sep 14:00` |
| Row hover tooltip, image | `Copied 3 min ago · 1.2 MB` | |
| Row action, tooltip | `Paste as text` | |
| Row action, tooltip | `Delete` | |
| Row context menu | `Paste` / `Paste as text` / `Copy only` / `Delete` | |
| Macro column heading | `Paste` | |
| Macro button | `14:00` / `2026-09-17` / `2026-09-17 14:00:05` | live values |
| Macro button, accelerator | `Alt+1` | small |
| Macro tooltip | `%Y-%m-%d · Change in Settings` | |
| Footer, hints | `Enter paste · Shift+Enter paste as text · Del delete · Esc close` | |
| Footer, after delete | `Deleted. Undo (Ctrl+Z)` | `Undo` is a link button |
| Footer, waiting for OCR on Shift+Enter | `Reading text...` | |
| Footer, error | `No text in this image` | |
| Footer, error | `No text recognition engine. Set one up in Settings` | `Settings` is a link |
| Footer, error | `Couldn't read history: <error>` | |
| Footer, settings saved | `Saved` | |
| Footer, settings saved, no service | `Saved. Restart the clippo service to apply paste and OCR settings` | |
| Banner | `Not recording. Start the clippo service to save new copies` | |
| Banner | `Text in images is not being read. Set up an engine in Settings` | `Settings` is a link |
| Empty state, title | `Nothing copied yet` | |
| Empty state, body | `Text and images you copy will show up here` | |
| Empty search, title | `No matches for "foo"` | |
| Empty search, button | `Clear search` | |
| Notification, paste failed | title `Copied, but couldn't paste` body `<error>. Press Shift+Insert to paste it yourself.` | key name follows config |
| Notification, copy failed | title `Couldn't copy` body `<error>` | |
| Settings, header | `Settings` | |
| Settings, header button | `Reset to defaults` | |
| Settings, reset confirm | `Reset all settings to defaults?` + `Reset` / `Cancel` | |
| Settings, section | `History` | |
| Settings, History | `Keep up to` + `items` suffix | |
| Settings, History | `Delete all history` | destructive button |
| Settings, delete confirm | `Delete all 1,000 items? This can't be undone.` + `Delete all` / `Cancel` | count is live |
| Settings, section | `Paste` | |
| Settings, Paste | `Paste after picking an item` | |
| Settings, Paste | `Paste after Paste as plain text (Super+Alt+V)` | |
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
