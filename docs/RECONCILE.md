# Reconcile plan: other forks into VIPRY (NVIDIA, Linux/Proton)

Audit of 2026-11-27. Sources: portwatch-vi.xyz, git history of each fork
(`git cherry vipry/main <fork>`). Target PC: RTX 3090, Ryzen 7 3700X, 31 GB, Proton.

## Facts

| Fork | Commits not in VIPRY | NVIDIA result (their PC, Windows) |
|---|---|---|
| trustno1-coder/VIctory-NVIDIA `main` | 527 | 26-30 fps, RTX 5070 + 9800X3D, Series X 1440p, DLSS |
| readmet3xt-lab/foxhound (trustno1's base since 11-26) | 1 squashed (+825 lines vs `02fc18a`) | 30 fps 4070 SUPER + 7800X3D; **~15 fps 3080 + 5700X**; ~12 fps 2080S + 2600 |
| fxceless/victory-fork | 130 | 9-12 fps, RTX 4070 |
| stellarr-studio/kelvaroq | patch series 0001-0064 on fxceless `f85101b` | ~10 fps 4K, RTX 5080 |
| uiliannn/VItoriaBr | 128 (mostly ports of trustno1/fxceless) | up to 4-6 fps |
| Belobug | 146 | AMD only (22 fps 9070 XT) |
| avatarmanz, DexNom, klbf-lab, Marvisak | 133 / 131 / 112 / 271 | AMD or 1-7 fps |

- VIPRY's base is trustno1 `033be27` (25 Nov) + Proudnoob4's hybrid merge. trustno1's own
  README gives 15-17 fps for that line, against 26-30 fps after the move to foxhound.
- VIPRY measured here: 0.6 fps median, 406 ms/frame in memory copies, 1.6 ms GPU, 31 GB RAM full.
- trustno1 `main` does **not** carry: VICTORY_NAN_SCRUB, BVH_MISS, NV_FLOAT, E5_TARGETS,
  OWN_UPSCALER, SPLIT_LDS_COALESCE (these are VIPRY-only today).
- trustno1 `main` has the same pipeline-library bug VIPRY fixed in `b2567497`
  (`library.rs:515`, empty slice -> non-null blob -> E_INVALIDARG under vkd3d-proton).

## Plan, biggest win first

### 1. Move VIPRY's base to trustno1 `main` (foxhound line) — do not cherry-pick 500 commits

New branch `vipry-next` from `trustno1/main`, then replay ours on top:
`linux/` tooling, `AGENTS.md`, `b2567497` (pipeline library), the stack-overflow reporter,
and the RAM agent's `4ccf65a8` / `9c49c062` if they still apply. Port its
`scripts/play-nvidia.ps1` defaults into `linux/VIPRY.env`. Try `-SeriesS` (720p inside) first:
this CPU is the limit. Gate: `vbench.py` median fps on the same scene must beat VIPRY's 0.6,
no new artifacts in screenshots. Then fast-forward `main`.

Proton notes: DLSS Super Resolution needs `PROTON_ENABLE_NVAPI=1` (dxvk-nvapi), not yet tested.
Frame generation needs RTX 40+: not for the 3090.

### 2. Linux-only wins (no other fork runs on Proton)

- Pipeline cache: library fix (done), one NVIDIA driver cache path for Steam and direct runs,
  size above the 1 GiB default (`~/.cache/nvidia/GLCache` was at 1021 MB, evicting), then `victory warm`.
- Pipeline workers overflow a 64 MB stack (3 per run): find the recursion (reporter in work).
- Write watch and guard pages cost a signal per page under Wine. Measure with `perf` on the
  wine process first. Candidates: trustno1 `VICTORY_FAULT_UNLOCKED`/`GUARD_HOT`, fxceless
  4 KiB dirty tracking (`e4245f3c`), Kelvaroq `0045-watch-reservation`.
- One compute shader still refused for wave64 by vkd3d-proton.

### 3. Cherry-pick list onto the new base (check absent, pick, measure, one commit each)

| Commit | From | Why |
|---|---|---|
| `e04c0781` constant table after instructions (#2143) | Marvisak | GBs of NVIDIA VRAM |
| `4fd34fdd` tiled cascade shadows exact on wave32 | Belobug | shadow artifacts on NVIDIA |
| `25025d2f` final LDS flag from every lane, 32-lane | Belobug | wave32 correctness |
| `4106550f` shadow writeback merging (#2054) | Marvisak | copy-back CPU time |
| `ea812894` uploads readable in one barrier batch | Marvisak | fewer barriers |
| `465b94f5` VICTORY_INDIRECT_GATHER | fxceless | 10.3 fps in first rooms (theirs) |
| NaN scrub, BVH_MISS, E5 targets (old trustno1 line) | VIPRY today | only if black blobs / RT haze come back |
| Kelvaroq 0039 rebar copies, 0046/0048 async present, 0063 uma | Kelvaroq | large port: after 1-3 |

Skip: AMD-only work (Belobug RDNA4, avatarmanz 7900 XTX tuning), DexNom ray tracing (1 fps),
klbf-lab (mostly bench/README), VItoriaBr (ports of trustno1/fxceless already above).

## Expectation

foxhound's own table puts a 3080 + Ryzen 7 5700X at about 15 fps on Windows. A 3700X is slower,
and Wine adds cost. 30 fps on this PC needs the copy loop gone (Kelvaroq's unified memory,
not public yet) or a faster CPU. 31 GB RAM is at the floor (foxhound ran on 32 GB).
