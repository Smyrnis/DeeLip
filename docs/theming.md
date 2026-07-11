# Theming

Source: `crates/ui/src/theme.rs`

DeeLip's design system is named semantic color tokens (`Palette`) plus the
JetBrains-Mono-everywhere type scale (see `lib.rs::install_fonts`), instead of
ad hoc `Color32` literals and whatever font egui ships by default.

The one rule every view follows: color communicates call *state*, not
decoration. `signal` means active/connected/positive, `ringing` means
pending/incoming/hold, `danger` means destructive -- nothing else borrows
them. Everything else is drawn from the neutral canvas/surface/border/ink
scale.

## Palette history

**v3 revision (2026-07-09)**: pulled back from the "Signal" redesign's
spacious/rounded/indigo look toward a Darcula-style IDE identity, per user
feedback that the app felt "too modern." Real IntelliJ Darcula hex values
(not an approximation): canvas `#2B2B2B`, surface `#3C3F41`, ink `#A9B7C6`
(Darcula's own iconic foreground), ringing-orange `#CC7832` (Darcula's own
keyword orange), danger-red `#BC3F3C` (Darcula's error red). Darcula is a
fixed dark identity in real IntelliJ -- there's no official light
counterpart, so unlike the previous `dark()`/`light()` pair, this was
deliberately single-theme (disclosed and accepted when the redesign mockup
was approved). Rounding also dropped to near-zero (sharp IDE-panel corners,
not the previous rounded cards) -- see `apply_style`/`card_frame`.

**v3.1 (2026-07-10)**: first live use of v3 turned up real feedback -- the
bright sky-blue `#6897BB` (Darcula's *numeric-literal* text color) read as
too saturated/"modern" once it was reused as general interactive chrome
(tab-bar selection, the Contacts FAB) rather than just text. `signal` became
Darcula's string-green `#6A8759` instead -- same semantic role
(active/connected/positive, per the rule above), just a color that doesn't
read as "blue everywhere." Interactive *chrome* (tab-bar/list selection
highlight, the Contacts FAB, the Dialer's main "Call" button) moved off
`signal` entirely onto neutral `surface`/`surface_hover` grey -- real
Darcula's own button chrome is grey, not accent-colored; `signal` now shows
up only on genuine call-state signals (connected badge, presence-available
dot, the ringing-screen's Accept button paired against a red Reject, ZRTP SAS
text, voicemail count). The old blue hex is kept as `link`, wired only to
`Visuals::hyperlink_color` -- there's no visible in-app hyperlink today, but
this keeps "blue = links/numbers only" true if one's ever added, rather than
quietly reintroducing blue as a second accent. Spacing (`apply_style`'s
`item_spacing`/`button_padding`, `card_frame`'s `inner_margin`) also
loosened -- the v2 "too much chrome" density pass had gone further than this
redesign's own margins needed, per feedback that the whole app now read as
too tight/cramped.

**v4 (2026-07-11)**: switched from Darcula to real IntelliJ Light theme
values, per user request ("light mode only", no toggle -- same single-theme
shape v3 already had, just the other identity). Sourced from JetBrains' own
`expUI_light.theme.json` (the modern IntelliJ Light theme's named palette),
not approximated: canvas `#F7F8FA` (`Gray13`, the theme's own global `"*"`
background), surface `#FFFFFF` (`Gray14`, used for elevated/search-field-style
surfaces) one step lighter than canvas -- mirrors the same canvas/surface
relationship v3's Darcula had (surface one step off canvas), just inverted
since light canvases sit *below* white surfaces instead of above a darker
one. `border` is the theme's own `Component.borderColor` (`Gray9`
`#C9CCD6`). `signal`/`ringing`/`danger` are `Green4`/`Yellow1`/`Red2` from
the same palette -- `Yellow1` (`#A46704`, a dark amber) and `Red2`
(`#BC303E`) rather than the theme's own lighter `Yellow4`/`Red4` tokens,
since DeeLip uses these as solid text/fill colors needing real contrast
against a white surface, not the subtle inline-hint tint JetBrains uses
`Red4` for. `link` is the theme's own `Blue2` (`#315FBD`) -- same narrow
"hyperlink text only" role v3.1 established, still off by default since
nothing renders one. `ink` (`#000000`, `Gray1`) is the palette's own darkest
token; the theme file has no single explicit global foreground key to quote
(inherited from the base Swing LaF), so this is the closest sourced value
rather than a guess. The "color = call state only, chrome stays neutral"
rule from v3.1 carries over unchanged -- buttons/tabs/selection still use
`surface`/`surface_hover` grey, not `signal` or the theme's own blue accent
(`Blue4` `#3574F0`, deliberately not used anywhere in this palette, same
reasoning as v3.1's "blue only for links" decision).

## Known broken icons

The bundled `egui-phosphor` 0.6.0 "Regular" variant font has several
codepoints whose cmap resolves to the wrong glyph -- not a tofu box, but a
real (wrong) Latin letter or punctuation mark, discovered by rendering every
icon constant this app uses at a large size and inspecting the actual shape.

Confirmed broken so far: `INFO`, `BACKSPACE`, `ARROW_BEND_UP_RIGHT`,
`ARROW_DOWN_LEFT`, `ARROW_UP_RIGHT`, `DOWNLOAD`, `DOWNLOAD_SIMPLE`,
`FILE_ARROW_DOWN`, `FLOPPY_DISK`, `ARROW_DOWN` -- these render fine:
`EXPORT`, `UPLOAD_SIMPLE`, `ARROW_SQUARE_OUT`.

Call sites needing a broken one use a plain Unicode character instead (e.g.
"⌫", "↱", "(i)") rather than the phosphor constant.

**This isn't limited to the phosphor icon font either**: a plain Unicode "☰"
(hamburger/trigram symbol) was also found silently rendering as "?" in this
app's actual font stack (caught live via Xvfb, not by reasoning about it) --
any icon-ish Unicode character, not just phosphor constants, needs to be
rendered large and actually looked at before trusting it; when in doubt use
a plain word instead.
