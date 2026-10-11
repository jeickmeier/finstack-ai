//! Bounded process execution for `ffmpeg`/`ffprobe`, input container
//! sniffing, and `ffprobe` output parsing.
//!
//! Every process spawned here is raced against the caller's cancellation
//! signal and the call's [`Budget`]: nothing in this module ever blocks
//! indefinitely, and a cancelled or overrun child is killed rather than
//! leaked (`kill_on_drop`). Child output is drained concurrently with the
//! wait and retained only up to fixed ceilings, so a chatty child can
//! neither block on a full pipe nor grow process memory.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_runtime::ports::model::CancellationSignal;
use finstack_ai_runtime::ports::tool::ToolError;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::{
    VIDEO_COMPOSE_CONFIG_INVALID, VIDEO_COMPOSE_FFMPEG_FAILED, VIDEO_COMPOSE_MEDIA_FAILURE,
    timeout_error, tool_error,
};

/// Bound on the trailing `stderr` slice carried by a
/// [`VIDEO_COMPOSE_FFMPEG_FAILED`] message.
pub(crate) const STDERR_TAIL_BYTES: usize = 2 * 1024;

/// Bound on the `ffprobe` JSON report retained from `stdout`. Anything past
/// this is drained and discarded; a report this large is not a media probe.
const PROBE_STDOUT_BYTES: usize = 256 * 1024;

/// Read-buffer size for the output drains.
const DRAIN_CHUNK_BYTES: usize = 8 * 1024;

/// One wall-clock budget shared by every process a single tool call spawns.
///
/// The budget starts when it is created and never pauses: each probe and
/// the render itself race against [`Budget::remaining`], so a call can never
/// exceed its total even when it runs several children in sequence.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    started: Instant,
    total: Duration,
}

impl Budget {
    /// Start a budget of `total` from now.
    pub(crate) fn start(total: Duration) -> Self {
        Self {
            started: Instant::now(),
            total,
        }
    }

    /// Time left, saturating at zero once the budget is spent.
    pub(crate) fn remaining(&self) -> Duration {
        self.total.saturating_sub(self.started.elapsed())
    }
}

/// Input container pinned onto every `ffprobe`/`ffmpeg` `-i` so a staged
/// clip can only ever be demuxed as the container its bytes declare, never
/// as whatever `ffmpeg` would probe it to be.
///
/// The demuxer tokens (`mov`, `matroska`) are the documented names of the
/// `mov,mp4,m4a,3gp,3g2,mj2` and `matroska,webm` demuxers. They were not
/// verified against a local `ffmpeg` (none is installed where this crate
/// was written); CI should run `ffmpeg -demuxers` once against the pinned
/// build to confirm both tokens resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputContainer {
    /// ISO base media file (mp4, m4v, mov).
    Mp4,
    /// EBML/Matroska (mkv, webm).
    Matroska,
}

impl InputContainer {
    /// `-f` token selecting the matching demuxer.
    pub(crate) const fn demuxer(self) -> &'static str {
        match self {
            Self::Mp4 => "mov",
            Self::Matroska => "matroska",
        }
    }
}

/// Identify a clip's container from its leading bytes.
///
/// Recognises an ISO base media file by the `ftyp` box type at offset 4 and
/// an EBML stream by the `1A 45 DF A3` magic at offset 0. Anything else,
/// including inputs shorter than eight bytes, is `None` and must be rejected
/// before it reaches a demuxer.
pub(crate) fn sniff_container(bytes: &[u8]) -> Option<InputContainer> {
    if bytes.get(4..8) == Some(b"ftyp".as_slice()) {
        return Some(InputContainer::Mp4);
    }
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(InputContainer::Matroska);
    }
    None
}

/// Completed child process: exit status plus the retained slices of its
/// output.
struct ChildOutcome {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr_tail: Vec<u8>,
}

