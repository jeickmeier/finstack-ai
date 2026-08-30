//! Pure `ffmpeg` argument construction from a validated [`CompositionSpec`].
//!
//! Nothing in this module touches the filesystem or spawns a process: every
//! function here is a total computation over its inputs. The caller (a later
//! task) is responsible for staging clips to disk, probing their durations,
//! and actually invoking `ffmpeg` with the returned argument vector.
//!
//! `build_ffmpeg_args` is exercised only by this module's own tests until
//! `exec.rs` lands and wires it into `Toolset::call`; `dead_code` stays
//! muted here for that interim stretch (see `spec.rs` for the same note).
#![allow(dead_code)]

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::spec::{
    AudioMode, ClipSpec, CompositionSpec, Container, OutputSpec, SubtitleMode, TransitionKind,
};

/// One resolved clip input: its staged path and probed duration in seconds.
pub(crate) struct ClipInput {
    /// Absolute path to the staged clip file on disk.
    pub path: PathBuf,
    /// Probed duration of the clip, in seconds.
    pub duration_s: f64,
}

/// Render an `f64` the way `ffmpeg` filtergraph literals expect: `3.0`
/// renders as `3`, `3.5` renders as `3.5`. Rust's own `Display` impl for
/// `f64` already drops a trailing `.0`, so this is a thin, self-documenting
/// wrapper rather than custom formatting logic.
fn fmt_f64(v: f64) -> String {
    format!("{v}")
}

