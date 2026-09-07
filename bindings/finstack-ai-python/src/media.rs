//! Typed media configuration; materialize shared dependencies once per agent assembly.
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai::media::{openai, openrouter, pipeline, video};
use finstack_ai::runtime::artifact::ArtifactStore;
use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai_kernel::ComponentRef;
use pyo3::prelude::*;

use crate::agent::{PREVIEW_VERSION, component};
use crate::errors::{agent_error, configuration_error};

type Registration = (ComponentRef, Arc<dyn Toolset>);

/// Explicit `OpenAI` media credentials, independent of the model provider.
#[pyclass(
    name = "OpenAiMediaToolset",
    module = "finstack_ai._finstack_ai",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyOpenAiMediaToolset {
    config: Arc<openai::OpenAiMediaConfig>,
}

#[pymethods]
impl PyOpenAiMediaToolset {
    /// Configure outbound media tools. Uses the receiving agent's artifact store.
    #[new]
    #[pyo3(signature = (api_key, *, max_result_bytes = 262_144))]
    fn new(api_key: String, max_result_bytes: usize) -> Self {
        Self {
            config: Arc::new(openai::OpenAiMediaConfig {
                api_key,
                endpoint: String::new(),
                max_result_bytes,
            }),
        }
    }

    /// Exact registration identity, unchanged from the former factory option.
    #[getter]
    #[expect(
        clippy::unused_self,
        reason = "Python instance property exposes a fixed native identity"
    )]
    fn component(&self) -> &'static str {
        "python.toolset.openai_media"
    }
}

/// Explicit `OpenRouter` media credentials and attribution.
#[pyclass(
    name = "OpenRouterMediaToolset",
    module = "finstack_ai._finstack_ai",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyOpenRouterMediaToolset {
    config: Arc<openrouter::OpenRouterMediaConfig>,
}

#[pymethods]
impl PyOpenRouterMediaToolset {
    /// Configure media tools. Validation occurs when an agent materializes this configuration.
    #[new]
    #[pyo3(signature = (api_key, *, referer = None, title = None, max_result_bytes = 262_144))]
    fn new(
        api_key: String,
        referer: Option<String>,
        title: Option<String>,
        max_result_bytes: usize,
    ) -> Self {
        Self {
            config: Arc::new(openrouter::OpenRouterMediaConfig {
                api_key,
                endpoint: String::new(),
                referer,
                title,
                max_result_bytes,
            }),
        }
    }

    /// Exact registration identity, unchanged from the former factory option.
    #[getter]
    #[expect(
        clippy::unused_self,
        reason = "Python instance property exposes a fixed native identity"
    )]
    fn component(&self) -> &'static str {
        "python.toolset.openrouter_media"
    }
}

struct VideoConfig {
    ffmpeg_path: PathBuf,
    ffprobe_path: PathBuf,
    scratch_dir: PathBuf,
    render_timeout: Duration,
}

/// Explicit local ffmpeg paths and render ceiling; never searches PATH.
#[pyclass(
    name = "VideoComposeToolset",
    module = "finstack_ai._finstack_ai",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyVideoComposeToolset {
    config: Arc<VideoConfig>,
}

#[pymethods]
impl PyVideoComposeToolset {
    /// Configure local composition over the receiving agent's artifact store.
    #[new]
    #[pyo3(signature = (ffmpeg_path, ffprobe_path, scratch_dir, *, render_timeout_s))]
    fn new(
        ffmpeg_path: PathBuf,
        ffprobe_path: PathBuf,
        scratch_dir: PathBuf,
        render_timeout_s: u64,
    ) -> Self {
        Self {
            config: Arc::new(VideoConfig {
                ffmpeg_path,
                ffprobe_path,
                scratch_dir,
                render_timeout: Duration::from_secs(render_timeout_s),
            }),
        }
    }

    /// Exact registration identity, unchanged from the former factory option.
    #[getter]
    #[expect(
        clippy::unused_self,
        reason = "Python instance property exposes a fixed native identity"
    )]
    fn component(&self) -> &'static str {
        "python.toolset.video_compose"
    }
}

struct PipelineConfig {
    media: Arc<openrouter::OpenRouterMediaConfig>,
    compose: Arc<VideoConfig>,
    limits: pipeline::PlanLimits,
    sqlite_state_path: Option<PathBuf>,
}

