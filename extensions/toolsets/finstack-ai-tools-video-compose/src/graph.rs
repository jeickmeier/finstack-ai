//! Pure `ffmpeg` argument construction from a validated [`CompositionSpec`].
//!
//! Nothing in this module touches the filesystem or spawns a process: every
//! function here is a total computation over its inputs. The caller
//! (`lib.rs::call`) stages clips to disk, probes their durations, and
//! invokes `ffmpeg` with the returned argument vector.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::exec::InputContainer;
use crate::spec::{
    AudioMode, ClipSpec, CompositionSpec, Container, SubtitleMode, TransitionKind, TransitionSpec,
};

/// Effective duration `ffmpeg` is given for a `cut` inside an `xfade` chain.
const CUT_XFADE_SECONDS: f64 = 0.01;

/// One resolved clip input: its staged path, sniffed container, and probe.
pub(crate) struct ClipInput {
    /// Absolute path to the staged clip file on disk.
    pub path: PathBuf,
    /// Container sniffed from the staged bytes; pinned onto `-i` via `-f`.
    pub container: InputContainer,
    /// Probed duration of the clip, in seconds.
    pub duration_s: f64,
    /// Probed video width in pixels, when a video stream was found.
    pub width: Option<u32>,
    /// Probed video height in pixels, when a video stream was found.
    pub height: Option<u32>,
    /// Probed video frame rate, when parseable.
    pub fps: Option<f64>,
    /// Whether the probe found an audio stream on this clip. A clip without
    /// one gets a synthesized silent lane instead of an `[i:a]` reference:
    /// referencing a stream that does not exist makes `ffmpeg` fail the whole
    /// render, and the concat/xfade folds downstream require one audio label
    /// per clip.
    pub has_audio: bool,
}

/// Resolved per-lane video normalisation: the spec's explicit `resolution`
/// and `fps`, falling back to clip 0's probe so every lane shares one
/// geometry and rate (`concat` and `xfade` both require it).
struct VideoFormat {
    scale: Option<(u32, u32)>,
    fps: Option<String>,
}

impl VideoFormat {
    fn resolve(spec: &CompositionSpec, first: &ClipInput) -> Self {
        let scale = spec
            .output
            .resolution
            .as_deref()
            .and_then(|resolution| resolution.split_once('x'))
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .or_else(|| Some((first.width?, first.height?)));
        let fps = spec.output.fps.map(|fps| fps.to_string()).or_else(|| {
            first
                .fps
                .filter(|fps| fps.is_finite() && *fps > 0.0)
                .map(fmt_f64)
        });
        Self { scale, fps }
    }
}

/// Render an `f64` the way `ffmpeg` filtergraph literals expect: `3.0`
/// renders as `3`, `3.5` renders as `3.5`. Rust's own `Display` impl for
/// `f64` already drops a trailing `.0`, so this is a thin, self-documenting
/// wrapper rather than custom formatting logic.
fn fmt_f64(v: f64) -> String {
    format!("{v}")
}

/// Escape `\`, `'` and `:` in a path so it can sit inside an `ffmpeg`
/// filter option (as used by the `subtitles` filter). The scratch root is
/// restricted to `[A-Za-z0-9/_.-]` at construction and every file name under
/// it is toolset-generated, so these three are the only escapes a staged
/// path can ever need.
fn escape_filter_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut escaped = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch == '\\' || ch == '\'' || ch == ':' {
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

/// Effective duration `ffmpeg` is given for one transition in an `xfade`
/// chain.
fn transition_duration(kind: TransitionKind, duration_s: Option<f64>) -> f64 {
    if matches!(kind, TransitionKind::Cut) {
        CUT_XFADE_SECONDS
    } else {
        duration_s.unwrap_or(0.5)
    }
}

/// Check each clip's trim against its probed duration and each transition
/// against the clips it joins. These bounds only exist once the clips have
/// been probed, so they cannot live in `validate_spec`.
fn validate_against_probes(
    spec: &CompositionSpec,
    clips: &[ClipInput],
    durations: &[f64],
) -> Result<(), &'static str> {
    for (clip_spec, input) in spec.clips.iter().zip(clips) {
        if !input.duration_s.is_finite() || input.duration_s <= 0.0 {
            return Err("probed clip duration must be positive");
        }
        if let Some(trim) = &clip_spec.trim {
            if trim.start_s >= trim.end_s {
                return Err("clip trim must satisfy 0 <= start < end");
            }
            if trim.end_s > input.duration_s {
                return Err("clip trim exceeds the probed clip duration");
            }
        }
    }
    if durations.iter().any(|d| !d.is_finite() || *d <= 0.0) {
        return Err("clip inputs must match the spec");
    }
    let Some(transitions) = spec.transitions.as_ref().filter(|t| !t.is_empty()) else {
        return Ok(());
    };
    let mut offset = durations.first().copied().unwrap_or(0.0);
    for (k, transition) in transitions.iter().enumerate() {
        let duration = transition_duration(transition.kind, transition.duration_s);
        let (Some(before), Some(after)) = (durations.get(k), durations.get(k + 1)) else {
            return Err("transition count must be clip count minus one");
        };
        if duration > *before || duration > *after {
            return Err("transition is longer than an adjacent clip");
        }
        // Defensive: with every transition bounded by both neighbours the
        // running offset cannot go negative, but a non-finite or negative
        // `xfade` offset must never reach `ffmpeg` regardless.
        let join_offset = offset - duration;
        if !join_offset.is_finite() || join_offset < 0.0 {
            return Err("transition offset is negative");
        }
        offset += after - duration;
    }
    Ok(())
}