/// Spawn one child and wait for it within `budget` and `cancellation`.
///
/// `stdout` is captured (bounded to [`PROBE_STDOUT_BYTES`]) only when
/// `capture_stdout` is set; otherwise it is discarded at the OS level.
/// `stderr` is always drained concurrently with the wait and only its last
/// [`STDERR_TAIL_BYTES`] are kept.
///
/// # Errors
///
/// [`crate::VIDEO_COMPOSE_TIMEOUT`] when the budget is already spent, the
/// budget runs out, or `cancellation` fires (the child is killed);
/// [`VIDEO_COMPOSE_CONFIG_INVALID`] when the binary cannot be spawned;
/// [`VIDEO_COMPOSE_FFMPEG_FAILED`] when the wait itself fails after the
/// child started.
async fn run_child(
    binary: &Path,
    args: &[OsString],
    budget: &Budget,
    cancellation: &CancellationSignal,
    capture_stdout: bool,
) -> Result<ChildOutcome, ToolError> {
    let remaining = budget.remaining();
    if remaining.is_zero() || cancellation.is_cancelled() {
        return Err(timeout_error());
    }

    let mut command = tokio::process::Command::new(binary);
    command.args(args);
    command.kill_on_drop(true);
    command.stdin(Stdio::null());
    command.stdout(if capture_stdout {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|_| {
        tool_error(
            VIDEO_COMPOSE_CONFIG_INVALID,
            ErrorCategory::Configuration,
            "compose binary failed to start",
        )
    })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let outcome = async {
        let (status, stdout, stderr_tail) = tokio::join!(
            child.wait(),
            async {
                match stdout {
                    Some(reader) => read_prefix(reader, PROBE_STDOUT_BYTES).await,
                    None => Vec::new(),
                }
            },
            async {
                match stderr {
                    Some(reader) => read_tail(reader, STDERR_TAIL_BYTES).await,
                    None => Vec::new(),
                }
            },
        );
        status.map(|status| ChildOutcome {
            status,
            stdout,
            stderr_tail,
        })
    };

    tokio::select! {
        result = outcome => result.map_err(|_| tool_error(
            VIDEO_COMPOSE_FFMPEG_FAILED,
            ErrorCategory::Tool,
            "compose binary exited abnormally",
        )),
        () = cancellation.cancelled() => Err(timeout_error()),
        () = tokio::time::sleep(remaining) => Err(timeout_error()),
    }
}

/// Keep the first `limit` bytes of `reader` and drain the rest.
async fn read_prefix<R: AsyncRead + Unpin>(mut reader: R, limit: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut chunk = vec![0_u8; DRAIN_CHUNK_BYTES];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = limit.saturating_sub(kept.len());
                let take = room.min(n);
                if let Some(slice) = chunk.get(..take) {
                    kept.extend_from_slice(slice);
                }
            }
        }
    }
    kept
}

/// Keep the last `limit` bytes of `reader`, draining everything before them.
async fn read_tail<R: AsyncRead + Unpin>(mut reader: R, limit: usize) -> Vec<u8> {
    let mut tail: Vec<u8> = Vec::with_capacity(limit.saturating_mul(2));
    let mut chunk = vec![0_u8; DRAIN_CHUNK_BYTES];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Some(slice) = chunk.get(..n) {
                    tail.extend_from_slice(slice);
                }
                if tail.len() > limit {
                    let excess = tail.len() - limit;
                    tail.drain(..excess);
                }
            }
        }
    }
    tail
}

/// Run one bounded `ffmpeg` render.
///
/// `stdout` is discarded; only the trailing [`STDERR_TAIL_BYTES`] of
/// `stderr` are retained, and only to describe a failure. A render runs
/// behind the compose tool's approval gate, so that tail may be surfaced.
///
/// # Errors
///
/// Propagates [`run_child`] failures, and yields
/// [`VIDEO_COMPOSE_FFMPEG_FAILED`] carrying the `stderr` tail on a non-zero
/// exit.
pub(crate) async fn render(
    binary: &Path,
    args: &[OsString],
    budget: &Budget,
    cancellation: &CancellationSignal,
) -> Result<(), ToolError> {
    let outcome = run_child(binary, args, budget, cancellation, false).await?;
    if !outcome.status.success() {
        return Err(ffmpeg_failure(&outcome.stderr_tail));
    }
    Ok(())
}

