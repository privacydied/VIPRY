# VIctory fork switcher (Linux, Proton Experimental)

Steam shortcut "Grand Theft Auto VI" -> current/victory.exe, launch options:
  /home/pry/Games/victory/bin/steam-launch %command%
`current` is a symlink to builds/<fork>. Switching forks = repointing it.

  victory-fork list                    what is here, * = what Steam starts
  victory-fork get <name> [ref]        fetch + build + make current (ref = commit/tag to pin)
  victory-fork use <name>              switch Steam to an already-built fork
  victory-fork add <name> <git-url> [branch]
  victory-fork add-prebuilt <name> <exe>
  victory-fork add-recipe <name>       local merge: recipes/<name>.recipe (base/merge/pick/pr steps)
  victory-fork env <name>              edit that fork's VICTORY_* switches / VICTORY_ARGS
  victory-fork run [name] [seconds]    run outside Steam (same Proton prefix), full log in logs/

One-off fork from Steam without switching: launch options
  VICTORY_FORK=dexnom /home/pry/Games/victory/bin/steam-launch %command%
Pre-translate shaders: VICTORY_MODE=shaders (or warm) in front of the same line.

Files: env/common.env (game path, shared), env/<fork>.env, cache/<fork>/ (per fork),
data/ (saves, shared), logs/, redist/d3dcompiler_47.dll (copied into every build).
Builds: isolated rustup in .rust/, cargo-xwin (MSVC CRT/SDK in .xwin/), target x86_64-pc-windows-msvc.
Under Steam the game's own log goes to its console window; `victory-fork run` captures it to a file.

VIPRY (current): recipe fork, recipes/VIPRY.recipe. Base = Proudnoob4/VIctory-NVIDIA nvidia-hybrid-v2,
the fxceless#1268 community NVIDIA merge (trustno1 base + avatarmanz wave/PRs + fx PRs + klbf-lab).
Add extra PRs/commits as lines in the recipe, then `victory-fork get VIPRY`.
Settings: env/VIPRY.env (from its play-nvidia.ps1; SKIP_WC off, 720p, RT skip list, long pipeline wait).
After a few warm runs set VICTORY_PIPELINE_WAIT_MS=500 there.
