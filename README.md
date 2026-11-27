# VIPRY

VIPRY is a private fork of [VIctory](https://github.com/Marvisak/VIctory), the
open-source compatibility layer that runs the Xbox Series build of the title on
a PC. Repository: `github.com/privacydied/VIPRY`.

## Goal

Run the title on **Linux, through Steam and Proton Experimental, on an NVIDIA
card**, at a **locked 30 fps with no visual artifacts**.

No other fork targets this. The other forks run on Windows, and most are tuned
for AMD cards. VIPRY takes the best NVIDIA work from those forks and adds what
Linux and Proton need.

## Status (27 November 2026)

We are not at the goal yet. Measured on the target machine (below), in the world:

| Branch | Base | fps | Main limit |
|---|---|---|---|
| `main` | trustno1 `033be27` + the `nvidia-hybrid-v2` merge | about 0.6 | CPU time copying title memory (300-1300 ms a frame); free RAM under 3 GB, so the game swaps |
| `vipry-next` | trustno1 `main` (the foxhound line) | 0.5-1.9 | the same |

- `vipry-next` is a trial. It is the line the NVIDIA forks build on now. If it
  measures faster than `main` on the same scene, it becomes `main`. If not, we
  delete it.
- About 600 draws a frame are dropped while their pipelines compile. The
  pipeline cache is fixed (it now saves under Proton), and pipelines can be
  built ahead with `victory warm`. On `vipry-next`, `VICTORY_PIPELINE_STANDINS=1`
  (from Kelvaroq, not yet measured) draws them with a built look-alike pipeline.
- 22 compute shaders do not translate, because NVIDIA cards run 32 lanes and
  these shaders need 64. On `vipry-next`, `VICTORY_WAVE64_PAIRED=1` (from
  Kelvaroq, not yet measured) runs them with two console lanes in each lane.
- Some pipeline worker threads die of a stack overflow. Those pipelines are
  never made.
- Plans and measurements: [docs/RECONCILE.md](docs/RECONCILE.md).

## Where the code comes from

The `main` branch is the merge Proudnoob4 posted in fxceless/victory-fork#1268
(branch `nvidia-hybrid-v2`), with our Linux work on top:

- **[trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA)**:
  the NVIDIA base (commit `033be27` on `main`, its current `main` on
  `vipry-next`). It draws everything on NVIDIA, its NaN scrub fixes the black
  sun and black blobs, and it skips the ray-tracing passes that break.
- **[avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)**:
  the speed work (threads, write relay, fills on the card, copy queue, texture
  read-back) and 11 of its open pull requests.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)**: pull
  requests #868, #907, #1044, #1146 and #1210.
- **[PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab)**: #41 and #50
  (DXIL shader optimizations).