/// Build the `[i:v]...[vi]` filter clause for one clip.
fn video_lane(idx: usize, clip: &ClipSpec, format: &VideoFormat) -> String {
    let mut lane = format!("[{idx}:v]");
    if let Some(trim) = &clip.trim {
        let start = fmt_f64(trim.start_s);
        let end = fmt_f64(trim.end_s);
        let _ = write!(lane, "trim=start={start}:end={end},setpts=PTS-STARTPTS,");
    }
    if let Some((width, height)) = format.scale {
        let _ = write!(lane, "scale={width}:{height},");
    }
    if let Some(fps) = &format.fps {
        let _ = write!(lane, "fps={fps},");
    }
    let _ = write!(lane, "setsar=1[v{idx}]");
    lane
}

/// Build the `[i:a]...[ai]` filter clause for one clip, or a synthesized
/// silent lane of the clip's effective duration when the clip has no audio
/// stream to reference.
fn audio_lane(idx: usize, clip: &ClipSpec, input: &ClipInput) -> String {
    if !input.has_audio {
        let duration = fmt_f64(clip_duration(clip, input));
        return format!(
            "anullsrc=channel_layout=stereo:sample_rate=48000,atrim=duration={duration}[a{idx}]"
        );
    }
    let mut lane = format!("[{idx}:a]");
    if let Some(trim) = &clip.trim {
        let start = fmt_f64(trim.start_s);
        let end = fmt_f64(trim.end_s);
        let _ = write!(lane, "atrim=start={start}:end={end},asetpts=PTS-STARTPTS,");
    }
    let _ = write!(lane, "anull[a{idx}]");
    lane
}

/// Leading global flags plus one `-f <demuxer> -i <clip>` pair per clip,
/// then the optional music and muxed-subtitle inputs, in input-index order.
fn input_args(
    clips: &[ClipInput],
    audio: Option<&Path>,
    muxed_subtitles: Option<&Path>,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-nostdin", "-y", "-hide_banner", "-loglevel", "error"]
        .into_iter()
        .map(OsString::from)
        .collect();
    for clip in clips {
        args.push(OsString::from("-f"));
        args.push(OsString::from(clip.container.demuxer()));
        args.push(OsString::from("-i"));
        args.push(clip.path.as_os_str().to_os_string());
    }
    for path in [audio, muxed_subtitles].into_iter().flatten() {
        args.push(OsString::from("-i"));
        args.push(path.as_os_str().to_os_string());
    }
    args
}

/// Join every `[vi][ai]` lane into `[vc][ac]`: an `xfade`/`acrossfade` chain
/// when any transition is visible, otherwise a single `concat`.
fn join_clauses(
    transitions: Option<&[TransitionSpec]>,
    durations: &[f64],
    clip_count: usize,
) -> Vec<String> {
    match transitions {
        Some(transitions)
            if transitions
                .iter()
                .any(|t| !matches!(t.kind, TransitionKind::Cut)) =>
        {
            xfade_chain(transitions, durations)
        }
        _ => {
            let mut concat_inputs = String::new();
            for idx in 0..clip_count {
                let _ = write!(concat_inputs, "[v{idx}][a{idx}]");
            }
            vec![format!(
                "{concat_inputs}concat=n={clip_count}:v=1:a=1[vc][ac]"
            )]
        }
    }
}

