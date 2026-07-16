# ui (`crates/ui`)

The `eframe`/`egui` desktop UI: one `DeelipApp` struct holding all session state,
rendered every frame from `frame.rs`, with per-tab/per-window content split across
`views/`. Talks to `deelip-sip` (call control), `deelip-media` (audio/video engines),
and `deelip-config` (settings/contacts/history/messages persistence) — this crate owns
no protocol or media logic itself, just presentation, input handling, and the
background-thread plumbing needed to keep both off the render thread.

## Architecture

**Entry point**: `lib.rs` re-exports `DeelipApp`/`SharedApp` and `install_fonts`
(embeds JetBrains Mono + the Phosphor icon font, replacing every egui default; see
"Theming" below for why there's no separate heading typeface).

**State**: `app.rs::DeelipApp` used to be one flat struct of ~75 fields — twice declined
for a split in earlier rounds as "touches nearly every file for a readability-only
gain" — since split into 10 grouped sub-structs, one per concern: `AccountsState`
(account/registration), `CallsState` (the live-call/dialer state machine), `NotifyState`
(ringtone/notifications), `SettingsUiState` (Settings' live-edited draft), `TrayState`
(tray badge), `HistoryState` (call history), `MessagesState`, `ContactsState`,
`UpdateCheckState`, `DirectoryUiState` (LDAP search). `DeelipApp` itself now just holds
one field per group (`accounts_state`, `calls_state`, etc.) plus whatever doesn't fit
any group (tab selection, fonts, etc.) — see "The `DeelipApp` struct split" below for
why this shape and not, say, one shared `Rc<RefCell<...>>`-per-group or further
splitting `DeelipApp` itself into multiple top-level structs. `SharedApp
(Arc<Mutex<DeelipApp>>)` wraps the whole thing so Settings/Messages/etc. can render as
real separate OS windows; see "Pop-out windows" below for why that needs an `unsafe impl
Send`.

**Frame loop** (`frame.rs`): `eframe::App::update` for `SharedApp` locks and delegates
to `DeelipApp::update_inner`, which each frame: refreshes `ctx_slot` (see below), drains
every event source (SIP events, tray, hotkeys, notification actions, update checker,
directory search), applies the theme, renders the tab bar + status bar +
`CentralPanel` for the active tab, renders every pop-out window (each a no-op when
closed), and finally calls `request_repaint_after` — 50ms while any call is
live/ringing/dialing (drives the ringing-dot pulse and the call timer), 2s otherwise as
a rare safety net (see "Repaint plumbing" below for why almost nothing actually depends
on this idle tick anymore).

**Event/action split**: `event_handling.rs` reacts to `SipEvent`s from `sip-core`
(registration, call state transitions, presence/MWI, incoming messages) and owns the
call-history/status-line bookkeeping that follows from them. `call_actions.rs` is the
other direction — user-initiated actions (dial, accept/reject, hold/swap, transfer,
DTMF, mute/record, gain) that call into `SipHandle`/`MediaEngine`. `media.rs` bridges
the two: `start_media`/`start_video`/`start_conference` build the actual
`MediaEngine`/`VideoEngine` once a call's SDP negotiation has already resolved codec/
SRTP/ICE (all of which happens in `sip-core`, not here). `start_conference` merges the
two connected calls' media locally with no new SIP signaling, but a held leg was put on
hold with `a=sendonly` (telling the far end to stop sending audio), so it first sends a
real resume re-INVITE (`a=sendrecv`) for any held leg before tearing down and rebuilding
the engine in conference mode — otherwise that leg would come through silent even
though it's now being mixed in locally. That resume is fire-and-forget like hold/resume
elsewhere in this codebase, but this one call site adds a brief (300ms) grace sleep
first, since it's about to tear down the single-leg engine right after and needs to
give the far end a moment to actually process the re-INVITE and resume sending — same
precedent as `frame.rs::hangup_before_exit`'s own post-BYE grace sleep, including the
same requirement that the sleep be constructed inside the `block_on`-driven `async`
block rather than as a bare argument (a bare `tokio::time::sleep(...)` expression would
be evaluated before `block_on` enters the runtime context, and registering a timer with
no ambient runtime context panics).

**Views** (`views/`): `contacts.rs`, `history.rs`, `messages.rs`, `directory.rs`
(LDAP search), `dialer/{idle,in_call,transfer_window,dtmf_window}.rs`, and
`settings/{general,account,audio,video,network,directory,hotkeys,advanced}.rs` — each a
`impl DeelipApp` block rendering one tab or pop-out window's content. Multi-file splits
(`dialer/`, `settings/`) are purely organizational, mirroring `sip-core`'s
`call/lifecycle/` precedent: cross-file inherent-method calls work regardless of which
file defines the method, so e.g. `settings/mod.rs`'s tab dispatch calling
`self.show_account_section(...)` (defined in `settings/account.rs`) needs no special
wiring.

**Helpers** (`helpers/`): `dial_target.rs` (bare-number-to-SIP-URI normalization, dial
plan application), `format.rs` (call-status/URI/duration/timestamp formatting, shared
CSV escaping), `widgets.rs` (list rows, search fields, the phone keypad, device
pickers, avatars — the shared building blocks most views compose from), `pop_out_window.rs`
(see below). All re-exported flat through `helpers/mod.rs` so call sites don't need to
know which file actually defines a given helper.

**Platform** (`platform/`): `tray.rs` (system tray + hide-to-tray), `hotkeys.rs`
(global Answer/Hangup/Mute bindings), `notify.rs` (desktop notifications with Accept/
Reject actions), `ringtone.rs` (ring cadence, real or synthesized). Each owns its own
background OS-integration thread(s) — see "Background-thread pattern" below for the
one idiom all four (plus the update checker and directory search) share.

**`update.rs`**: startup GitHub-release check (`deelip-updater`), the small popup
offering "Update Now"/"Skip"/"Later", and (if `auto_update_enabled`) the automatic
download-and-relaunch flow. Same background-thread-plus-channel idiom as everything
else backed by blocking I/O.

## Design decisions & invariants

### The `DeelipApp` struct split

Ten field groups, one commit per group, migration order **smallest fan-out first**
(`DirectoryUiState` first, `CallsState` last — deliberately: prove the mechanical
pattern on a group only 1-2 files touch before attempting the one that touches
almost every file in the crate). Each commit was verified with `cargo check/
clippy/build` immediately after editing, not batched at the end — this caught
several real multi-line `self\n.field` chains a blind `sed` pass missed, and one
genuine downstream type-inference cascade from a single missed field rename.

**Why grouped sub-structs on the same `DeelipApp`, not a deeper split into several
independent top-level structs**: plenty of methods here (`process_sip_events`,
`do_call`/`do_hangup`, the frame-loop's per-tab dispatch) legitimately need fields
from several groups at once — a real multi-struct split would just turn every one
of those into a multi-argument function threading 3-4 structs through, trading a
big struct for a long parameter list, not a real reduction in coupling. Grouping
by concern keeps `DeelipApp`/`SharedApp` as the one thing every view/handler
already takes `&mut self` on, while shrinking what any *one* piece of code has to
reason about — e.g. `views/settings/*.rs` only ever touches `self.settings_ui`
(plus whichever data it's editing), not 70-odd unrelated fields interleaved with
it.

A full end-to-end live GUI smoke test (two-instance Xvfb) after all ten groups
landed confirmed dial/hold/resume/hangup, History recording, Contacts add/search/
save, and Settings all still work post-refactor.

### Repaint plumbing: `ctx_slot`

Every background producer (SIP events, tray clicks, global hotkeys, notification
actions, the update checker, LDAP search, Settings' audio/camera device scans) needs a
way to wake the UI the instant it has something, rather than the idle tick discovering
it late. `app.rs`'s `ctx_slot: Arc<Mutex<Option<egui::Context>>>` is refreshed
unconditionally every frame in `update_inner`, and each background thread calls
`ctx.request_repaint_of(...)` through it right before finishing. This is why `frame.rs`'s
idle repaint interval (2s) is safe to leave long: it used to be the *primary* way
anything got noticed, and forcing it short while Settings was open caused a real,
diagnosed slowdown (GNOME/Mutter throttles frame delivery for whichever of DeeLip's two
windows isn't focused, and both share one render thread — see "Pop-out windows" below).
Now the idle tick is a rare safety net; only the ringing-dot pulse and the in-call timer
have no waker of their own and genuinely depend on the 50ms fast path.

### Pop-out windows: why `Deferred`, not `Immediate`

DeeLip opens five things as genuine separate native OS windows: Settings, Messages, the
Transfer Call panel, the DTMF Keypad, and the Contact dialog. Settings used to be an
`egui::Window` floating inside the main window's own canvas — mechanically trapped
there, unable to be dragged to a different part of the screen (a real user-reported
bug). `Context::show_viewport_deferred` opens an actual second native window; the
`Deferred` viewport *class* matters too, not just the window itself — an `Immediate`
child viewport has no redraw path of its own and only repaints when its parent's tick
runs, which is what made Settings feel slow whenever the main window was unfocused
(confirmed live: GNOME/Mutter throttles an unfocused window's frame delivery to ~1Hz,
independent of visual overlap). `Deferred` viewports get their own independent redraw
path, invoked directly whenever *their* window needs a repaint.

This is also why `DeelipApp` is wrapped in `SharedApp` (`Arc<Mutex<DeelipApp>>`): a
`Deferred` callback must be `Fn + Send + Sync + 'static`, so it locks the shared app
instead of directly borrowing `&mut self`. `unsafe impl Send + Sync for SharedApp` is a
borrow-checker/orphan-rule necessity, not a real concurrency mechanism — `eframe`'s
winit event loop is single-threaded, and a `Deferred` viewport's callback only ever
runs as a separate, sequential event on that same thread (confirmed against `eframe`
0.28.1's own source), never reentrantly. `DeelipApp` itself is `!Send` only because it
transitively holds a `cpal::Stream`, which `cpal` marks `!Send` defensively for
cross-thread use it never sees here.

**Non-obvious closure-capture pitfall** (worth remembering if this pattern is extended):
`SharedApp::lock(&self)` is a real method, not a bare `.0.lock()` at each call site,
specifically so a `move` closure calling it captures the *whole* `SharedApp` (carrying
the unsafe impl) rather than reaching straight through to the inner `!Send`
`Arc<Mutex<DeelipApp>>` field — Rust's 2021 disjoint-closure-capture would otherwise
capture just that field and silently miss the wrapper's soundness argument.

**The shared `helpers::show_pop_out_window`** (`pop_out_window.rs`) covers four of the
five (Settings, Transfer, DTMF Keypad, Contact dialog): check `ctx.embed_viewports()`
up front (must happen synchronously, before any deferred closure runs — on an embedding
backend the closure itself runs synchronously, and locking `self_app` there would
deadlock against the lock the caller already holds) and fall back to an in-canvas
`egui::Window` if the backend can't open a second native window, otherwise open a real
`Deferred` viewport with a plain titlebar and a 14px-margin `CentralPanel` (confirmed
live that egui's own default left content flush against the window edge). `is_open`/
`on_close`/`title` are plain `fn` pointers (every real call site is a non-capturing
closure, which Rust coerces to `fn` for free) rather than general closures, avoiding
`Clone + Send + Sync` bounds that aren't otherwise needed.

**Why Transfer is one window, not two**: blind and attended transfer share a single
pop-out with a mode switch rather than two near-identical windows, since they're one
workflow. `do_transfer`/`do_attended_transfer_dial` already flip their own `showing_*`
flag back to `false` on success — which is also this window's open condition — so
firing either closes the window as a side effect.

**Why Messages is the one exception**: its content is a `SidePanel` (peer list) beside
a `CentralPanel` (thread+compose), not one panel — `show_pop_out_window`'s content
closure is `Ui`-shaped so it can run inside both the embedded fallback's `egui::Window`
and the real `CentralPanel`, but a `SidePanel` attaches to a viewport's `Context`
directly, not an arbitrary parent `Ui`. Forcing Messages into that shape would need a
second, `Context`-shaped parameter used by nobody else. Messages also has no tab-bar
entry point at all — the only way `messages_window_open` becomes `true` is
`message_from_list` (a right-click "Message" action elsewhere).

**The Contact create/edit dialog is rendered from `frame.rs::update()`**, not from
inside `show_contacts`, for the same cross-tab-trigger reason: History's own
right-click "Add to Contacts" action needs to open it while History, not Contacts, is
the active tab. `on_close` and the content closure both end up calling
`finish_contact_dialog`, harmlessly even when neither Save nor Cancel was actually
clicked (e.g. a plain window-close).

### Theming

One design system (`theme.rs::Palette`) plus the JetBrains-Mono-everywhere type scale,
instead of ad hoc `Color32` literals and egui's default font. The one rule every view
follows: **color communicates call state, not decoration** — `signal` means
active/connected/positive, `ringing` means pending/incoming/hold, `danger` means
destructive; nothing else borrows them. Interactive chrome (buttons, tabs, selection
highlight) deliberately uses neutral `surface`/`surface_hover` grey instead, a
correction made after an earlier pass reused `signal`'s blue as general chrome and it
read as "too much blue." `link` exists solely for `Visuals::hyperlink_color`, kept
separate from `signal` so blue can't quietly leak back into chrome even though nothing
in-app currently renders a hyperlink.

The palette itself has gone through several real revisions chasing user feedback (a
spacious/indigo "Signal" redesign → Darcula IDE colors, sourced from real IntelliJ
Darcula hex values → the current IntelliJ Light palette, sourced from JetBrains' own
`expUI_light.theme.json`) — currently light-only, single-theme, no toggle. `ink`
(`#000000`) is the closest sourced value for a global foreground the source theme file
doesn't explicitly name; disclosed as such rather than presented as a certain quote.

**Known broken icon glyphs**: the bundled `egui-phosphor` 0.6.0 "Regular" variant has
several codepoints (`INFO`, `BACKSPACE`, `ARROW_BEND_UP_RIGHT`, `ARROW_DOWN_LEFT`,
`ARROW_UP_RIGHT`, `DOWNLOAD`, `DOWNLOAD_SIMPLE`, `FILE_ARROW_DOWN`, `FLOPPY_DISK`,
`ARROW_DOWN`) whose cmap resolves to the wrong (not missing, just wrong) glyph —
discovered by rendering every icon constant this app uses at a large size and actually
looking at the shape. Confirmed fine: `EXPORT`, `UPLOAD_SIMPLE`, `ARROW_SQUARE_OUT`.
This isn't limited to Phosphor either: a plain Unicode "☰" was separately found
silently rendering as "?" in this app's font stack, and even a previously-confirmed-fine
plain-Unicode workaround ("↱") was later caught rendering as "?" in a *different*,
smaller/differently-weighted spot than where it was first checked. **Standing rule for
any future icon-ish glyph in this app, Phosphor or not**: render it large in the actual
context it'll be used and look at it — don't assume a glyph verified once elsewhere
still renders correctly everywhere.

**A few narrower widget-level color/layout fixes worth remembering**, all in
`helpers/widgets.rs`: `avatar_color` hashes a contact/peer's name+URI across a short
fixed set of Darcula-adjacent hues (the app's own `signal`/`ringing` colors plus
Darcula's own class-name purple and string green) rather than an arbitrary rainbow, so
avatar variety doesn't read as an unrelated bolt-on to the rest of the palette.
`field_label` (a Settings field's own "Account name:"/"Username:" caption) is muted
(`ink_muted`) rather than plain `ink` — without that, both the label and a `TextEdit`'s
typed content fell back to the same `override_text_color` (see `apply_style` above)
and rendered in the literal same color, making them hard to tell apart at a glance.
`text_edit_scope` scopes `visuals.selection.bg_fill` to `palette.link` (blue) just for
a text field's own selected-range highlight, since `apply_style` otherwise points that
same `Visuals` field at `surface_hover` grey for the tab-bar/list "selected" chrome (the
v3.1 "grey chrome" decision above) — `ui.scope` restores the prior value once the field
is done, so there's no separate reset to keep in sync by hand. `styled_slider` scopes
`visuals.widgets.inactive.bg_fill` to `palette.border` for just a slider: this theme
sets that field to `palette.surface` (plain white) so ordinary buttons/comboboxes read
as flat chrome, which otherwise left every slider's rail invisible against a
`surface`-colored card (a bare handle, no visible track). And `theme::full_width_card`
takes `Palette` by value, not `&Palette` — it's `Copy`, and call sites that also read
`self` inside their `add_contents` closure (most of them) would otherwise hit a borrow
conflict between `&self.palette` and the closure capturing `self` mutably, since both
are evaluated as part of the same call.

### The dialer in-call screen

**The status-dot redesign** (`in_call.rs::RingState`): `call_avatar`/`state_badge`
render `Pending` (ringing/dialing/hold) as a small avatar with a softly pulsing amber
corner dot, and `Connected` as the same avatar with a static `signal`-colored dot. This
replaced an earlier animated dual-ring pulse (concentric circles expanding around the
avatar) that user feedback called "too playful — a big bouncing shape, not a serious
instrument." The pulse still animates for `Pending` (a slow opacity fade via
`ui.input(|i| i.time)`, no separate `request_repaint` — `frame.rs`'s own 50ms cadence
during a live call already redraws it often enough).

**A real cross-platform layout bug, worth remembering for any future icon+caption
button**: `icon_toggle_button` (Mute/Record/Transfer/Keypad) is built from raw
`ui.painter()` calls on one `ui.allocate_exact_size` rect, not `egui::Button` plus a
layout container — two container approaches (`vertical_centered`, then
`allocate_ui_with_layout`) were tried first and both broke on a real desktop (never
reproduced in this project's own Xvfb sandbox): `horizontal`'s default cross-axis
alignment is `Center`, so if any one column's *measured content height* differs (e.g. a
caption wrapping to 2 lines under different font metrics than Xvfb's), that whole
column's contents shift relative to the others. Painting at fixed offsets within one
rect leaves no content-dependent height to differ by, on any font stack.

**Centering nested rows**: `ui.vertical_centered`/a bare `ui.horizontal` only centers a
single fixed-size child — a nested `ui.horizontal` row reports its own `min_rect`
starting flush at the parent's left edge, so it never gets centered by the outer layout.
Every centered button row/keypad in this app (the idle dial pad's `STAGE_WIDTH` margin,
`phone_keypad`'s per-row centering, the in-call action-button row, the slider row) works
around this the same way: compute the row's own known width and add an explicit leading
`ui.add_space` sized to center it, rather than trusting the parent layout.

**Video panel**: reads the latest camera/decoded frames via a short immutable borrow of
`self.video`, updates each side's cached egui texture only if the frame actually
changed (a separate short mutable borrow — can't hold both in one closure), then draws
from a final immutable borrow. Avoids re-uploading an unchanged GPU texture every
repaint, since egui repaints far faster than either the camera or the decode framerate.

### Settings

A tabbed dialog (`views/settings/mod.rs`) — one section visible at a time,
sized to fit without scrolling, replacing an earlier single long scrolling stack of 12
sections grouped down to 8 tabs. The Save button's `TopBottomPanel::bottom` is anchored
*before* the tab-content match, not after — `ScrollArea::vertical()` (used by the
Account and Advanced tabs, the two exceptions below) greedily claims all remaining
space in its parent, and a naive "content, then Save" ordering silently pushed the Save
button below the visible window whenever a tab's content scrolled (caught live: it was
simply missing from a screenshot, not visibly clipped).

**Two tabs that scroll internally, by necessity not preference**: Account (confirmed
live that its content doesn't fit even at ~1400px window height, an unreasonable
size) and Advanced (its 4 stacked sections — Updates/Blocklist/Call History export/
Contacts import-export — overflow past the window bottom). Every other tab fits at the
window's 950×740 default, confirmed live across all 8 tabs, not assumed.

**Audio/camera device enumeration runs on a background thread**, not inline in the
section render: measured ~620ms on first Audio-tab visit via PulseAudio, which froze
the *whole app* (both windows share one render thread) for that long. The scan's
completion callback wakes both `ROOT` and the Settings viewport by name
(`SETTINGS_VIEWPORT_NAME`) specifically — waking `ROOT` alone doesn't repaint a
`Deferred` child viewport, which left the "Scanning..." label stuck showing stale text
until the user happened to interact with the window directly (a real bug, caught live).
The result is also cached indefinitely once scanned (populated lazily on first render,
refreshed only via an explicit button), not just moved off-thread — calling cpal's
enumeration every frame (egui repaints continuously) was separately measured hammering
every ALSA/jack backend dozens of times a second, producing log spam and a real
slowdown of its own even after the background-thread fix. ALSA's multi-channel
surround (`surround21`/`surround40`/...) and digital-passthrough (`iec958`/`spdif`)
pseudo-devices are filtered out of the picker lists — real cpal enumerations, never a
sensible mic/speaker choice, with jargon-heavy names meaningless to a non-technical
user.

**The account editor's per-section files** (`views/settings/account/{identity,codecs,
call_handling,dial_plan,network}.rs`) are free functions taking `&mut SipAccount`
directly, not `impl DeelipApp` methods like this crate's other multi-file splits
(`dialer/`, `settings/mod.rs`'s own per-tab dispatch) — `account` is borrowed from
`self.config.accounts[idx]` for the whole body of `show_account_section`'s card
closure, and a `self.method(...)` call from inside it would need to re-borrow all of
`self` while that borrow is still live. Passing exactly the pieces each section needs
(the account, the palette, an `edited` out-flag, and the odd extra like `network.rs`'s
`global_ice_enabled` read before `account` was borrowed) as explicit parameters
sidesteps the conflict entirely.

### List views (History/Contacts/Messages)

Share a common row idiom (`helpers/widgets.rs::list_row`/`list_row_menu`): a hover
highlight painted via egui's "reserve a shape slot, fill it in once the row's rect/
hover state are known" trick, plus a bottom divider — both need the row to be a single
widget (one `ui.horizontal`), not a widget-plus-separate-separator, since a second
sibling widget's auto-inserted `item_spacing.y` gap breaks the fixed-row-height math
`ScrollArea::show_rows` (History's virtualization) needs. Each row is wrapped in
`ui.push_id(row_index, ...)` — without it, every row from the same virtualized loop
gets the *same* egui id (derived from the parent id, not anything row-specific), so
hovering one row would light up every row's highlight simultaneously.

History additionally caches its filtered-index list (`history_filter_key`) and its
tab-bar unseen-count label, recomputing only when the search text/status filter/record
count (or unseen count) actually changed — both used to rebuild from scratch every
single frame regardless, at continuous ~20fps, which was the real cause of a reported
scroll-lag bug. `ScrollArea::show_rows` (used for History specifically, up to 200
records) only lays out the rows actually scrolled into view instead of all of them
every frame — without it, the plain `show` form was doing thousands of unnecessary
widget layouts/sec, which combined with the two-widgets-per-row `item_spacing.y` gap
above was the actual cause of a reported scroll-jitter bug, not raw row count.

The same cache-and-compare-key idiom recurs elsewhere in `DeelipApp`'s state:
`ContactsState::contact_filter_key` mirrors `history_filter_key` exactly, including
needing an explicit invalidation on an in-place edit (same length, so the cache key
alone can't detect it); `TrayState::tray_calls_key`/`tray_pending_key` let
`process_tray_events` skip re-cloning `Sender`s/call-ids and re-locking `QuitState`
every frame when the live/pending call set hasn't actually changed; and Settings'
`audio_device_cache`/`camera_device_cache` (see "Settings" above) apply it to a
one-shot background scan's result rather than a per-frame computation.

Contacts' "+" FAB is a floating `egui::Area` anchored with `fixed_pos` to the tab's own
`ui.max_rect()`, not `Area::anchor` — `anchor` is window-relative and, tried first, put
the button overlapping the bottom status bar; `fixed_pos` against the content rect
scopes it to the tab instead. Row name labels use `helpers::double_clickable_label`
rather than sensing clicks on the whole row: egui's hit-testing gives the *last*-added
widget at a position priority, and upgrading the entire row to `Sense::click()` would
compete with the row's own trailing quick-action buttons (added after the label) for
clicks — scoping `Sense::click()` to just the non-overlapping name label sidesteps that
entirely.

### Platform integration (tray/hotkeys/notifications)

`platform::tray`/`hotkeys`/`notify` each wrap a mechanism that needs its own event
loop independent of egui's: tray-icon clicks need *some* OS-level event loop pumping
the thread that owns the icon, global hotkeys need `global-hotkey`'s own event
thread, and desktop-notification action buttons block synchronously on
`notify-rust`'s `wait_for_action`. All three (plus `update.rs`'s release check and
`views/directory.rs`'s LDAP search) share one idiom: spawn a dedicated background
thread that owns the blocking/foreign-event-loop call, forward whatever it produces
through a channel, and call `ctx.request_repaint_of(...)`/`request_repaint()` through
`ctx_slot` the instant something's ready — the polling side (`process_*_events`,
called once per frame) just drains the channel, never blocks.

The startup account-registration task (`main()`'s pre-window `rt.spawn`, reporting back
through `AccountSpawnMsg`/`process_account_spawn_events`) follows the same shape for the
same reason: registering every enabled account inline used to block the window from
ever appearing at all, since an unreachable server's DNS/TCP/TLS connect can hang
indefinitely. `AccountSpawnMsg::Done` marks the end of the batch (every enabled account
attempted, success or not) so `refresh_idle_status` can stop showing "connecting" once
nothing more is coming; newly-spawned accounts land in `AccountsState::accounts` in
whatever order they finish in, not `config.accounts`' original order, which is safe
only because every existing `accounts[i]` access already derives `i` from `accounts`'
own current position, never from config order.

**Why tray clicks can't just be polled from inside `update()`**: eframe/winit pause the
render/update loop while the window is hidden (a normal optimization) — but "restore"
and "quit" are exactly the two actions you need *while* hidden. The tray's event
threads run independently of whether any frame is being drawn, and `egui::Context` is
thread-safe by design specifically for this. On Linux, hiding/restoring uses
`ViewportCommand::Visible`, not `Minimized`: window mapping is baseline ICCCM behavior
every X11 window manager gets right, whereas GNOME Shell/Mutter's handling of the
WM-level iconify state for an XWayland-forced client is unreliable and could leave
"Show DeeLip" doing nothing.

**Tray construction is genuinely per-OS** (`platform/tray.rs`), because `tray-icon`
itself has different requirements per platform for *when*, and on which thread, the
icon can be built:
- **Linux**: `MenuItem`/`Menu`/`TrayIcon` use `Rc` internally, so they're built *on* a
  dedicated spawned GTK thread, not constructed elsewhere and moved in —
  `spawn_tray_icon` blocks briefly on a one-shot channel round-trip until the menu
  items exist so their ids can be returned to the caller. The missed-call badge
  overlay updates through a separate `glib::MainContext` channel for the same reason —
  `gtk::glib::Sender` is the officially-supported way to hand work to a GTK main loop
  running on a different thread from the sender, the same category of problem
  `MenuItem`/`Menu` needing to be *constructed* on the GTK thread already solved for
  menu setup, just for an ongoing update instead of a one-time handoff (attached before
  `gtk::main()` starts).
- **Windows/macOS**: `tray-icon`'s own docs require the tray be created only once an
  OS event loop is already running on the thread that will pump it — the opposite of
  Linux's "build it before anything else starts" approach, and incompatible with
  building it eagerly in `main.rs` before `eframe::run_native`. Instead,
  `frame.rs::init_lazy_tray` builds it lazily on `DeelipApp`'s first real frame (by
  which point eframe's winit loop is definitely already running on this thread), with
  no dedicated thread of its own — the badge overlay is instead polled once per frame
  (`tray::poll_native_tray_badge`, called from `process_tray_events`) rather than
  attached to a GTK-style running main loop. Written and cross-compile-checked
  without access to real Windows/macOS hardware, so unverified at runtime — `init_lazy_tray`
  therefore calls `spawn_tray_icon()` through `catch_unwind`: a panic from some
  platform-specific condition this sandbox can never exercise would otherwise take the
  whole process down silently (no window ever shown) on the app's very first frame,
  instead of just leaving the tray unavailable like the existing `Err` arm does.

**Global hotkeys**: Linux support in `global-hotkey` is X11-only, which is fine since
`main.rs` already forces the main window onto X11/XWayland for the tray-restore
reasons above (that forcing is itself `cfg(target_os = "linux")`-gated — Windows/macOS
have no Wayland/X11 split to work around). Unlike the tray, this crate's backend owns
its own dedicated connection/event-loop thread internally on every platform, so it has
no GTK-style setup-ordering constraint. A hardware headset/multimedia hook button
(`Code::MediaPlayPause`) is registered as a fourth, independent binding interpreted as
"Answer if ringing, else Hangup" — a real phone's hook-switch behavior.

### i18n

See `docs/crates/i18n.md`.

## Known limitations / open items

- Settings' Account/Advanced tabs still require internal scrolling; not pursued further
  since splitting them again would just move the density problem, not remove it.
- The broken-icon-glyph list above is almost certainly incomplete — only glyphs this
  app actually uses have been checked, not the full Phosphor Regular set.
