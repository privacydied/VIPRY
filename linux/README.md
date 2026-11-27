# VIPRY on Linux (Steam + Proton Experimental)

VIPRY = Proudnoob4/VIctory-NVIDIA `nvidia-hybrid-v2` (the fxceless/victory-fork#1268 community
NVIDIA merge) plus Linux/Proton tuning. Target: RTX 3090, Ryzen 7 3700X, 31 GB RAM.

- `VIPRY.env`: run switches (ported from `scripts/play-nvidia.ps1`), sourced by `steam-launch`.
- `steam-launch`: Steam launch wrapper (`steam-launch %command%`); sets Wine paths and DLL overrides.
- `victory-fork`: builds/switches forks; builds with cargo-xwin for x86_64-pc-windows-msvc.
- `vbench.py`: summarizes the frame-rate lines of a run log (needs `VICTORY_LOG=warn,victory_gpu::host=info`).

Wine needs Microsoft's `d3dcompiler_47.dll` beside `victory.exe` (`WINEDLLOVERRIDES=d3dcompiler_47=n`).
