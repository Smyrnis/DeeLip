# Pop-out window architecture

Sources: `crates/ui/src/helpers/pop_out_window.rs`, `crates/ui/src/app.rs`
(`SharedApp`), `crates/ui/src/views/settings/mod.rs`, `crates/ui/src/views/messages.rs`,
`crates/ui/src/views/dialer/transfer_window.rs`.

DeeLip opens five things as genuine separate native OS windows: Settings,
Messages, the Transfer Call panel, the DTMF Keypad, and the Contact dialog.
Four of them (all but Messages) share one helper, `helpers::show_pop_out_window`.
This doc is the "why" for the whole pattern; the individual call sites just
point back here.

## Why `Deferred`, not `Immediate`

Settings used to be an `egui::Window` -- a floating panel drawn *inside* the
main app's own OS window canvas, with a hand-rolled dimming backdrop faking
modality. It looked like a separate window but was mechanically trapped
inside the main window's bounds, unable to be dragged out to a different
part of the screen (a real user-reported bug: "the settings window is inside
the initial deelip window, and i can not move it"). `Context::show_viewport_deferred`
creates an actual second native window (its own OS-level title bar, move,
resize, close), which is what "a separate window" needs to mean.

The viewport class matters too. Settings was originally `Immediate`, which
renders synchronously nested inside the *main* window's own per-tick
callback (confirmed against `eframe`'s own source: an `Immediate` child
viewport has no redraw path of its own, it only ever repaints when its
parent's tick runs). That's what made Settings feel slow whenever the main
window was unfocused (which it always is while Settings itself has focus):
GNOME/Mutter throttles frame delivery for whichever of the two windows isn't
focused down to roughly 1Hz (confirmed live, independent of whether the
windows visually overlap), and since Settings was nested inside the main
window's own callback, every click/keystroke inside Settings was gated by
that same ~1s throttle on the *main* window's redraw.

`Deferred` viewports get their own independent redraw path -- `eframe`
invokes their stored callback directly whenever *their* window needs a
repaint, not the main window's -- so Settings now responds to its own input
normally regardless of the main window's focus/throttle state. This is also
why `DeelipApp` is wrapped in `SharedApp` (`Arc<Mutex<DeelipApp>>`): a
`Deferred` callback must be `Fn + Send + Sync + 'static`, so it can't
directly borrow `&mut self` the way the old `Immediate` closure did; it
locks the shared app instead. Each pop-out window is still called every
frame while its own `_open` flag is true -- egui's viewport model stays
declarative, not create-once-and-forget.

`app.rs`'s `ctx_slot` field exists for the same reason: every background
producer (SIP events, hotkeys, notification actions, the update checker,
LDAP search, device scans) calls `request_repaint()` through it the instant
it has something, instead of relying on a periodic forced repaint of the
whole window tree -- which, while Settings was open, meant repainting its
own viewport too and was the original source of the slowdown this whole
`Deferred` migration fixed.

## `SharedApp`'s `Send`/`Sync` soundness

`unsafe impl Send + Sync for SharedApp` is a borrow-checker/orphan-rule
necessity, not a real concurrency mechanism. `eframe`'s winit event loop is
single-threaded, and a `Deferred` viewport's callback is only ever invoked
as a separate, sequential event on that same thread (confirmed against
`eframe` 0.28.1's native/{glow,wgpu}_integration.rs) -- never reentrantly,
never nested inside another locked call to `update()`. `DeelipApp` itself is
`!Send` only because it transitively holds a `cpal::Stream`, which `cpal`
marks `!Send` defensively for genuine cross-thread use it never sees here.

## The shared `show_pop_out_window` helper

Every one of the four windows built on it needs the same ~35-line skeleton:
check `ctx.embed_viewports()` up front and render a fallback in-canvas
`egui::Window` directly against `app` if the backend can't open a real
second native window, otherwise open a genuine `Deferred` viewport with a
titlebar (a plain heading-styled label -- no in-app Close button, removed
once real window decorations already provided one) and wire up
`close_requested()` to whatever that window's own close action is.

The `embed_viewports()` check has to happen up front, in the synchronous
part of the function, rather than being branched on from inside the
deferred closure: on a backend that embeds, `show_viewport_deferred`'s
closure runs *synchronously*, right there -- if it tried to lock `self_app`
in that case, it would deadlock against the lock the caller of
`show_pop_out_window` already holds.

`is_open`/`on_close`/`title` are plain `fn` pointers rather than general
closures -- every real call site's version is already a non-capturing
closure (e.g. `|app| app.settings_open`, or Transfer Call's two-field
`|app| app.showing_transfer || app.showing_attended`), which Rust coerces to
`fn` for free, so there's no need for `Clone + Send + Sync` bounds just to
store one. `content` stays a real closure since it's the one genuinely
different piece of code per window -- bound as `Fn`, not `FnMut`:
`show_viewport_deferred` itself requires the outer closure to be
`Fn + Send + Sync` (it may be invoked repeatedly through a shared
reference), so a `content` that needed its *own* captured state to mutate
across calls wouldn't fit without interior mutability. None of this app's
actual pop-out windows need that -- `content` always just forwards to a
method on the `app`/`ui` it's handed, no captured state of its own.

The content panel's 14px inner margin is universal across every pop-out
window, not just Settings -- confirmed live that `CentralPanel::default()`'s
bare default left content rendering flush against the window's right edge
with zero breathing room; applying it everywhere preempts the same class of
bug recurring in one of the others later.

## Why Transfer Call is one window, not two

Blind and attended transfer share a single pop-out window with a mode
switch, rather than two near-identical windows, because they're one
workflow, not two unrelated features. `do_transfer`/`do_attended_transfer_dial`
already flip `showing_transfer`/`showing_attended` back to `false` on
success, which is also this window's open condition -- so firing either one
closes the window as a side effect, no separate "close" bookkeeping needed
for the happy path.

## Why Messages is the one exception

Messages doesn't build on `show_pop_out_window`. Its content is a
`SidePanel` (peer list) *and* a `CentralPanel` (thread+compose) side by
side, not one panel. The shared helper's content closure is `Ui`-shaped so
it can run inside both the `embed_viewports()` fallback's `egui::Window` and
the real deferred branch's `CentralPanel` -- but a `SidePanel` attaches to a
viewport's `Context`, not to an arbitrary parent `Ui`/`Area`, so it can't be
built from inside that shared closure. Forcing Messages into that shape
would need a second, `Context`-shaped content parameter used by nobody
else -- not worth it for the one exception. Messages also has no tab-bar
entry point at all: the only way `messages_window_open` becomes `true` is
`message_from_list` (a right-click "Message" action on a History/Contacts/
Directory row).