/// Fold the clip lanes pairwise through `xfade`/`acrossfade`, carrying the
/// running output offset so each join starts `duration` before the end.
fn xfade_chain(transitions: &[TransitionSpec], durations: &[f64]) -> Vec<String> {
    let mut parts = Vec::with_capacity(transitions.len() * 2);
    let mut offset = durations.first().copied().unwrap_or(0.0);
    let mut prev_v = "v0".to_string();
    let mut prev_a = "a0".to_string();
    let last_idx = transitions.len().saturating_sub(1);
    for (k, transition) in transitions.iter().enumerate() {
        let duration = transition_duration(transition.kind, transition.duration_s);
        let name = if matches!(transition.kind, TransitionKind::FadeToBlack) {
            "fadeblack"
        } else {
            "fade"
        };
        let join_offset = offset - duration;
        let (next_v, next_a) = if k == last_idx {
            ("vc".to_string(), "ac".to_string())
        } else {
            (format!("vx{k}"), format!("ax{k}"))
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
        offset += durations.get(next_clip_idx).copied().unwrap_or(0.0) - duration;
        prev_v = next_v;
        prev_a = next_a;
    }
    parts
}

/// Append the burn-in `subtitles` clause when the spec asks for one and
/// return the label of the final video stream.
fn burn_in_subtitles(
    spec: &CompositionSpec,
    subtitles: Option<&Path>,
    parts: &mut Vec<String>,
) -> Result<&'static str, &'static str> {
    let Some(subs) = spec
        .subtitles
        .as_ref()
        .filter(|subs| matches!(subs.mode, SubtitleMode::BurnIn))
    else {
        return Ok("vc");
    };
    let subs_path = subtitles.ok_or("subtitles path is required by the spec")?;
    let escaped = escape_filter_path(subs_path);
    let mut clause = format!("[vc]subtitles={escaped}");
    let style_parts: Vec<String> = subs
        .style
        .iter()
        .flat_map(|style| {
            [
                style.font_size.map(|v| format!("FontSize={v}")),
                style.margin_v.map(|v| format!("MarginV={v}")),
            ]
        })
        .flatten()
        .collect();
    if !style_parts.is_empty() {
        let _ = write!(clause, ":force_style='{}'", style_parts.join(","));
    }
    clause.push_str("[vs]");
    parts.push(clause);
    Ok("vs")
}

/// Append the music input's gain clause, plus an `amix` for
/// [`AudioMode::Mix`], and return the label of the final audio stream.
fn music_track(spec: &CompositionSpec, audio_idx: usize, parts: &mut Vec<String>) -> &'static str {
    let Some(audio_spec) = &spec.audio else {
        return "ac";
    };
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
        AudioMode::Replace => "music",
        AudioMode::Mix => {
            parts.push("[ac][music]amix=inputs=2:duration=first[am]".to_string());
            "am"
        }
    }
}

/// Encoder flags for the output container.
fn encoder_args(container: Container, mux_subtitles: bool) -> Vec<&'static str> {
    match container {
        Container::Mp4 => {
            let mut flags = vec![
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
            ];
            if mux_subtitles {
                flags.extend(["-c:s", "mov_text"]);
            }
            flags
        }
        Container::Webm => vec!["-c:v", "libvpx-vp9", "-c:a", "libopus"],
    }
}

