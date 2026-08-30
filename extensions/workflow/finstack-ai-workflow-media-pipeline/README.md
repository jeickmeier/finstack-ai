# finstack-ai-workflow-media-pipeline

MoviePlan pipeline driver, in the `finstack-ai-workflow-local` mold:
tick-based, resumable, with adapter-owned sqlite state rather than kernel
journal records.

Tools `render_movie`, `advance_render`, and `get_render_status` expose the
pipeline to agents. Hosts bound spend via `PlanLimits`; all generated media
moves through the host's artifact store as `ArtifactRef`s, never as raw
bytes on the wire.

This crate is T1 native and is not isolated.
