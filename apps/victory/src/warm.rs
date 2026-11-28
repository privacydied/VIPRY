//! `victory warm`: host pipelines made ahead of a run (`known_pipelines`), in
//! processes of their own: a compiler that crashes or hangs costs one pipeline.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use victory_hle::known_pipelines as known;

/// Pipelines a process makes before another takes its place: the compiler's
/// and the translations' memory goes with it.
const AT_ONCE: usize = 40;
/// A pipeline that takes longer is taken for one the compiler hangs on.
const LONGEST: Duration = Duration::from_secs(180);

/// As the runs count one that takes long.
const SLOW_MS: f64 = 1000.0;

fn folder() -> Result<PathBuf> {
    let cfg = victory_core::Config::load()?;
    Ok(cfg.cache_subdir("shaders")?.with_file_name("pipelines"))
}

fn select_pipelines(
    keys: &[String],
    done: &HashSet<&str>,
    bad: &HashSet<String>,
    early: &HashSet<&str>,
    again: bool,
    used: bool,
    limit: Option<usize>,
) -> Vec<usize> {
    // Those earlier runs drew with first, the others in no order (by key).
    let mut all: Vec<usize> = (0..keys.len())
        .filter(|&n| {
            let key = keys[n].as_str();
            (again || !done.contains(key))
                && (again || !bad.contains(key))
                && (!used || early.contains(key))
        })
        .collect();
    all.sort_by_key(|&n| (!early.contains(keys[n].as_str()), &keys[n]));
    all.truncate(limit.unwrap_or(usize::MAX));
    all
}

pub fn run(jobs: Option<usize>, limit: Option<usize>, again: bool, used: bool, these: Option<String>) -> Result<()> {
    let dir = folder()?;
    if let Some(these) = these {
        return make(&dir, &these);
    }
    let began = Instant::now();
    // (A first preload on a PC no run has written pipelines down on yet: none known.)
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    victory_hle::known_pipelines::use_translations(dir.with_file_name("shaders"));
    // The driver keeps what is made here by program path and card: made by
    // another copy of this program, or on the other card of a PC with two, a
    // pipeline is not made for this one (v2026.11.27.8 made them all on an RTX
    // 5060 Ti while the runs played on the RTX 5070 beside it: black parts in
    // the picture where the 5070 had not made a pipeline yet).
    let fingerprint = {
        let program = std::env::current_exe().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
        let card = victory_shader::emit::host::hardware_adapter().map(|(_, name, _)| name).unwrap_or_default();
        let mut h = blake3::Hasher::new();
        h.update(known::fingerprint().as_bytes());
        h.update(program.as_bytes());
        h.update(card.as_bytes());
        (h.finalize().to_hex()[..16].to_owned(), card)
    };
    let (fingerprint, card) = fingerprint;
    let done_file = dir.join("warm-done.txt");
    let bad_file = dir.join(known::WARM_BAD_FILE);
    let keys = known::keys(&dir);
    let listed = std::fs::read_to_string(&done_file).unwrap_or_default();
    let mut done: HashSet<&str> = HashSet::new();
    for line in listed.lines() {
        if let Some((of, key)) = line.split_once(' ')
            && of == fingerprint
        {
            done.insert(key);
        }
    }
    let bad: HashSet<String> = if known::bad_list_disabled() {
        HashSet::new()
    } else {
        let text = std::fs::read_to_string(&bad_file).unwrap_or_default();
        known::parse_bad_keys(&text, &fingerprint)
    };
    let early = std::fs::read_to_string(dir.join("early-pipelines.txt")).unwrap_or_default();
    let early: HashSet<&str> = early.lines().collect();
    let all = select_pipelines(&keys, &done, &bad, &early, again, used, limit);
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let jobs = jobs.unwrap_or(cores * 2 / 3).max(1);
    println!(
        "{} pipelines known, {} made before ({fingerprint}, on {card}), {} bad; {} to make, {jobs} processes",
        keys.len(),
        done.len(),
        bad.len(),
        all.len()
    );
    let state = Mutex::new((Vec::<f64>::new(), 0usize, 0usize));
    let done_out = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(&done_file)?);
    // (Read back by the runs: these are made beside the others, not before.)
    let slow_out = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(dir.join("slow-pipelines.txt"))?);
    let bad_out = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(&bad_file)?);
    run_batches(
        &all,
        jobs,
        start_worker,
        |n, ms, there| {
            if ms >= SLOW_MS {
                let _ = writeln!(lock(&slow_out), "\n{} {ms:.0}", keys[n]);
            }
            let mut state = lock(&state);
            state.0.push(ms);
            state.1 += usize::from(!there);
            let count = state.0.len();
            if count.is_multiple_of(1000) {
                println!("{count} of {}, {:.0} s", all.len(), began.elapsed().as_secs_f64());
            }
            drop(state);
            // One with no host pipeline was left out (a
            // pipeline the run's switches leave out, such as
            // `CS_RaytracingResolve` without the structure
            // builder) or refused: not done, so that a warm
            // with other switches still makes it.
            if there {
                let _ = writeln!(lock(&done_out), "{fingerprint} {}", keys[n]);
            }
        },
        |n, why| {
            println!("pipeline {n} {}: {why}", keys[n]);
            let _ = writeln!(lock(&bad_out), "{}", known::format_bad_entry(&keys[n], &fingerprint, &why));
            lock(&state).2 += 1;
        },
    )?;
    let (mut times, refused, crashed) = state.into_inner().unwrap_or_else(|e| e.into_inner());
    times.sort_by(f64::total_cmp);
    let at = |q: f64| times.get((times.len() as f64 * q) as usize).copied().unwrap_or(0.0);
    println!(
        "{} made in {:.0} s ({refused} have no host pipeline, {crashed} ended their process); ms each: median {:.1}, p90 {:.1}, p99 {:.1}, most {:.0}, sum {:.0} s",
        times.len(),
        began.elapsed().as_secs_f64(),
        at(0.5),
        at(0.9),
        at(0.99),
        times.last().copied().unwrap_or(0.0),
        times.iter().sum::<f64>() / 1e3
    );
    Ok(())
}