fn ffmpeg_failure(stderr: &[u8]) -> ToolError {
    let text = String::from_utf8_lossy(stderr);
    let tail: String = text
        .chars()
        .rev()
        .scan(0_usize, |bytes, ch| {
            *bytes += ch.len_utf8();
            if *bytes > STDERR_TAIL_BYTES {
                None
            } else {
                Some(ch)
            }
        })
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    tool_error(VIDEO_COMPOSE_FFMPEG_FAILED, ErrorCategory::Tool, tail)
}

/// Probed media summary.
#[derive(Debug)]
pub(crate) struct ProbeResult {
    /// Container duration in seconds.
    pub duration_s: f64,
    /// Video stream width in pixels, when a video stream is present.
    pub width: Option<u32>,
    /// Video stream height in pixels, when a video stream is present.
    pub height: Option<u32>,
    /// Video stream frame rate, when parseable.
    pub fps: Option<f64>,
    /// Whether any stream is an audio stream.
    pub has_audio: bool,
}

#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    format: Option<FfprobeFormat>,
    #[serde(default)]
    streams: Vec<FfprobeStream>,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    #[serde(default)]
    codec_type: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    avg_frame_rate: Option<String>,
}

/// Probe one media file with `ffprobe`, pinned to `container`'s demuxer,
/// and parse its JSON report.
///
/// Probe failures never carry `ffprobe`'s `stderr`: the probe tool runs
/// without approval, and a demuxer's diagnostics would otherwise act as a
/// path-existence oracle for the host filesystem.
///
/// # Errors
///
/// Propagates [`run_child`] failures; yields [`VIDEO_COMPOSE_FFMPEG_FAILED`]
/// with a fixed message on a non-zero exit, and
/// [`VIDEO_COMPOSE_MEDIA_FAILURE`] when `ffprobe`'s JSON is malformed or
/// omits a parseable duration.
pub(crate) async fn probe(
    ffprobe: &Path,
    target: &Path,
    container: InputContainer,
    budget: &Budget,
    cancellation: &CancellationSignal,
) -> Result<ProbeResult, ToolError> {
    let args: Vec<OsString> = [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-f",
        container.demuxer(),
    ]
    .into_iter()
    .map(OsString::from)
    .chain(std::iter::once(target.as_os_str().to_os_string()))
    .collect();

    let outcome = run_child(ffprobe, &args, budget, cancellation, true).await?;
    if !outcome.status.success() {
        return Err(tool_error(
            VIDEO_COMPOSE_FFMPEG_FAILED,
            ErrorCategory::Tool,
            "probe process failed",
        ));
    }
    let parsed: FfprobeOutput = serde_json::from_slice(&outcome.stdout).map_err(|_| {
        tool_error(
            VIDEO_COMPOSE_MEDIA_FAILURE,
            ErrorCategory::Tool,
            "ffprobe output is invalid",
        )
    })?;

    let duration_s = parsed
        .format
        .as_ref()
        .and_then(|format| format.duration.as_ref())
        .and_then(|duration| duration.parse::<f64>().ok())
        .filter(|duration| duration.is_finite())
        .ok_or_else(|| {
            tool_error(
                VIDEO_COMPOSE_MEDIA_FAILURE,
                ErrorCategory::Tool,
                "ffprobe output omits a parseable duration",
            )
        })?;

    let video_stream = parsed
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"));
    let has_audio = parsed
        .streams
        .iter()
        .any(|stream| stream.codec_type.as_deref() == Some("audio"));
    let width = video_stream.and_then(|stream| stream.width);
    let height = video_stream.and_then(|stream| stream.height);
    let fps = video_stream
        .and_then(|stream| stream.avg_frame_rate.as_ref())
        .and_then(|rate| parse_frame_rate(rate));

    Ok(ProbeResult {
        duration_s,
        width,
        height,
        fps,
        has_audio,
    })
}