/// `MoviePlan` orchestration over explicitly supplied media and compose dependencies.
#[pyclass(
    name = "MediaPipelineToolset",
    module = "finstack_ai._finstack_ai",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyMediaPipelineToolset {
    config: Arc<PipelineConfig>,
}

#[pymethods]
impl PyMediaPipelineToolset {
    /// Configure bounded scenes/jobs. Dependencies share the agent's artifact store.
    ///
    /// Pass the same objects in toolsets to expose their tools individually too.
    /// `sqlite_state_path` persists render state; omitting it uses process-local state.
    #[new]
    #[pyo3(signature = (media, compose, *, max_scenes, max_total_video_s, max_concurrent_jobs, sqlite_state_path = None))]
    fn new(
        media: &PyOpenRouterMediaToolset,
        compose: &PyVideoComposeToolset,
        max_scenes: usize,
        max_total_video_s: u64,
        max_concurrent_jobs: usize,
        sqlite_state_path: Option<PathBuf>,
    ) -> Self {
        Self {
            config: Arc::new(PipelineConfig {
                media: Arc::clone(&media.config),
                compose: Arc::clone(&compose.config),
                limits: pipeline::PlanLimits {
                    max_scenes,
                    max_total_video_s,
                    max_concurrent_jobs,
                },
                sqlite_state_path,
            }),
        }
    }

    /// Exact registration identity, unchanged from the former factory option.
    #[getter]
    #[expect(
        clippy::unused_self,
        reason = "Python instance property exposes a fixed native identity"
    )]
    fn component(&self) -> &'static str {
        "python.toolset.media_pipeline"
    }
}

/// One assembly's cache retains config ownership so identity cannot be reused.
/// It never crosses artifact-store or agent-construction boundaries.
pub(crate) struct MediaRegistrations {
    artifacts: Arc<dyn ArtifactStore>,
    openai: Vec<(Arc<openai::OpenAiMediaConfig>, Arc<dyn Toolset>)>,
    openrouter: Vec<(Arc<openrouter::OpenRouterMediaConfig>, Arc<dyn Toolset>)>,
    compose: Vec<(Arc<VideoConfig>, Arc<dyn Toolset>)>,
    pipeline: Vec<(Arc<PipelineConfig>, Arc<dyn Toolset>)>,
}

impl MediaRegistrations {
    pub(crate) fn new(artifacts: Arc<dyn ArtifactStore>) -> Self {
        Self {
            artifacts,
            openai: vec![],
            openrouter: vec![],
            compose: vec![],
            pipeline: vec![],
        }
    }

    pub(crate) fn openai(&mut self, tools: &PyOpenAiMediaToolset) -> PyResult<Registration> {
        let config = &tools.config;
        let handle = if let Some((_, handle)) =
            self.openai.iter().find(|(key, _)| Arc::ptr_eq(key, config))
        {
            Arc::clone(handle)
        } else {
            let handle: Arc<dyn Toolset> = Arc::new(
                openai::OpenAiMediaToolset::try_new((**config).clone())
                    .map_err(media_error)?
                    .with_artifact_store(Arc::clone(&self.artifacts)),
            );
            self.openai.push((Arc::clone(config), Arc::clone(&handle)));
            handle
        };
        registration("python.toolset.openai_media", handle)
    }

    pub(crate) fn openrouter(
        &mut self,
        tools: &PyOpenRouterMediaToolset,
    ) -> PyResult<Registration> {
        let handle = self.openrouter_handle(&tools.config)?;
        registration("python.toolset.openrouter_media", handle)
    }

    fn openrouter_handle(
        &mut self,
        config: &Arc<openrouter::OpenRouterMediaConfig>,
    ) -> PyResult<Arc<dyn Toolset>> {
        if let Some((_, handle)) = self
            .openrouter
            .iter()
            .find(|(key, _)| Arc::ptr_eq(key, config))
        {
            return Ok(Arc::clone(handle));
        }
        let handle: Arc<dyn Toolset> = Arc::new(
            openrouter::OpenRouterMediaToolset::try_new((**config).clone())
                .map_err(media_error)?
                .with_artifact_store(Arc::clone(&self.artifacts)),
        );
        self.openrouter
            .push((Arc::clone(config), Arc::clone(&handle)));
        Ok(handle)
    }

    pub(crate) fn compose(&mut self, tools: &PyVideoComposeToolset) -> PyResult<Registration> {
        let handle = self.compose_handle(&tools.config)?;
        registration("python.toolset.video_compose", handle)
    }

