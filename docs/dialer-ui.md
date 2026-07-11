# Dialer in-call UI: design history and a real layout bug

Source: `crates/ui/src/views/dialer/in_call.rs`

## The status-dot redesign (`RingState`)

`call_avatar`/`state_badge` render one of two states: `Pending`
(ringing/dialing/hold) gets a softly pulsing amber status dot since it wants
attention; `Connected` settles to a static `signal`-colored dot, since a
live call is a stable state, not an urgent one.

**v2 note**: the original pass used a large animated dual-ring pulse
(concentric circles expanding outward around the avatar) as the app's
signature element. User feedback on that first pass was that it read as too
playful -- a big bouncing shape, not a serious instrument. The replacement
is a small static avatar with a corner status dot (the same "live status"
convention Slack/Stripe/Notion use) plus a separate text badge, not a hero
animation. The dot still animates for `Pending` (a slow opacity fade, not a
bounce), reusing the app's existing ~20fps repaint cadence
(`frame.rs`'s `request_repaint_after`) as its clock rather than requesting
its own.

## The `icon_toggle_button` box-position bug

`icon_toggle_button` renders the secondary in-call actions (Mute, Record,
Transfer, Keypad) -- a smaller icon-only rounded-square button with a small
caption underneath, same icon+caption idiom `phone_keypad` uses for its
digit+letters.

It's deliberately built from raw `ui.painter()` calls on one
`ui.allocate_exact_size` rect, not `egui::Button` plus a layout container.
Two layout-container approaches were tried first (`vertical_centered`, then
`allocate_ui_with_layout`) and both were wrong in different ways.

Live testing on a real desktop (not just this project's own Xvfb sandbox,
which never reproduced it) showed the *whole button box* for Mute sitting
visibly higher than Record/Xfer/Keypad's. Root cause: those were built as 4
separate `ui.allocate_ui_with_layout(_, Layout::top_down(Align::Center), ...)`
calls inside one `ui.horizontal` -- `horizontal`'s default cross-axis
alignment is `Align::Center`, so if any one column's *measured content
height* differs (e.g. "Xfer"'s caption already needed shortening from
"Transfer" because it wraps to 2 lines in a 48px-wide slot -- a
content-height difference exactly like this can happen, just gated on exact
font metrics that apparently differ between this sandbox's font stack and a
real desktop's), that column gets re-centered against the row's shared
center line, shifting its whole contents up/down relative to the others.

Painting everything at fixed offsets within one `allocate_exact_size` rect
leaves no content-dependent height for any column to differ by, on any font
stack -- which is why the current implementation is immune to this class of
bug regardless of what any particular caption/glyph measures out to.