/// Build the ordered `ffmpeg` argument vector for one validated composition.
///
/// Every clip `-i` is preceded by `-f <demuxer>` from its sniffed container,
/// and the output is capped with `-fs max_output_bytes` (the caller still
/// checks the rendered size: `-fs` stops writing but exits successfully).
/// When the spec omits `resolution`/`fps`, every lane is normalised to clip
/// 0's probed geometry and rate.
///
/// # Errors
///
/// Returns a stable, non-secret reason string when `clips` does not match
/// `spec.clips` in length, a probed or effective clip duration is non-finite
/// or non-positive, a trim ends past its clip's probed duration, a
/// transition is longer than either clip it joins or would start at a
/// negative offset, or a required side input path (`audio`/`subtitles`) is
/// missing for a spec that declares one.
pub(crate) fn build_ffmpeg_args(
    spec: &CompositionSpec,
    clips: &[ClipInput],
    audio: Option<&Path>,
    subtitles: Option<&Path>,
    output: &Path,
    max_output_bytes: u64,
) -> Result<Vec<OsString>, &'static str> {
    if clips.len() != spec.clips.len() {
        return Err("clip inputs must match the spec");
    }
    let Some(first_clip) = clips.first() else {
        return Err("clip inputs must match the spec");
    };
    let durations: Vec<f64> = spec
        .clips
        .iter()
        .zip(clips)
        .map(|(clip_spec, input)| clip_duration(clip_spec, input))
        .collect();
    validate_against_probes(spec, clips, &durations)?;
    if spec.subtitles.is_some() && subtitles.is_none() {
        return Err("subtitles path is required by the spec");
    }
    if spec.audio.is_some() && audio.is_none() {
        return Err("audio path is required by the spec");
    }
    let format = VideoFormat::resolve(spec, first_clip);
    let mux_subtitles = matches!(
        spec.subtitles.as_ref().map(|s| s.mode),
        Some(SubtitleMode::Mux)
    );
    let subtitles_input_idx = clips.len() + usize::from(audio.is_some());
    let mut args = input_args(clips, audio, subtitles.filter(|_| mux_subtitles));

    let mut parts: Vec<String> = Vec::new();
    for (idx, (clip_spec, input)) in spec.clips.iter().zip(clips).enumerate() {
        parts.push(video_lane(idx, clip_spec, &format));
        parts.push(audio_lane(idx, clip_spec, input));
    }
    parts.extend(join_clauses(
        spec.transitions.as_deref(),
        &durations,
        clips.len(),
    ));
    let video_label = burn_in_subtitles(spec, subtitles, &mut parts)?;
    let audio_label = music_track(spec, clips.len(), &mut parts);

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
    args.extend(
        encoder_args(spec.output.container, mux_subtitles)
            .into_iter()
            .map(OsString::from),
    );

    if matches!(
        spec.audio.as_ref().map(|a| a.mode),
        Some(AudioMode::Replace)
    ) {
        args.push(OsString::from("-shortest"));
    }

    args.push(OsString::from("-fs"));
    args.push(OsString::from(max_output_bytes.to_string()));
    args.push(output.as_os_str().to_os_string());

    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::tests::{artifact_ref, minimal};
    use crate::spec::{AudioSpec, SubtitleStyle, SubtitlesSpec, TransitionSpec, TrimSpec};

    const MAX_OUT: u64 = 4_194_304;

    fn clip(path: &str, duration_s: f64) -> ClipInput {
        ClipInput {
            path: PathBuf::from(path),
            container: InputContainer::Mp4,
            duration_s,
            width: None,
            height: None,
            fps: None,
            has_audio: true,
        }
    }

    fn silent_clip(path: &str, duration_s: f64) -> ClipInput {
        ClipInput {
            has_audio: false,
            ..clip(path, duration_s)
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
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            None,
            None,
            Path::new("/out/movie.mp4"),
            MAX_OUT,
        )
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
    fn audio_less_clip_gets_a_synthesized_silent_lane() {
        let spec = minimal(2, None);
        let clips = [clip("/in/a.mp4", 4.0), silent_clip("/in/b.mp4", 6.0)];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            None,
            None,
            Path::new("/out/movie.mp4"),
            MAX_OUT,
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(
            text.contains("anullsrc=channel_layout=stereo:sample_rate=48000,atrim=duration=6[a1]"),
            "{text}"
        );
        assert!(
            !text.contains("[1:a]"),
            "the missing stream must never be referenced: {text}"
        );
        assert!(text.contains("[0:a]"), "clip 0 still uses its real audio");
        assert!(text.contains("concat=n=2:v=1:a=1"));
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
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            None,
            None,
            Path::new("/out/movie.mp4"),
            MAX_OUT,
        )
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
            MAX_OUT,
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
            MAX_OUT,
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
            MAX_OUT,
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
                Path::new("/o.mp4"),
                MAX_OUT,
            )
            .is_err()
        );
    }

    #[test]
    fn argv_pins_container_format_and_size_cap() {
        let spec = minimal(2, None);
        let clips = [
            clip("/in/a.mp4", 4.0),
            ClipInput {
                container: InputContainer::Matroska,
                ..clip("/in/b.webm", 6.0)
            },
        ];
        let args = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/out/movie.mp4"), 1234)
            .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.contains("-f mov -i /in/a.mp4"), "{text}");
        assert!(text.contains("-f matroska -i /in/b.webm"), "{text}");
        assert!(text.ends_with("-fs 1234 /out/movie.mp4"), "{text}");
    }

    #[test]
    fn omitted_resolution_and_fps_default_from_clip_zero() {
        let spec = minimal(2, None);
        let clips = [
            ClipInput {
                width: Some(1920),
                height: Some(1080),
                fps: Some(29.97),
                ..clip("/in/a.mp4", 4.0)
            },
            ClipInput {
                width: Some(640),
                height: Some(360),
                fps: Some(24.0),
                ..clip("/in/b.mp4", 6.0)
            },
        ];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            None,
            None,
            Path::new("/out/movie.mp4"),
            MAX_OUT,
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(
            text.contains("[0:v]scale=1920:1080,fps=29.97,setsar=1[v0]"),
            "{text}"
        );
        assert!(
            text.contains("[1:v]scale=1920:1080,fps=29.97,setsar=1[v1]"),
            "{text}"
        );

        let mut explicit = minimal(2, None);
        explicit.output.resolution = Some("1280x720".into());
        explicit.output.fps = Some(30);
        let args = build_ffmpeg_args(
            &explicit,
            &clips,
            None,
            None,
            Path::new("/out/movie.mp4"),
            MAX_OUT,
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(
            text.contains("[1:v]scale=1280:720,fps=30,setsar=1[v1]"),
            "{text}"
        );
        assert!(
            !text.contains("1920"),
            "the spec wins over the probe: {text}"
        );
    }

    #[test]
    fn trim_beyond_probed_duration_is_rejected() {
        let mut spec = minimal(1, None);
        spec.clips[0].trim = Some(TrimSpec {
            start_s: 1.0,
            end_s: 12.0,
        });
        let clips = [clip("/in/a.mp4", 10.0)];
        let error = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/o.mp4"), MAX_OUT)
            .expect_err("trim past the clip end");
        assert_eq!(error, "clip trim exceeds the probed clip duration");

        spec.clips[0].trim = Some(TrimSpec {
            start_s: 3.0,
            end_s: 3.0,
        });
        let error = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/o.mp4"), MAX_OUT)
            .expect_err("empty trim");
        assert_eq!(error, "clip trim must satisfy 0 <= start < end");

        spec.clips[0].trim = Some(TrimSpec {
            start_s: 1.0,
            end_s: 10.0,
        });
        assert!(
            build_ffmpeg_args(&spec, &clips, None, None, Path::new("/o.mp4"), MAX_OUT).is_ok(),
            "a trim ending exactly at the probed duration is fine"
        );
    }

    #[test]
    fn transition_longer_than_clip_is_rejected() {
        let spec = minimal(
            2,
            Some(vec![TransitionSpec {
                kind: TransitionKind::Crossfade,
                duration_s: Some(2.0),
            }]),
        );
        let clips = [clip("/in/a.mp4", 1.0), clip("/in/b.mp4", 6.0)];
        let error = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/o.mp4"), MAX_OUT)
            .expect_err("transition longer than clip 0");
        assert_eq!(error, "transition is longer than an adjacent clip");

        let clips = [clip("/in/a.mp4", 6.0), clip("/in/b.mp4", 1.5)];
        let error = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/o.mp4"), MAX_OUT)
            .expect_err("transition longer than clip 1");
        assert_eq!(error, "transition is longer than an adjacent clip");

        // A trimmed clip counts by its effective duration.
        let mut trimmed = spec.clone();
        trimmed.clips[0].trim = Some(TrimSpec {
            start_s: 0.0,
            end_s: 1.0,
        });
        let clips = [clip("/in/a.mp4", 6.0), clip("/in/b.mp4", 6.0)];
        assert!(
            build_ffmpeg_args(&trimmed, &clips, None, None, Path::new("/o.mp4"), MAX_OUT).is_err()
        );
        assert!(build_ffmpeg_args(&spec, &clips, None, None, Path::new("/o.mp4"), MAX_OUT).is_ok());
    }

    #[test]
    fn filter_path_escapes_backslash_quote_and_colon() {
        assert_eq!(
            escape_filter_path(Path::new("/a/b\\c:d'e.srt")),
            "/a/b\\\\c\\:d\\'e.srt"
        );
    }
}