    fn compose_handle(&mut self, config: &Arc<VideoConfig>) -> PyResult<Arc<dyn Toolset>> {
        if let Some((_, handle)) = self
            .compose
            .iter()
            .find(|(key, _)| Arc::ptr_eq(key, config))
        {
            return Ok(Arc::clone(handle));
        }
        let handle: Arc<dyn Toolset> = Arc::new(
            video::VideoComposeToolset::try_new(video::VideoComposeConfig {
                ffmpeg_path: config.ffmpeg_path.clone(),
                ffprobe_path: config.ffprobe_path.clone(),
                scratch_dir: config.scratch_dir.clone(),
                render_timeout: config.render_timeout,
                artifact_store: Arc::clone(&self.artifacts),
            })
            .map_err(media_error)?,
        );
        self.compose.push((Arc::clone(config), Arc::clone(&handle)));
        Ok(handle)
    }

    pub(crate) fn pipeline(&mut self, tools: &PyMediaPipelineToolset) -> PyResult<Registration> {
        let config = &tools.config;
        if let Some((_, handle)) = self
            .pipeline
            .iter()
            .find(|(key, _)| Arc::ptr_eq(key, config))
        {
            return registration("python.toolset.media_pipeline", Arc::clone(handle));
        }
        let media_tools = self.openrouter_handle(&config.media)?;
        let compose_tools = self.compose_handle(&config.compose)?;
        let state: Arc<dyn pipeline::RenderStateStore> = match &config.sqlite_state_path {
            Some(path) => {
                Arc::new(pipeline::SqliteRenderStateStore::open(path).map_err(media_error)?)
            }
            None => Arc::new(pipeline::MemoryRenderStateStore::new()),
        };
        let driver = pipeline::MediaPipelineDriver::try_new(pipeline::MediaPipelineConfig {
            media_tools,
            compose_tools,
            state,
            artifact_store: Some(Arc::clone(&self.artifacts)),
            limits: config.limits,
        })
        .map_err(media_error)?;
        let handle: Arc<dyn Toolset> = Arc::new(
            pipeline::MediaPipelineToolset::try_new(Arc::new(driver)).map_err(media_error)?,
        );
        self.pipeline
            .push((Arc::clone(config), Arc::clone(&handle)));
        registration("python.toolset.media_pipeline", handle)
    }
}

fn registration(id: &str, handle: Arc<dyn Toolset>) -> PyResult<Registration> {
    Ok((
        component(id, PREVIEW_VERSION).map_err(|error| agent_error(&error, None))?,
        handle,
    ))
}

fn media_error(error: impl std::fmt::Display) -> PyErr {
    agent_error(&configuration_error(error.to_string()), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai::runtime::artifact::InProcessArtifactStore;

    #[test]
    fn pipeline_and_individual_registrations_share_handles_only_within_assembly() {
        let scratch = tempfile::tempdir().expect("scratch");
        let media = PyOpenRouterMediaToolset::new("media-secret".into(), None, None, 262_144);
        let compose = PyVideoComposeToolset::new(
            "/usr/bin/ffmpeg".into(),
            "/usr/bin/ffprobe".into(),
            scratch.path().into(),
            300,
        );
        let pipeline = PyMediaPipelineToolset::new(&media, &compose, 4, 120, 2, None);
        let mut cache = MediaRegistrations::new(Arc::new(InProcessArtifactStore::default()));
        let pipeline_handle = cache.pipeline(&pipeline).expect("pipeline").1;
        let media_handle = cache.openrouter(&media).expect("media").1;
        let compose_handle = cache.compose(&compose).expect("compose").1;
        assert!(Arc::ptr_eq(&media_handle, &cache.openrouter[0].1));
        assert!(Arc::ptr_eq(&compose_handle, &cache.compose[0].1));
        assert!(Arc::ptr_eq(
            &pipeline_handle,
            &cache.pipeline(&pipeline).expect("again").1
        ));
        assert_eq!(cache.openrouter.len(), 1);
        assert_eq!(cache.compose.len(), 1);
        let mut other = MediaRegistrations::new(Arc::new(InProcessArtifactStore::default()));
        assert!(!Arc::ptr_eq(
            &media_handle,
            &other.openrouter(&media).expect("other store").1
        ));
    }
}
