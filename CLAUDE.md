# CLAUDE.md

Repo-specific instructions for Claude Code. Read automatically at the start
of every session in this directory.

## Workflow

- Ask before committing, and ask before pushing — nothing on `development`
  has ever been pushed (no upstream configured); don't assume a prior
  approval carries forward.
- Prefer several small, single-purpose commits over one large one.
- Build/check clean is not enough for GUI/feature work: `cargo check
  --workspace --all-targets --locked`, `cargo test --workspace --locked`,
  `cargo clippy --workspace --all-targets` (matches `ci.yml`), plus live
  verification via the `verify` skill (Xvfb + xdotool) before calling a
  change done. If live verification isn't possible (no display, no
  hardware), say so explicitly rather than claiming success.
- If blocked by missing sudo/root, stop and ask the user to run the
  command themselves (suggest `!command`) — don't probe `sudo -l`,
  sudoers files, or alternate invocations.
- Branch flow: implement features/fixes on `development` → move to
  `testing` to write/run tests against that work → cut a new branch named
  after the target version (this is where the version number actually
  gets bumped, see versioning scheme below) → merge that version branch
  into `main`. Don't skip straight from `development` to `main`.
- Comments explaining *why* (design rationale, tradeoffs, things tried and
  rejected, bug writeups) go into the matching `docs/crates/<crate>.md`
  file, not as a source comment — write the doc, don't leave the
  explanation in the code. Only short local-invariant/bug-workaround
  one-liners belong inline in the source itself.

## Commit messages

- Short and straightforward — one line where possible.
- Never mention ROADMAP.md or other markdown files that has to do with implementation plan in a commit message.

## Other notes

- Cargo workspace: root binary crate `deelip` + `crates/{config,sip-core,
  media-engine,nat,ui,updater}`.
- Config persistence is SQLite (`deelip_config::Db`), respects
  `$XDG_CONFIG_HOME`.
- Two separate doc trees, don't confuse them, and they hold different
  *kinds* of writing, not just different audiences:
  - `docs/` (per-crate docs) is development notes — what was implemented
    and, importantly, *why* (design rationale, tradeoffs, things tried
    and rejected). Source of truth for architecture (see the Workflow
    rule above on where comments belong).
  - `pages/` is the public VitePress site (`npm run docs:build`, deployed
    via `.github/workflows/pages.yml`) — a marketing/showcase web page for
    the app (landing page, downloads, FAQ, troubleshooting, contact), not
    a documentation book. No implementation rationale, and no deep
    how-to-use reference pages either — that content was deliberately
    trimmed and folded into the landing page/FAQ/troubleshooting as short
    inline facts instead of standalone pages. Anyone wanting the deep
    per-crate docs is pointed at `docs/crates/` on GitHub directly.
  - `pages/changelog/` is neither of the above alone — it's for both
    developers and users, so keep entries readable by a user, not just a
    dev (no deep implementation specifics). It lives under `pages/`
    (rather than `docs/`) specifically so it publishes to the site — this
    is the one place development-adjacent content belongs there. One
    changelog file per weekly release, created on the version branch,
    named after the target version it documents (e.g. `0.2.0.md`), plus
    `index.md` listing them newest-first.
- `ROADMAP.md` is the current living planning doc (replaced the
  now-deleted `ARCHITECTURE_GAPS.md`) — it gets cleared/rewritten once a
  round closes, so treat it as current-round scratch, not a stable
  long-term anchor.

## Versioning

Release cadence is weekly: each week's `main` merge (see branch flow
above) is a new release, and the version (`major.mid.minor`, e.g. the
`workspace.package.version` in `Cargo.toml`) is bumped on the version
branch before merging:
- **mid** version: +1 every release, unconditionally.
- **minor** version: bumped by however many bugs were fixed in that
  release (e.g. 4 bug fixes this cycle → minor +4), not just +1.
- **major** version: not on this weekly cadence — bump only when
  explicitly told to.

**Retention:**
- Version branches: keep only the last 5; prune older ones once there are
  more than that.
- Changelogs: keep all of them only within the current major version. On
  a major bump (e.g. into `1.x`), archive every existing changelog and
  start the new major's changelog history fresh.