fn run_batches(
    all: &[usize],
    jobs: usize,
    start: impl Fn(&[usize]) -> Result<Child> + Sync,
    each: impl Fn(usize, f64, bool) + Sync,
    bad: impl Fn(usize, String) + Sync,
) -> Result<()> {
    let chunks: Vec<&[usize]> = all.chunks(AT_ONCE).collect();
    let next = AtomicUsize::new(0);
    // Stop launching workers after a setup failure. In-flight workers can still report.
    let failure = OnceLock::new();
    std::thread::scope(|s| {
        for _ in 0..jobs.min(chunks.len()) {
            s.spawn(|| {
                while let Some(&chunk) = chunks.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let mut rest = chunk;
                    while !rest.is_empty() {
                        if failure.get().is_some() {
                            return;
                        }
                        let outcome = start(rest)
                            .context("could not start warm worker")
                            .and_then(|process| child(process, &each));
                        match outcome {
                            Ok(Some((made, why))) => {
                                let remaining = &rest[made.min(rest.len())..];
                                let Some((&n, after)) = remaining.split_first() else {
                                    break;
                                };
                                bad(n, why);
                                rest = after;
                            }
                            Ok(None) => break,
                            Err(error) => {
                                let _ = failure.set(error);
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    failure.into_inner().map_or(Ok(()), Err)
}

/// `victory preload`'s last step: the pipelines a run makes while it loads
/// (the early list) and those that take a second or more, made again in this
/// process and stored in the host pipeline library the runs load at their
/// start. `run` made them in processes of their own first, so the driver has
/// them and they come quickly; those that ended a process are left out. Not
/// all the known ones: one process holding the compiler's memory for 88,000
/// pipelines is what `AT_ONCE` keeps `run` from.
pub fn into_library(jobs: Option<usize>) -> Result<()> {
    let dir = folder()?;
    let cfg = victory_core::Config::load()?;
    known::use_library(cfg.cache_subdir("shaders")?);
    let keys = known::keys(&dir);
    let read = |file: &str| std::fs::read_to_string(dir.join(file)).unwrap_or_default();
    let (bad, early, slow) = (read("warm-bad.txt"), read("early-pipelines.txt"), read("slow-pipelines.txt"));
    let bad: HashSet<&str> = bad.lines().filter_map(|l| l.split(' ').next()).collect();
    let early: HashSet<&str> = early.lines().collect();
    let slow: HashSet<&str> = slow.lines().filter_map(|l| l.split(' ').next()).filter(|k| !k.is_empty()).collect();
    let mut all: Vec<usize> = (0..keys.len())
        .filter(|&n| {
            let key = keys[n].as_str();
            !bad.contains(key) && (early.contains(key) || slow.contains(key))
        })
        .collect();
    all.sort_by_key(|&n| (!early.contains(keys[n].as_str()), &keys[n]));
    let threads = jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()) * 2 / 3).max(1);
    println!("into the pipeline library: {} pipelines (loading and slow ones), {threads} threads", all.len());
    let began = Instant::now();
    let (count, made) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|s| {
        for t in 0..threads {
            let these: Vec<usize> = all.iter().copied().skip(t).step_by(threads).collect();
            let (count, made, total, dir) = (&count, &made, all.len(), &dir);
            s.spawn(move || {
                known::make(dir, &these, |_, came| {
                    let Some((_, there)) = came else { return };
                    made.fetch_add(usize::from(there), Ordering::Relaxed);
                    let n = count.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(500) {
                        println!("{n} of {total} in the library, {:.0} s", began.elapsed().as_secs_f64());
                    }
                });
            });
        }
    });
    known::keep_library();
    println!(
        "{} of {} in the pipeline library in {:.0} s",
        made.load(Ordering::Relaxed),
        all.len(),
        began.elapsed().as_secs_f64()
    );
    Ok(())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn start_worker(these: &[usize]) -> Result<Child> {
    use std::os::windows::process::CommandExt;
    const IDLE_PRIORITY_CLASS: u32 = 0x40;
    let exe = std::env::current_exe().context("could not locate the warm worker executable")?;
    let list: Vec<String> = these.iter().map(usize::to_string).collect();
    Command::new(&exe)
        .args(["warm", "--these", &list.join(",")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(IDLE_PRIORITY_CLASS)
        .spawn()
        .with_context(|| format!("could not spawn {}", exe.display()))
}

/// On abnormal exit, return the number of reports before the failure and the exit reason.
fn child(mut process: Child, mut each: impl FnMut(usize, f64, bool)) -> Result<Option<(usize, String)>> {
    let Some(out) = process.stdout.take() else {
        let _ = process.kill();
        process.wait().context("warm worker stdout unavailable, failed to reap worker")?;
        anyhow::bail!("warm worker stdout unavailable");
    };
    let process = Mutex::new(process);
    let (heard, over) = (Mutex::new(Instant::now()), AtomicBool::new(false));
    let hung = AtomicBool::new(false);
    let mut made = 0;
    std::thread::scope(|s| {
        s.spawn(|| {
            while !over.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                if lock(&heard).elapsed() > LONGEST {
                    hung.store(true, Ordering::Relaxed);
                    let _ = lock(&process).kill();
                    return;
                }
            }
        });
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            *lock(&heard) = Instant::now();
            let mut words = line.split(' ');
            if words.next() == Some("made")
                && let (Some(n), Some(ms), Some(there)) = (words.next(), words.next(), words.next())
                && let (Ok(n), Ok(ms)) = (n.parse(), ms.parse())
            {
                made += 1;
                each(n, ms, there == "1");
            }
        }
        over.store(true, Ordering::Relaxed);
    });
    let status = lock(&process).wait();
    let well = status.as_ref().is_ok_and(|s| s.success());
    if well {
        return Ok(None);
    }
    let why = if hung.load(Ordering::Relaxed) {
        format!("hung for {} s", LONGEST.as_secs())
    } else {
        status.map_or_else(|e| e.to_string(), |s| format!("{s}"))
    };
    Ok(Some((made, why)))
}

/// What such a process does.
fn make(dir: &Path, these: &str) -> Result<()> {
    let these: Vec<usize> = these.split(',').map(|n| n.parse().context("--these")).collect::<Result<_>>()?;
    let cfg = victory_core::Config::load()?;
    victory_hle::known_pipelines::use_translations(cfg.cache_subdir("shaders")?);
    let mut out = std::io::stdout();
    known::make(dir, &these, |n, came| {
        let told = match came {
            None => writeln!(out, "at {n}"),
            Some((ms, there)) => writeln!(out, "made {n} {ms:.2} {}", u8::from(there)),
        };
        if told.and_then(|()| out.flush()).is_err() {
            // Whoever asked is gone.
            std::process::exit(1);
        }
    });
    // Without what a run's end does (nothing to save, the host's threads).
    let _ = out.flush();
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;

    fn fake_child(script: &str, stdout: Stdio) -> Result<Child> {
        let cmd = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\cmd.exe");
        Ok(Command::new(cmd)
            .args(["/d", "/c"])
            .raw_arg(script)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(Stdio::null())
            .spawn()?)
    }

    #[test]
    fn missing_executable_leaves_the_batch_pending() {
        let missing = std::env::temp_dir().join(format!("victory-missing-worker-{}.exe", std::process::id()));
        assert!(!missing.try_exists().unwrap());
        let starts = AtomicUsize::new(0);
        let done = Mutex::new(Vec::new());
        let bad = Mutex::new(Vec::new());
        let result = run_batches(
            &[7, 8, 9],
            1,
            |_| {
                starts.fetch_add(1, Ordering::Relaxed);
                Ok(Command::new(&missing).spawn()?)
            },
            |n, _, _| lock(&done).push(n),
            |n, _| lock(&bad).push(n),
        );
        assert_eq!(starts.load(Ordering::Relaxed), 1);
        assert!(lock(&done).is_empty());
        assert!(lock(&bad).is_empty());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("start warm worker"));
        assert_eq!(error.downcast_ref::<std::io::Error>().unwrap().kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn missing_stdout_is_a_startup_failure() {
        let error = run_batches(
            &[1, 2],
            1,
            |_| fake_child("exit /b 0", Stdio::null()),
            |_, _, _| panic!("unexpected completion"),
            |_, _| panic!("unexpected bad pipeline"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("stdout"));
    }

    #[test]
    fn worker_exit_records_the_failed_pipeline_and_resumes() {
        let done = Mutex::new(Vec::new());
        let bad = Mutex::new(Vec::new());
        run_batches(
            &[7, 8, 9],
            1,
            |rest| match rest {
                [7, 8, 9] => fake_child("echo made 7 3.5 1&exit /b 7", Stdio::piped()),
                [9] => fake_child("echo made 9 2.0 0", Stdio::piped()),
                _ => panic!("unexpected batch {rest:?}"),
            },
            |n, ms, there| lock(&done).push((n, ms, there)),
            |n, why| lock(&bad).push((n, why)),
        )
        .unwrap();
        assert_eq!(*lock(&done), [(7, 3.5, true), (9, 2.0, false)]);
        let bad = lock(&bad);
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].0, 8);
        assert!(!bad[0].1.is_empty());
    }

    #[test]
    fn startup_failure_preserves_another_workers_completion() {
        // Hold the failed start until its peer has reported the only key in its chunk.
        let keys: Vec<_> = (0..=AT_ONCE).collect();
        let (confirmed, received) = std::sync::mpsc::channel();
        let received = Mutex::new(received);
        let error = run_batches(
            &keys,
            2,
            |chunk| {
                if chunk[0] == 0 {
                    assert_eq!(lock(&received).recv_timeout(Duration::from_secs(5)).unwrap(), AT_ONCE);
                    anyhow::bail!("worker unavailable")
                }
                fake_child(&format!("echo made {AT_ONCE} 1 1"), Stdio::piped())
            },
            |n, _, _| confirmed.send(n).unwrap(),
            |_, _| panic!("unexpected bad pipeline"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("worker unavailable"));
    }

    #[test]
    fn select_pipelines_skips_current_bad_and_retries_on_again_or_new_fingerprint() {
        let hex_key = |c: char| c.to_string().repeat(32);
        let (k_done, k_bad_current, k_bad_old, k_legacy_bad, k_fresh) = (
            hex_key('0'),
            hex_key('1'),
            hex_key('2'),
            hex_key('3'),
            hex_key('4'),
        );
        let keys = vec![
            k_done.clone(),
            k_bad_current.clone(),
            k_bad_old.clone(),
            k_legacy_bad.clone(),
            k_fresh.clone(),
        ];
        let done = HashSet::from([k_done.as_str()]);
        let early = HashSet::from([k_bad_current.as_str(), k_fresh.as_str()]);
        let current_entry = known::format_bad_entry(&k_bad_current, "1111111111111111", "exit code: 0xc0000005");
        assert_eq!(current_entry.split(' ').next(), Some(k_bad_current.as_str()));
        let old_entry = known::format_bad_entry(&k_bad_old, "0000000000000000", "hung for 180 s");
        let bad_text = format!(
            "# comment\n\
             {current_entry}\n\
             {old_entry}\n\
             {k_legacy_bad} legacy crash\n"
        );
        let bad = known::parse_bad_keys(&bad_text, "1111111111111111");
        assert_eq!(bad, HashSet::from([k_bad_current.clone(), k_legacy_bad]));

        // Early non-done/non-bad first (k_fresh = 4), then stale-fingerprint k_bad_old (2).
        assert_eq!(select_pipelines(&keys, &done, &bad, &early, false, false, None), vec![4, 2]);
        // --again retries done and bad pipelines (early indices 1, 4 first, then 0, 2, 3).
        assert_eq!(select_pipelines(&keys, &done, &bad, &early, true, false, None), vec![1, 4, 0, 2, 3]);
        assert_eq!(select_pipelines(&keys, &done, &bad, &early, true, true, None), vec![1, 4]);
        // Simulating VICTORY_WARM_BAD=0 clears bad while still honoring done.
        assert_eq!(
            select_pipelines(&keys, &done, &HashSet::new(), &early, false, false, None),
            vec![1, 4, 2, 3]
        );
    }
}
