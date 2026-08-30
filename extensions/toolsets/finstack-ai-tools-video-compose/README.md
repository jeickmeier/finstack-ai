# finstack-ai-tools-video-compose

T1 declarative ffmpeg composition Toolset. Agents submit a bounded
composition spec (clips, transitions, audio, subtitles, output) as JSON;
the toolset alone translates that spec into a host-supplied `ffmpeg`
invocation. Agents can never pass raw flags or filesystem paths.

Construction requires an injected `ArtifactStore` — every clip, audio
track, and subtitle file referenced by a spec must already be a staged
artifact, and the store's own ceilings bound how large a render can get.

v1 transitions: `cut`, `crossfade`, `fade_to_black`. Subtitles are either
burned in from an SRT track or muxed as an mp4 `mov_text` stream.

A clip with no audio stream is handled automatically: the toolset gives it a
silent lane of the clip's effective duration instead of referencing an audio
stream that does not exist, so mixing silent and voiced clips renders
normally.

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_tools_video_compose::{VideoComposeConfig, VideoComposeToolset};

# fn example(artifact_store: Arc<dyn finstack_ai_runtime::artifact::ArtifactStore>) {
let tools = VideoComposeToolset::try_new(VideoComposeConfig {
    ffmpeg_path: PathBuf::from("/usr/bin/ffmpeg"),
    ffprobe_path: PathBuf::from("/usr/bin/ffprobe"),
    artifact_store,
    scratch_dir: PathBuf::from("/tmp/finstack-video-compose"),
    render_timeout: Duration::from_secs(300),
})
.expect("video compose");
# let _ = tools;
# }
```
