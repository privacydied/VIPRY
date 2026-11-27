# AGENTS.md — VIPRY

VIPRY is a private fork, `github.com/privacydied/VIPRY` (branch `main`). It is
Proudnoob4/VIctory-NVIDIA `nvidia-hybrid-v2` plus our Linux/Proton work.
`nvidia-hybrid-v2` is built on trustno1-coder/VIctory-NVIDIA
(`github.com/trustno1-coder/VIctory-NVIDIA`, commit `033be27`). That fork is the
NVIDIA base: it draws everything on NVIDIA, its NaN scrub fixes the black sun and
black blobs, and it skips the broken ray-tracing passes. The avatarmanz,
fxceless and klbf-lab work is merged on top of it. So VIPRY is also a fork of
trustno1-coder/VIctory-NVIDIA.
Target: RTX 3090, Ryzen 7 3700X, 31 GB RAM, Steam with Proton Experimental.

## Rules for content

- Nothing from any game enters this repository: no game files, executables,
  shaders (in any form, their names included), textures, audio, screenshots, or
  logs that quote them. Shader hashes and plain numbers (fps, milliseconds,
  megabytes, counts, error codes) are fine.
- Do not name games, publishers or their staff in code, comments, commits or pull
  requests. Say "the title".
- Do not loosen `crates/victory-hle/src/modules/net.rs`: the title may only reach
  loopback.
- No GPL-2.0-only code. Nothing from repositories without a licence.
- Write commits in English. No AI attribution lines in commits. A pull request
  body ends with `AI-assisted`.

## No god files

- Do not write monolithic "god files". One file does one job.
- New code that does its own job goes in a new module, not at the end of a large file.
- Do not make the large files larger. The largest today: `crates/victory-shader/src/emit/lower.rs`
  (about 14,000 lines), `crates/victory-gpu/src/host.rs` (about 11,500),
  `crates/victory-gpu/src/descriptor/host.rs`, `draw.rs`, `shadow.rs` and `watch.rs`
  (5,000-7,000 each). When you change one of them, put the new logic in a module of its own and
  call it from there. A few lines in a large file to call the new module are fine.
- Do not split a large file as part of a port. Split it in its own commit, and only when asked.
- Moving existing code out of a large file is its own commit, with no change in behavior.
  Do not mix it with a fix or a feature.

## Commit and push

- Commit and push after every meaningful change. Do not let work pile up uncommitted.
- One change per commit. Give each commit a message that says what changed and why.
- Push to `main`. Before you push, run `git fetch` and rebase onto `origin/main`.
  More than one agent pushes to this repo.
- Never force-push `main`.

## Shared worktrees

- More than one agent works on this repo at the same time. Each agent uses its own
  git worktree. Do not merge, reset or check out in a worktree that another agent uses.
- Do not leave a merge or rebase unfinished in a shared worktree.

## Build and run

- Build: `victory-fork build VIPRY`. This is a release build made with cargo-xwin.
- Run settings: `linux/VIPRY.env`. The live copy is `~/Games/victory/env/VIPRY.env`.
- Test run outside Steam: `victory-fork run VIPRY <seconds>`. Summarize the log with
  `linux/vbench.py <log>`.
