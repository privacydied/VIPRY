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
  built ahead with `victory warm`.
- 22 compute shaders do not translate, because NVIDIA cards run 32 lanes and
  these shaders need 64.
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
[stellarr-studio/kelvaroq](https://github.com/stellarr-studio/kelvaroq) and
VItoriaBr (see [docs/RECONCILE.md](docs/RECONCILE.md)).

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

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE); the one file that is not
covered is named in [docs/asset-policy.md](docs/asset-policy.md).
