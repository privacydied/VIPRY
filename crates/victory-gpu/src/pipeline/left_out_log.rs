//! One log line for each pipeline the layer leaves out, with the hashes of its
//! programs and the entry names their containers carry, so that a hash list
//! (`VICTORY_LEAVE_OUT_FILE`) can be split by the pass it belongs to.
//! Target `victory_gpu::pipeline::left_out`, info: off unless a run's
//! `VICTORY_LOG` names it.

use super::Shader;
use std::sync::Arc;

/// Logs `shaders`, left out for `why`.
pub(super) fn note(shaders: &[Arc<Shader>], why: &str) {
    if !tracing::enabled!(target: "victory_gpu::pipeline::left_out", tracing::Level::INFO) {
        return;
    }
    let programs: Vec<String> = shaders
        .iter()
        .map(|s| format!("{} {} [{}]", s.stage, s.hash, names(&s.container).join(" ")))
        .collect();
    tracing::info!(target: "victory_gpu::pipeline::left_out", why, programs = %programs.join(", "), "pipeline left out");
}

/// The entry-like names in a container: runs of 6 to 64 ASCII letters,
/// digits and `_` that start with a known prefix (`CS_`, `PS_`, `VS_`, ...),
/// each once, in order.
pub(super) fn names(container: &[u8]) -> Vec<String> {
    const PREFIXES: [&[u8]; 6] = [b"CS_", b"PS_", b"VS_", b"MS_", b"AS_", b"GS_"];
    let mut out: Vec<String> = Vec::new();
    for run in container.split(|b| !(b.is_ascii_alphanumeric() || *b == b'_')) {
        if (6..=64).contains(&run.len())
            && PREFIXES.iter().any(|p| run.starts_with(p))
            && let Ok(name) = std::str::from_utf8(run)
            && !out.iter().any(|n| n == name)
        {
            out.push(name.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_are_entry_like_runs_with_a_stage_prefix() {
        let c = b"\0\x01CS_RaytracingResolve\0junk CS_x PS_Shadow_Trace2\0CS_RaytracingResolve";
        assert_eq!(super::names(c), ["CS_RaytracingResolve", "PS_Shadow_Trace2"]);
        assert!(super::names(b"no names here").is_empty());
    }
}
