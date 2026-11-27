# AGENTS.md — VIPRY

VIPRY is a private fork, `github.com/privacydied/VIPRY` (branch `main`). It is
Proudnoob4/VIctory-NVIDIA `nvidia-hybrid-v2` plus our Linux/Proton work.
Target: RTX 3090, Ryzen 7 3700X, 31 GB RAM, Steam with Proton Experimental.

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