/// Escape `'` and `:` in a path so it can sit inside a single-quoted
/// `ffmpeg` filter option (as used by the `subtitles` filter).
fn escape_filter_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut escaped = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch == '\'' || ch == ':' {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Effective duration of one clip after any trim is applied.
fn clip_duration(clip: &ClipSpec, input: &ClipInput) -> f64 {
    clip.trim
        .as_ref()
        .map_or(input.duration_s, |trim| trim.end_s - trim.start_s)
}

/// Build the `[i:v]...[vi]` filter clause for one clip.
fn video_lane(idx: usize, clip: &ClipSpec, output: &OutputSpec) -> String {
    let mut lane = format!("[{idx}:v]");
    if let Some(trim) = &clip.trim {
        let start = fmt_f64(trim.start_s);
        let end = fmt_f64(trim.end_s);
        let _ = write!(lane, "trim=start={start}:end={end},setpts=PTS-STARTPTS,");
    }
    if let Some(resolution) = &output.resolution
        && let Some((width, height)) = resolution.split_once('x')
    {
        let _ = write!(lane, "scale={width}:{height},");
    }
    if let Some(fps) = output.fps {
        let _ = write!(lane, "fps={fps},");
    }
    let _ = write!(lane, "setsar=1[v{idx}]");
    lane
}

/// Build the `[i:a]...[ai]` filter clause for one clip.
fn audio_lane(idx: usize, clip: &ClipSpec) -> String {
    let mut lane = format!("[{idx}:a]");
    if let Some(trim) = &clip.trim {
        let start = fmt_f64(trim.start_s);
        let end = fmt_f64(trim.end_s);
        let _ = write!(lane, "atrim=start={start}:end={end},asetpts=PTS-STARTPTS,");
    }
    let _ = write!(lane, "anull[a{idx}]");
    lane
}

/// Build the ordered `ffmpeg` argument vector for one validated composition.
///
/// # Errors
///
/// Returns a stable, non-secret reason string when `clips` does not match
/// `spec.clips` in length, any effective clip duration is non-finite or
/// non-positive, or a required side input path (`audio`/`subtitles`) is
/// missing for a spec that declares one.
#[allow(clippy::too_many_lines)]
pub(crate) fn build_ffmpeg_args(
    spec: &CompositionSpec,
    clips: &[ClipInput],
    audio: Option<&Path>,
    subtitles: Option<&Path>,
    output: &Path,
) -> Result<Vec<OsString>, &'static str> {
    if clips.len() != spec.clips.len() {
        return Err("clip inputs must match the spec");
    }
    let durations: Vec<f64> = spec
        .clips
        .iter()
        .zip(clips)
        .map(|(clip_spec, input)| clip_duration(clip_spec, input))
        .collect();
    if durations.iter().any(|d| !d.is_finite() || *d <= 0.0) {
        return Err("clip inputs must match the spec");
    }
    if spec.subtitles.is_some() && subtitles.is_none() {
        return Err("subtitles path is required by the spec");
    }
    if spec.audio.is_some() && audio.is_none() {
        return Err("audio path is required by the spec");
    }

    let mut args: Vec<OsString> = ["-nostdin", "-y", "-hide_banner", "-loglevel", "error"]
        .into_iter()
        .map(OsString::from)
        .collect();

    for clip in clips {
        args.push(OsString::from("-i"));
        args.push(clip.path.as_os_str().to_os_string());
    }
    if let Some(audio_path) = audio {
        args.push(OsString::from("-i"));
        args.push(audio_path.as_os_str().to_os_string());
    }
    let mux_subtitles = matches!(
        spec.subtitles.as_ref().map(|s| s.mode),
        Some(SubtitleMode::Mux)
    );
    let subtitles_input_idx = clips.len() + usize::from(audio.is_some());
    if mux_subtitles && let Some(subs_path) = subtitles {
        args.push(OsString::from("-i"));
        args.push(subs_path.as_os_str().to_os_string());
    }

    let mut parts: Vec<String> = Vec::new();
    for (idx, clip_spec) in spec.clips.iter().enumerate() {
        parts.push(video_lane(idx, clip_spec, &spec.output));
        parts.push(audio_lane(idx, clip_spec));
    }

    match &spec.transitions {
        Some(transitions)
            if !transitions.is_empty()
                && transitions
                    .iter()
                    .any(|t| !matches!(t.kind, TransitionKind::Cut)) =>
        {
            let mut offset = durations[0];
            let mut prev_v = "v0".to_string();
            let mut prev_a = "a0".to_string();
            let last_idx = transitions.len() - 1;
            for (k, transition) in transitions.iter().enumerate() {
                let duration = if matches!(transition.kind, TransitionKind::Cut) {
                    0.01
                } else {
                    transition.duration_s.unwrap_or(0.5)
                };
                let name = if matches!(transition.kind, TransitionKind::FadeToBlack) {
                    "fadeblack"
                } else {
                    "fade"
                };
                let join_offset = offset - duration;
                let is_last = k == last_idx;
                let next_v = if is_last {
                    "vc".to_string()
                } else {
                    format!("vx{k}")
                };
                let next_a = if is_last {
                    "ac".to_string()
                } else {
                    format!("ax{k}")
                };
                let next_clip_idx = k + 1;
                let d = fmt_f64(duration);
                let o = fmt_f64(join_offset);
                parts.push(format!(
                    "[{prev_v}][v{next_clip_idx}]xfade=transition={name}:duration={d}:offset={o}[{next_v}]"
                ));
                parts.push(format!(
                    "[{prev_a}][a{next_clip_idx}]acrossfade=d={d}[{next_a}]"
                ));
                offset += durations[next_clip_idx] - duration;
                prev_v = next_v;
                prev_a = next_a;
            }
        }
        _ => {
            let mut concat_inputs = String::new();
            for idx in 0..clips.len() {
                let _ = write!(concat_inputs, "[v{idx}][a{idx}]");
            }
            let n = clips.len();
            parts.push(format!("{concat_inputs}concat=n={n}:v=1:a=1[vc][ac]"));
        }
    }

    let mut video_label = "vc".to_string();
    if let Some(subs) = &spec.subtitles
        && matches!(subs.mode, SubtitleMode::BurnIn)
    {
        let subs_path = subtitles.ok_or("subtitles path is required by the spec")?;
        let escaped = escape_filter_path(subs_path);
        let mut clause = format!("[vc]subtitles={escaped}");
        let mut style_parts = Vec::new();
        if let Some(style) = &subs.style {
            if let Some(font_size) = style.font_size {
                style_parts.push(format!("FontSize={font_size}"));
            }
            if let Some(margin_v) = style.margin_v {
                style_parts.push(format!("MarginV={margin_v}"));
            }
        }
        if !style_parts.is_empty() {
            let _ = write!(clause, ":force_style='{}'", style_parts.join(","));
        }
        clause.push_str("[vs]");
        parts.push(clause);
        video_label = "vs".to_string();
    }

    let mut audio_label = "ac".to_string();
    if let Some(audio_spec) = &spec.audio {
        let audio_idx = clips.len();
        let mut music_clause = format!("[{audio_idx}:a]");
        match audio_spec.gain_db {
            Some(gain) => {
                let _ = write!(music_clause, "volume={}dB", fmt_f64(gain));
            }
            None => music_clause.push_str("anull"),
        }
        music_clause.push_str("[music]");
        parts.push(music_clause);
        match audio_spec.mode {
            AudioMode::Replace => audio_label = "music".to_string(),
            AudioMode::Mix => {
                parts.push("[ac][music]amix=inputs=2:duration=first[am]".to_string());
                audio_label = "am".to_string();
            }
        }
    }

    args.push(OsString::from("-filter_complex"));
    args.push(OsString::from(parts.join(";")));

    args.push(OsString::from("-map"));
    args.push(OsString::from(format!("[{video_label}]")));
    args.push(OsString::from("-map"));
    args.push(OsString::from(format!("[{audio_label}]")));
    if mux_subtitles {
        args.push(OsString::from("-map"));
        args.push(OsString::from(format!("{subtitles_input_idx}:s")));
    }

    match spec.output.container {
        Container::Mp4 => {
            for flag in [
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
            ] {
                args.push(OsString::from(flag));
            }
            if mux_subtitles {
                args.push(OsString::from("-c:s"));
                args.push(OsString::from("mov_text"));
            }
        }
        Container::Webm => {
            for flag in ["-c:v", "libvpx-vp9", "-c:a", "libopus"] {
                args.push(OsString::from(flag));
            }
        }
    }

    if matches!(
        spec.audio.as_ref().map(|a| a.mode),
        Some(AudioMode::Replace)
    ) {
        args.push(OsString::from("-shortest"));
    }

    args.push(output.as_os_str().to_os_string());

    Ok(args)
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::{ArtifactId, ArtifactRef, BlobRef, Digest, Metadata};

    use super::*;
    use crate::spec::{AudioSpec, SubtitleStyle, SubtitlesSpec, TransitionSpec, TrimSpec};

    fn artifact_ref() -> ArtifactRef {
        let content = b"clip".as_slice();
        let digest = Digest::blob_content(content);
        let blob = BlobRef::try_new(
            "blob-1",
            "video/mp4",
            u64::try_from(content.len()).expect("length"),
            Some(digest),
            None::<&str>,
        )
        .expect("blob");
        ArtifactRef::try_new(
            ArtifactId::from_bytes([7; 16]),
            "video",
            blob,
            digest,
            Digest::raw_json(b"scope"),
            Metadata::empty(),
        )
        .expect("artifact")
    }

    fn minimal(clips: usize, transitions: Option<Vec<TransitionSpec>>) -> CompositionSpec {
        CompositionSpec {
            version: 1,
            clips: (0..clips)
                .map(|_| ClipSpec {
                    artifact: artifact_ref(),
                    trim: None,
                })
                .collect(),
            transitions,
            audio: None,
            subtitles: None,
            output: OutputSpec {
                container: Container::Mp4,
                resolution: None,
                fps: None,
            },
        }
    }

    fn clip(path: &str, duration_s: f64) -> ClipInput {
        ClipInput {
            path: PathBuf::from(path),
            duration_s,
        }
    }

    fn rendered(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn fmt_f64_drops_trailing_zero() {
        assert_eq!(fmt_f64(3.0), "3");
        assert_eq!(fmt_f64(3.5), "3.5");
    }

    #[test]
    fn cut_only_composition_uses_concat() {
        let spec = minimal(2, None);
        let clips = [clip("/in/a.mp4", 4.0), clip("/in/b.mp4", 6.0)];
        let args = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/out/movie.mp4"))
            .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.starts_with("-nostdin -y"));
        assert!(text.contains("-i /in/a.mp4"));
        assert!(text.contains("-i /in/b.mp4"));
        assert!(text.contains("concat=n=2:v=1:a=1"));
        assert!(text.ends_with("/out/movie.mp4"));
        assert!(!text.contains("xfade"));
    }

    #[test]
    fn crossfade_composition_chains_xfade_with_running_offsets() {
        let spec = minimal(
            3,
            Some(vec![
                TransitionSpec {
                    kind: TransitionKind::Crossfade,
                    duration_s: Some(1.0),
                },
                TransitionSpec {
                    kind: TransitionKind::Crossfade,
                    duration_s: Some(0.5),
                },
            ]),
        );
        let clips = [
            clip("/in/a.mp4", 4.0),
            clip("/in/b.mp4", 6.0),
            clip("/in/c.mp4", 5.0),
        ];
        let args = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/out/movie.mp4"))
            .expect("args");
        let text = rendered(&args).join(" ");
        // first xfade offset = 4.0 - 1.0; second = (4.0 + 6.0 - 1.0) - 0.5
        assert!(text.contains("xfade=transition=fade:duration=1:offset=3"));
        assert!(text.contains("xfade=transition=fade:duration=0.5:offset=8.5"));
        assert!(text.contains("acrossfade=d=1"));
    }

    #[test]
    fn trims_scale_fps_and_audio_are_encoded() {
        let mut spec = minimal(1, None);
        spec.clips[0].trim = Some(TrimSpec {
            start_s: 1.0,
            end_s: 3.5,
        });
        spec.output.resolution = Some("1280x720".into());
        spec.output.fps = Some(24);
        spec.audio = Some(AudioSpec {
            artifact: artifact_ref(),
            mode: AudioMode::Replace,
            gain_db: Some(-6.0),
        });
        let clips = [clip("/in/a.mp4", 10.0)];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            Some(Path::new("/in/music.mp3")),
            None,
            Path::new("/out/movie.mp4"),
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.contains("trim=start=1:end=3.5"));
        assert!(text.contains("scale=1280:720"));
        assert!(text.contains("fps=24"));
        assert!(text.contains("-i /in/music.mp3"));
        assert!(text.contains("volume=-6dB"));
        assert!(text.contains("-shortest"));
    }

    #[test]
    fn burned_in_subtitles_apply_the_clamped_style() {
        let mut spec = minimal(1, None);
        spec.subtitles = Some(SubtitlesSpec {
            artifact: artifact_ref(),
            mode: SubtitleMode::BurnIn,
            style: Some(SubtitleStyle {
                font_size: Some(42),
                margin_v: Some(80),
            }),
        });
        let clips = [clip("/in/a.mp4", 4.0)];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            None,
            Some(Path::new("/scratch/subs.srt")),
            Path::new("/out/movie.mp4"),
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.contains("subtitles=/scratch/subs.srt:force_style='FontSize=42,MarginV=80'"));
    }

    #[test]
    fn muxed_subtitles_add_an_input_and_the_mov_text_codec() {
        let mut spec = minimal(1, None);
        spec.subtitles = Some(SubtitlesSpec {
            artifact: artifact_ref(),
            mode: SubtitleMode::Mux,
            style: None,
        });
        let clips = [clip("/in/a.mp4", 4.0)];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            None,
            Some(Path::new("/scratch/subs.srt")),
            Path::new("/out/movie.mp4"),
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.contains("-i /scratch/subs.srt"));
        assert!(text.contains("-c:s mov_text"));
        assert!(!text.contains("subtitles="));
    }

    #[test]
    fn clip_count_and_duration_mismatch_is_rejected() {
        let spec = minimal(2, None);
        assert!(
            build_ffmpeg_args(
                &spec,
                &[clip("/in/a.mp4", 4.0)],
                None,
                None,
                Path::new("/o.mp4")
            )
            .is_err()
        );
    }
}