fn parse_frame_rate(raw: &str) -> Option<f64> {
    let (num, den) = raw.split_once('/')?;
    let num: f64 = num.parse().ok()?;
    let den: f64 = den.parse().ok()?;
    if den == 0.0 || !num.is_finite() || !den.is_finite() {
        None
    } else {
        Some(num / den)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use finstack_ai_runtime::ports::model::CancellationSignal;
    use tempfile::tempdir;

    use super::*;

    fn write_stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("stub");
        let mut permissions = std::fs::metadata(&path).expect("meta").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("chmod");
        path
    }

    fn budget(total: Duration) -> Budget {
        Budget::start(total)
    }

    /// A budget whose total was consumed long ago.
    fn spent_budget() -> Budget {
        Budget {
            started: Instant::now()
                .checked_sub(Duration::from_secs(10))
                .expect("recent instant"),
            total: Duration::from_secs(1),
        }
    }

    #[test]
    fn container_sniff_detects_mp4_and_webm() {
        let mp4 = [
            0, 0, 0, 0x18, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm',
        ];
        assert_eq!(sniff_container(&mp4), Some(InputContainer::Mp4));
        let webm = [0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81];
        assert_eq!(sniff_container(&webm), Some(InputContainer::Matroska));
        assert_eq!(sniff_container(b"RIFF....WAVE"), None);
        assert_eq!(sniff_container(b"ftyp"), None, "too short");
        assert_eq!(sniff_container(b""), None);
        assert_eq!(InputContainer::Mp4.demuxer(), "mov");
        assert_eq!(InputContainer::Matroska.demuxer(), "matroska");
    }

    #[test]
    fn budget_remaining_saturates_at_zero() {
        let spent = spent_budget();
        assert_eq!(spent.remaining(), Duration::ZERO);
        let fresh = budget(Duration::from_mins(1));
        assert!(fresh.remaining() > Duration::from_secs(59));
    }

    #[tokio::test]
    async fn render_surfaces_bounded_stderr_on_nonzero_exit() {
        let dir = tempdir().expect("dir");
        let stub = write_stub(
            dir.path(),
            "ffmpeg",
            "echo \"boom: filter parse error\" >&2; exit 1",
        );
        let error = render(
            &stub,
            &[],
            &budget(Duration::from_secs(5)),
            &CancellationSignal::new(),
        )
        .await
        .expect_err("nonzero exit");
        assert_eq!(error.code(), crate::VIDEO_COMPOSE_FFMPEG_FAILED);
        assert!(error.message().contains("filter parse error"));
        assert!(error.message().len() <= STDERR_TAIL_BYTES);
    }

    #[tokio::test]
    async fn stderr_tail_is_bounded() {
        let dir = tempdir().expect("dir");
        // 1 MiB of stderr, far past the pipe buffer: the drain must run
        // concurrently with the wait or the child blocks forever.
        let stub = write_stub(
            dir.path(),
            "ffmpeg",
            "head -c 1048576 /dev/zero | tr '\\0' 'a' >&2; printf 'TAIL-MARKER' >&2; exit 1",
        );
        let error = render(
            &stub,
            &[],
            &budget(Duration::from_secs(10)),
            &CancellationSignal::new(),
        )
        .await
        .expect_err("nonzero exit");
        assert_eq!(error.code(), crate::VIDEO_COMPOSE_FFMPEG_FAILED);
        assert!(
            error.message().len() <= STDERR_TAIL_BYTES,
            "{}",
            error.message().len()
        );
        assert!(error.message().ends_with("TAIL-MARKER"));
    }

    #[tokio::test]
    async fn render_times_out_a_slow_child() {
        let dir = tempdir().expect("dir");
        let stub = write_stub(dir.path(), "ffmpeg", "exec sleep 30");
        let error = render(
            &stub,
            &[],
            &budget(Duration::from_millis(200)),
            &CancellationSignal::new(),
        )
        .await
        .expect_err("timeout");
        assert_eq!(error.code(), crate::VIDEO_COMPOSE_TIMEOUT);
    }

    #[tokio::test]
    async fn spent_budget_fails_before_spawn() {
        let dir = tempdir().expect("dir");
        let stub = write_stub(dir.path(), "ffmpeg", "touch \"$(dirname \"$0\")/ran\"");
        let error = render(&stub, &[], &spent_budget(), &CancellationSignal::new())
            .await
            .expect_err("timeout");
        assert_eq!(error.code(), crate::VIDEO_COMPOSE_TIMEOUT);
        assert!(!dir.path().join("ran").exists(), "nothing may spawn");
    }

    #[tokio::test]
    async fn probe_maps_ffprobe_json_and_pins_the_demuxer() {
        let dir = tempdir().expect("dir");
        let stub = write_stub(
            dir.path(),
            "ffprobe",
            r#"d=$(dirname "$0"); printf '%s\n' "$@" > "$d/ffprobe-args.txt"; printf '%s' '{"format":{"duration":"4.0"},"streams":[{"codec_type":"video","width":640,"height":360,"avg_frame_rate":"24/1"},{"codec_type":"audio"}]}'"#,
        );
        let target = dir.path().join("clip.mp4");
        std::fs::write(&target, b"x").expect("clip");
        let result = probe(
            &stub,
            &target,
            InputContainer::Matroska,
            &budget(Duration::from_secs(5)),
            &CancellationSignal::new(),
        )
        .await
        .expect("probe");
        assert!((result.duration_s - 4.0).abs() < f64::EPSILON);
        assert_eq!(result.width, Some(640));
        assert_eq!(result.height, Some(360));
        assert!((result.fps.expect("fps") - 24.0).abs() < f64::EPSILON);
        assert!(result.has_audio);
        let args = std::fs::read_to_string(dir.path().join("ffprobe-args.txt")).expect("args");
        let lines: Vec<&str> = args.lines().collect();
        let f = lines.iter().position(|l| *l == "-f").expect("-f present");
        assert_eq!(lines[f + 1], "matroska");
        assert!(
            f + 2 < lines.len() && lines[f + 2].ends_with("clip.mp4"),
            "-f precedes the input"
        );
    }

    #[tokio::test]
    async fn probe_error_omits_stderr() {
        let dir = tempdir().expect("dir");
        let stub = write_stub(
            dir.path(),
            "ffprobe",
            "echo \"/secret/host/path/clip.mp4: No such file or directory\" >&2; exit 1",
        );
        let target = dir.path().join("clip.mp4");
        let error = probe(
            &stub,
            &target,
            InputContainer::Mp4,
            &budget(Duration::from_secs(5)),
            &CancellationSignal::new(),
        )
        .await
        .expect_err("probe failure");
        assert_eq!(error.code(), crate::VIDEO_COMPOSE_FFMPEG_FAILED);
        assert!(!error.message().contains("secret"));
        assert!(!error.message().contains("No such file"));
        assert_eq!(error.message(), "probe process failed");
    }

    #[tokio::test]
    async fn probe_stdout_is_bounded_and_fully_drained() {
        let dir = tempdir().expect("dir");
        // Valid JSON first, then far more than PROBE_STDOUT_BYTES of junk: the
        // retained prefix is invalid JSON (bounded), and the child must not
        // block on a full pipe.
        let stub = write_stub(
            dir.path(),
            "ffprobe",
            "printf '{}'; head -c 1048576 /dev/zero | tr '\\0' 'a'; exit 0",
        );
        let target = dir.path().join("clip.mp4");
        let error = probe(
            &stub,
            &target,
            InputContainer::Mp4,
            &budget(Duration::from_secs(10)),
            &CancellationSignal::new(),
        )
        .await
        .expect_err("junk after the JSON is invalid");
        assert_eq!(error.code(), crate::VIDEO_COMPOSE_MEDIA_FAILURE);
    }
}