- **[Marvisak/VIctory](https://github.com/Marvisak/VIctory)**: the layer under
  all of it (loader, runtime services, Direct3D 12 back end, shader recompiler).
- **[readmet3xt-lab/foxhound](https://github.com/readmet3xt-lab/foxhound)**:
  through trustno1's current `main`, on `vipry-next` only.

Single commits we take from other forks name the source commit and its author.
Forks audited for code to take: Belobug, DexNom, myokex,
[stellarr-studio/kelvaroq](https://github.com/stellarr-studio/kelvaroq),
[brothernog/rota](https://github.com/brothernog/rota) and VItoriaBr (see [docs/RECONCILE.md](docs/RECONCILE.md)).

## Taken onto `vipry-next` from other forks

Code taken after the move to trustno1's current `main`. Each commit names its
source and author. None of it is measured in the title yet: the release build
passes, and the unit tests under Wine fail only the tests that fail on the base
too (tests that need a real GPU are skipped or fail there in the same way).
Every new switch is **off** unless the table says otherwise.

**Single commits**

| Commit | From | What |
|---|---|---|
| `9425df50` | fxceless (PR #1210) | A view moved to the first level with memory keeps the title's minimum level |
| `8fb1aed2` | fxceless | CI: the gpu crate's tests run one at a time |
| `3d6fa92c` | cccw (#1320) | Lists a split submission runs at once give their descriptor table slots back |
| `2eac15c9` | fxceless | `VICTORY_DUMP_REFUSED=<dir>` saves a shader that does not translate, for offline work |
| `cc2d1656` | rota (nog) | Skip the depth-bounds test on the software adapter |
| `20701aa0` | rota (nog) | Tiled cascade shadows translate exactly on 32-lane cards (the same fix as Belobug `4fd34fd`) |
| `20834d4b` | rota (nog) | `VICTORY_DYNAMIC_SLOTS` on a capped view heap |
| `d8efddc9` | rota (nog) | HLE test: adding a user never waits for a picker |
| `5a8d8097` | rota (nog) | GPU time stamps only in the timing mode |
| `225d4531` | rota (nullrv) | Pipeline warm-up records kept per adapter (this PC has two 3090s) |
| `2eabed07` | rota (nog) | `VICTORY_LIBRARY_PREFETCH=1` reads the mapped pipeline library at once |

**Ported by hand** (the code did not apply; it was re-fitted to our tree)

| Switch | From | What |
|---|---|---|
| `VICTORY_RT_MISS=1` | rota `7f18c7b1`, `a291870d` | Every ray is a miss, and the ray-only compute programs (skinning, about 1,000 hit-shading programs, probe ray activation, tracing) are left out. Upstream: +0.3 to +0.8 fps. `VICTORY_RT_SKIN`, `VICTORY_RT_RESOLVE`, `VICTORY_ACTIVATE_RAYS`, `VICTORY_RT_TRACE` keep one kind |
| `VICTORY_HEAP_WORKER=1` | rota `d15ae120`, `1654bf2b` | The texture pass's rechecks run on a thread of ours, off the frame thread |
| `VICTORY_LEAN_MATH=<bits>` | rota `52c6d16a` (from AntoninMichelq/VIctory-forkAMD) | Cheaper host forms of console float math; differs only at NaN, a zero's sign, or one rounding |
| `VICTORY_WAVE64_PAIRED=1` | Kelvaroq patch 0052 | A 64-lane program that no 32-lane form takes runs with two console lanes in each host lane |
| `VICTORY_PIPELINE_STANDINS=1` | Kelvaroq patch 0050 | While a pipeline is built, its draws use a built pipeline with the same shaders and compatible state, instead of being dropped |
| `VICTORY_PIPELINE_WARMUP=1` | Kelvaroq patch 0050 | Saves the exact pipeline recipes and builds them, most used first, at the next start |
| `VICTORY_WATCH_RESERVATION=1` | Kelvaroq patch 0045 | One write-watch query per memory reservation per submission. Wine gives no region information, so under Proton it falls back to the old path |
| `VICTORY_ASYNC_PRESENT=1` | Kelvaroq patch 0046 | Present returns with up to `VICTORY_FRAMES_AHEAD` (2) frames in flight; needs `VICTORY_ASYNC_SUBMIT=1`. Kelvaroq's 0048 turns it on by default; we keep it off |
| `VICTORY_MESH_LAUNCH=1` | Kelvaroq patch 0041 | Indirect mesh draws are launched by an amplification stage of ours instead of a clamp pass between two full waits. Upstream: heavy frame 198 to 120 ms, mostly GPU time. Kelvaroq ships it on; we keep it off until it is tested on the 3090 |

**Settings from rota's README** (no code; `linux/vipry-next.env` does not have
them yet): `VICTORY_THIN=0` (rota: `THIN=1` more than halves the frame rate on
NVIDIA, and our first `vipry-next` run had it on), `VICTORY_CARD_STEPS=1`,
`VICTORY_CARD_ALL=1`, `VICTORY_MEMORY_CARD_MB=1024`, `VICTORY_MEMORY_MAP=0`,
`VICTORY_DYNAMIC_SLOTS=499999`, `VICTORY_PIPELINE_WORKERS=6`, then
`victory warm --used` after one run with `VICTORY_PIPELINE_BOOK=1`.

**Not taken**

- rota `9eb26275` (small buffers on the card) and `TEXTURE_DROP_MIPS`: they
  help 8-12 GB cards; a 24 GB card already gets these limits. rota `5e5aaf9a`
  (half views): needs rota's 16-bit view work.
- Kelvaroq 0044 (in-place copies) and 0059 (pipeline precompile): we have
  trustno1's versions. 0058 (CLZERO): the 3700X has the instruction. 0062 and
  0063 (start of the shared-memory layer): built on Windows memory sections,
  high risk under Wine, and the full layer is not public. 0049 and 0060: many
  small fixes, taken only if we meet the same bug. 0041's wide hash: we already
  hash with a group of 64 threads.
- Kelvaroq says patches from 0049 on rebuild ideas from XCL
  (github.com/ivall/xcl, GPL-2.0-only). The code we took is Kelvaroq's own,
  under GPL-3.0; no code from XCL itself is in this repository.

## Target machine

| Part | |
|---|---|
| OS | Arch Linux, kernel 7.3.6-arch1-1 |
| Desktop | Hyprland (Wayland) |
| GPU | 2 x NVIDIA GeForce RTX 3090 24 GB, driver 620.73.04, Vulkan 1.4 |
| CPU | AMD Ryzen 7 3700X, 8 cores, 16 threads |
| RAM | 31 GiB, plus 39 GiB swap |
| Runtime | Steam, Proton Experimental (11.0, 12 November 2026), vkd3d-proton for Direct3D 12 |
| Build | cargo-xwin, target `x86_64-pc-windows-msvc`, from Linux |

The CPU and the RAM are the limits on this machine, not the GPU. Results from
the other forks' READMEs (Windows, faster CPUs) do not carry over directly.

## Build and run

The tools are in [linux/](linux/README.md). They live in `~/Games/victory`.

```sh
victory-fork build VIPRY          # release build of victory.exe
victory-fork run VIPRY 300        # run outside Steam for 300 s, log to logs/
linux/vbench.py <log>             # fps, frame time, copies, dropped draws
```

- In Steam, set the launch options of the title's shortcut to
  `steam-launch %command%`. `linux/steam-add-shortcut` adds the shortcut.
- Run settings: `linux/VIPRY.env` (`linux/vipry-next.env` on `vipry-next`).
- Wine needs Microsoft's `d3dcompiler_47.dll` beside `victory.exe`.
- On the first run, give the pipelines time to compile, or the world stays
  black.
- `victory-fork` switches between forks. Each fork has its own build and
  settings.

You need your own dump of the title. Nothing from the title is in this
repository.

## Rules

See [AGENTS.md](AGENTS.md): commit and push after every meaningful change, one
worktree per agent, and the content rules (nothing from the title, no names,
loopback-only network).

## Credits

This is a tree of other people's work with more on top. The history is kept, and
commits taken from elsewhere name the commit and its author.

- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its
  contributors: the loader, the runtime services, the Direct3D 12 back end and
  the shader recompiler.
- **[trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA)**:
  the NVIDIA base.
- **Proudnoob4**: the `nvidia-hybrid-v2` merge (fxceless/victory-fork#1268).
- **[avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)**:
  threading, write relay, card fills, copy queue, texture read-back.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)**: the fork
  the 7900 XTX one started from, the people whose pull requests it carries,
  pull request 1189 (Tissma95) for the Series S interface plane, 927
  (tippythetaidum) for the face network and 1216 (GamingRipperDoc) for the
  marked constant buffer slots.
- **[PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab)**: DXIL shader
  optimizations.
- **[readmet3xt-lab/foxhound](https://github.com/readmet3xt-lab/foxhound)**: the
  base of trustno1's current line.
- **[DexNom/VIctory](https://github.com/DexNom/VIctory)**: ray tracing fixes,
  cube gradients, sparse textures.
- **[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)**:
  the ambient light idea, pipeline workers, and work by romainhedouin.
- **[FQY55kai/ZJOU-VICTORY](https://github.com/FQY55kai/ZJOU-VICTORY)**.
- **[myokex/victory-amd](https://github.com/myokex/victory-amd)**: the guard over
  every view of a page (with Spexivus, joroyt and tightfall), and Joshua Odom's
  reads into title memory.
- **Geckomacabre** and **cxy0072019-sketch**: the texture read-back.
- **[brothernog/rota](https://github.com/brothernog/rota)** (nog, nullrv,
  edsuuu's VI-BR-PORT before it): the commits and ports listed above.
- **[stellarr-studio/kelvaroq](https://github.com/stellarr-studio/kelvaroq)**:
  patches 0041, 0045, 0046, 0050 and 0052.
- **cccw**: pull request #1320.

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE); the one file that is not
covered is named in [docs/asset-policy.md](docs/asset-policy.md).
