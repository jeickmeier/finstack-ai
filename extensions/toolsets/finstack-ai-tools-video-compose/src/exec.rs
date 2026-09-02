//! Bounded process execution for `ffmpeg`/`ffprobe`, and `ffprobe` output
//! parsing.
//!
//! Every process spawned here is raced against the caller's cancellation
//! signal and a wall-clock timeout: nothing in this module ever blocks
//! indefinitely, and a cancelled or overrun child is killed rather than
//! leaked (`kill_on_drop`).

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_runtime::ports::model::CancellationSignal;
use finstack_ai_runtime::ports::tool::ToolError;
use serde::Deserialize;

use crate::{
    VIDEO_COMPOSE_CONFIG_INVALID, VIDEO_COMPOSE_FFMPEG_FAILED, VIDEO_COMPOSE_MEDIA_FAILURE,
    timeout_error, tool_error,
};

/// Bound on the trailing `stderr` slice carried by a
/// [`VIDEO_COMPOSE_FFMPEG_FAILED`] message.
const STDERR_TAIL_BYTES: usize = 2 * 1024;

/// Run one bounded child process and return its completed output.
///
/// Races the child's exit against `cancellation` and `timeout`; either one
/// firing first kills the child (`kill_on_drop`) and yields
/// [`crate::VIDEO_COMPOSE_TIMEOUT`]. A spawn failure yields
/// [`VIDEO_COMPOSE_CONFIG_INVALID`]. A non-zero exit yields
/// [`VIDEO_COMPOSE_FFMPEG_FAILED`] carrying the trailing bytes of `stderr`.
///
/// # Errors
///
/// See above.
pub(crate) async fn run_bounded(
    binary: &Path,
    args: &[OsString],
    timeout: Duration,
    cancellation: &CancellationSignal,
) -> Result<std::process::Output, ToolError> {
    let mut command = tokio::process::Command::new(binary);
    command.args(args);
    command.kill_on_drop(true);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let child = command.spawn().map_err(|_| {
        tool_error(
            VIDEO_COMPOSE_CONFIG_INVALID,
            ErrorCategory::Configuration,
            "compose binary failed to start",
        )
    })?;

    let output = tokio::select! {
        result = child.wait_with_output() => result.map_err(|_| tool_error(
            VIDEO_COMPOSE_CONFIG_INVALID,
            ErrorCategory::Configuration,
            "compose binary failed to start",
        ))?,
        () = cancellation.cancelled() => return Err(timeout_error()),
        () = tokio::time::sleep(timeout) => return Err(timeout_error()),
    };

    if !output.status.success() {
        return Err(ffmpeg_failure(&output.stderr));
    }
    Ok(output)
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

/// Probe one media file with `ffprobe` and parse its JSON report.
///
/// # Errors
///
/// Propagates [`run_bounded`] failures, and yields
/// [`VIDEO_COMPOSE_MEDIA_FAILURE`] when `ffprobe`'s JSON is malformed or omits
/// a parseable duration.
pub(crate) async fn probe(
    ffprobe: &Path,
    target: &Path,
    timeout: Duration,
    cancellation: &CancellationSignal,
) -> Result<ProbeResult, ToolError> {
    let args: Vec<OsString> = [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ]
    .into_iter()
    .map(OsString::from)
    .chain(std::iter::once(target.as_os_str().to_os_string()))
    .collect();

    let output = run_bounded(ffprobe, &args, timeout, cancellation).await?;
    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout).map_err(|_| {
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
    if den == 0.0 { None } else { Some(num / den) }
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

    #[tokio::test]
    async fn run_bounded_surfaces_bounded_stderr_on_nonzero_exit() {
        let dir = tempdir().expect("dir");
        let stub = write_stub(
            dir.path(),
            "ffmpeg",
            "echo \"boom: filter parse error\" >&2; exit 1",
        );
        let error = run_bounded(
            &stub,
            &[],
            Duration::from_secs(5),
            &CancellationSignal::new(),
        )
        .await
        .expect_err("nonzero exit");
        assert_eq!(error.code(), crate::VIDEO_COMPOSE_FFMPEG_FAILED);
        assert!(error.message().contains("filter parse error"));
        assert!(error.message().len() <= STDERR_TAIL_BYTES);
    }

    #[tokio::test]
    async fn run_bounded_times_out_a_slow_child() {
        let dir = tempdir().expect("dir");
        let stub = write_stub(dir.path(), "ffmpeg", "sleep 30");
        let error = run_bounded(
            &stub,
            &[],
            Duration::from_millis(200),
            &CancellationSignal::new(),
        )
        .await
        .expect_err("timeout");
        assert_eq!(error.code(), crate::VIDEO_COMPOSE_TIMEOUT);
    }

    #[tokio::test]
    async fn probe_maps_ffprobe_json() {
        let dir = tempdir().expect("dir");
        let stub = write_stub(
            dir.path(),
            "ffprobe",
            r#"printf '%s' '{"format":{"duration":"4.0"},"streams":[{"codec_type":"video","width":640,"height":360,"avg_frame_rate":"24/1"},{"codec_type":"audio"}]}'"#,
        );
        let target = dir.path().join("clip.mp4");
        std::fs::write(&target, b"x").expect("clip");
        let result = probe(
            &stub,
            &target,
            Duration::from_secs(5),
            &CancellationSignal::new(),
        )
        .await
        .expect("probe");
        assert!((result.duration_s - 4.0).abs() < f64::EPSILON);
        assert_eq!(result.width, Some(640));
        assert!((result.fps.expect("fps") - 24.0).abs() < f64::EPSILON);
        assert!(result.has_audio);
    }
}
