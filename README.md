# VIPRY

A private fork of [VIctory](https://github.com/Marvisak/VIctory), the open-source
compatibility layer that runs the Xbox Series build of the title on a PC.

## Aim

Run the title on **Linux, through Steam and Proton, on an NVIDIA card**, at a
**locked 30 fps with no visual artifacts**. No other fork targets this.

## Target hardware

| Part | |
|---|---|
| GPU | 2 x RTX 3090 24 GB, NVIDIA driver 620 |
| CPU | Ryzen 7 3700X, 8 cores, 16 threads |
| RAM | 31 GiB + 39 GiB swap |
| OS | Arch Linux, Hyprland (Wayland) |
| Runtime | Steam, Proton Experimental, vkd3d-proton |

The CPU and RAM are the limits here, not the GPU.

## How to run it

The tools are in [linux/](linux/README.md) and live in `~/Games/victory`.

```sh
victory-fork build VIPRY      # release build (cargo-xwin, from Linux)
victory-fork run VIPRY 300    # run outside Steam for 300 s
linux/vbench.py <log>         # fps, frame time, copies, dropped draws
```

- Steam: set the shortcut's launch options to `steam-launch %command%`
  (`linux/steam-add-shortcut` adds it).
- Settings: `linux/VIPRY.env`.
- Put Microsoft's `d3dcompiler_47.dll` beside `victory.exe`.
- First run: give the pipelines time to compile, or the world stays black.
- You need your own dump of the title. Nothing from it is in this repository.

## Credits

Built on other people's work. The history is kept, and each commit taken from
elsewhere names its source and author.

- **[Marvisak/VIctory](https://github.com/Marvisak/VIctory)** and contributors:
  loader, runtime services, Direct3D 12 back end, shader recompiler.
- **[trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA)**:
  the NVIDIA base.
- **Proudnoob4**: the `nvidia-hybrid-v2` merge.
- **[avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)**:
  threading, write relay, card fills, copy queue, texture read-back.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)** and the
  authors of its pull requests, among them Tissma95, tippythetaidum,
  GamingRipperDoc and cccw.
- **[PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab)**: DXIL shader
  optimizations.
- **[readmet3xt-lab/foxhound](https://github.com/readmet3xt-lab/foxhound)**.
- **[DexNom/VIctory](https://github.com/DexNom/VIctory)**: ray tracing fixes,
  cube gradients, sparse textures.
- **[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)**
  and romainhedouin: ambient light, pipeline workers, barriers, render scale.
- **[FQY55kai/ZJOU-VICTORY](https://github.com/FQY55kai/ZJOU-VICTORY)**.
- **[myokex/victory-amd](https://github.com/myokex/victory-amd)**, Spexivus,
  joroyt, tightfall and Joshua Odom: page guards, title memory reads.
- **Geckomacabre** and **cxy0072019-sketch**: texture read-back.
- **[brothernog/rota](https://github.com/brothernog/rota)** (nog, nullrv, edsuuu)
  and AntoninMichelq: ray-miss path, heap worker, lean math, NVIDIA fixes.
- **[stellarr-studio/kelvaroq](https://github.com/stellarr-studio/kelvaroq)**:
  pipeline stand-ins and warm-up, wave64 pairing, async present, mesh launch.
  Only Kelvaroq's own GPL-3.0 code is here; nothing from XCL (GPL-2.0-only).
- **[Kemerd/VIctory-NVIDIA-RT](https://github.com/Kemerd/VIctory-NVIDIA-RT)**:
  graphics pipelines from pieces, heap views, ultrawide, HDR10 output, DLSS
  Ray Reconstruction, Reflex.
- **[Slaktarn700/SlaktarnVI](https://github.com/Slaktarn700/SlaktarnVI)**: a core
  for the submitting thread, the A/B report script.
- **[Spaghetih/victory-sley-fork](https://github.com/Spaghetih/victory-sley-fork)**:
  the pipeline workers' stack size.
- **[uiliannn/VItoriaBr](https://github.com/uiliannn/VItoriaBr)**: texture
  formats.
- **[ProjectLeonida/mandy](https://github.com/ProjectLeonida/mandy)**: the order
  of in-place indirect arguments for renderer port 1.
- **[CHECK6960/Victory-Nvidia-optimized](https://github.com/CHECK6960/Victory-Nvidia-optimized)**:
  coalesced ahead-interval search and shared read-journal guard.

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE); the one file not covered
is named in [docs/asset-policy.md](docs/asset-policy.md).
