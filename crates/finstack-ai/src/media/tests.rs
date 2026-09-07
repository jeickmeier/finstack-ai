use super::{openai, openrouter, pipeline, video};
use crate::{
    Agent, LinkedAgentPorts, LinkedCommon, LinkedProviderSpec, OllamaAgentSpec, OpenAiAgentSpec,
};
use finstack_ai_kernel::{ComponentId, ComponentRef, Version};
use finstack_ai_runtime::{
    artifact::{ArtifactStore, InProcessArtifactStore},
    ports::tool::Toolset,
};
use std::sync::Arc;

#[tokio::test]
async fn provider_changes_preserve_typed_media_catalogs_and_handles() {
    let artifacts: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());
    let scratch = tempfile::tempdir().expect("scratch");
    let registrations = media_registrations(&artifacts, scratch.path());
    let common = || LinkedCommon {
        ports: LinkedAgentPorts {
            toolsets: registrations.clone(),
            artifact_store: Some(Arc::clone(&artifacts)),
            ..LinkedAgentPorts::default()
        },
        ..LinkedCommon::default()
    };
    for spec in [
        LinkedProviderSpec::Ollama(OllamaAgentSpec {
            model: "fixture".into(),
            base_url: "http://127.0.0.1:1".into(),
            common: common(),
        }),
        LinkedProviderSpec::OpenAi(OpenAiAgentSpec {
            model: "fixture".into(),
            api_key: "model-secret-only".into(),
            reasoning_effort: None,
            reasoning_summary: None,
            common: common(),
        }),
    ] {
        let built = Agent::linked(spec).await.expect("compose");
        for resolved in built.agent.resolved().run_plan().toolsets() {
            let (_, expected) = registrations
                .iter()
                .find(|(id, _)| id == &resolved.descriptor().component)
                .expect("same identity");
            assert!(Arc::ptr_eq(expected, resolved.handle()));
            assert_eq!(expected.tools(), resolved.handle().tools());
        }
        assert_eq!(built.agent.resolved().run_plan().toolsets().len(), 4);
    }
}

fn media_registrations(
    artifacts: &Arc<dyn ArtifactStore>,
    scratch: &std::path::Path,
) -> Vec<(ComponentRef, Arc<dyn Toolset>)> {
    let media: Arc<dyn Toolset> = Arc::new(
        openrouter::OpenRouterMediaToolset::try_new(openrouter::OpenRouterMediaConfig {
            api_key: "explicit-media-secret".into(),
            endpoint: String::new(),
            referer: None,
            title: None,
            max_result_bytes: 262_144,
        })
        .expect("media")
        .with_artifact_store(Arc::clone(artifacts)),
    );
    let openai: Arc<dyn Toolset> = Arc::new(
        openai::OpenAiMediaToolset::try_new(openai::OpenAiMediaConfig {
            api_key: "separate-openai-media-secret".into(),
            endpoint: String::new(),
            max_result_bytes: 262_144,
        })
        .expect("openai media")
        .with_artifact_store(Arc::clone(artifacts)),
    );
    let compose: Arc<dyn Toolset> = Arc::new(
        video::VideoComposeToolset::try_new(video::VideoComposeConfig {
            ffmpeg_path: "/usr/bin/ffmpeg".into(),
            ffprobe_path: "/usr/bin/ffprobe".into(),
            scratch_dir: scratch.into(),
            render_timeout: std::time::Duration::from_mins(5),
            artifact_store: Arc::clone(artifacts),
        })
        .expect("compose"),
    );
    let pipeline: Arc<dyn Toolset> = Arc::new(
        pipeline::MediaPipelineToolset::try_new(Arc::new(
            pipeline::MediaPipelineDriver::try_new(pipeline::MediaPipelineConfig {
                media_tools: Arc::clone(&media),
                compose_tools: Arc::clone(&compose),
                artifact_store: Some(Arc::clone(artifacts)),
                state: Arc::new(pipeline::MemoryRenderStateStore::new()),
                limits: pipeline::PlanLimits {
                    max_scenes: 4,
                    max_total_video_s: 120,
                    max_concurrent_jobs: 2,
                },
            })
            .expect("driver"),
        ))
        .expect("pipeline"),
    );
    let registrations: Vec<_> = [
        ("openai_media", openai),
        ("openrouter_media", media),
        ("video_compose", compose),
        ("media_pipeline", pipeline),
    ]
    .into_iter()
    .map(|(suffix, tools)| {
        (
            ComponentRef::new(
                ComponentId::parse(format!("python.toolset.{suffix}")).expect("id"),
                Some(Version {
                    major: 0,
                    minor: 0,
                    patch: 1,
                }),
            ),
            tools,
        )
    })
    .collect();
    registrations
}
