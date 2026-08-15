//! World-surface inventory and experimental version policy.

use crate::generated::{
    AI_CONTEXT_PACKAGE, AI_HOST_PACKAGE, AI_TOOLSET_PACKAGE, AI_TYPES_PACKAGE, CONTEXT_FUNCS,
    CONTEXT_WORLD_EXPORTS, CRATE_VERSION, FORBIDDEN_WORLD_TOKENS, HOST_IMPORTS, PUBLISHED_PACKAGES,
    TOOLSET_FUNCS, TOOLSET_WORLD_EXPORTS,
};

/// Checked-in `types.wit`.
pub const TYPES_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-types/types.wit");
/// Checked-in `host.wit`.
pub const HOST_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-host/host.wit");
/// Checked-in `toolset.wit`.
pub const TOOLSET_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-toolset/toolset.wit");
/// Checked-in `context.wit`.
pub const CONTEXT_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-context/context.wit");

/// Confirm the experimental worlds export only the coarse toolset and context operations.
///
/// # Errors
///
/// Returns a static reason when the generated inventory drifts from A05/A06.
pub fn assert_experimental_surface() -> Result<(), &'static str> {
    if CRATE_VERSION == "1.0.0" {
        return Err("plugin crate version 1.0.0 is blocked until framework 1.0");
    }
    if PUBLISHED_PACKAGES
        != [
            AI_TYPES_PACKAGE,
            AI_HOST_PACKAGE,
            AI_TOOLSET_PACKAGE,
            AI_CONTEXT_PACKAGE,
        ]
    {
        return Err("published packages drifted");
    }
    if PUBLISHED_PACKAGES
        .iter()
        .any(|package| package.contains("@1.0.0"))
    {
        return Err("@1.0.0 package generation is blocked until framework 1.0");
    }
    if TOOLSET_WORLD_EXPORTS != ["toolset"] {
        return Err("toolset-plugin must export only toolset");
    }
    if TOOLSET_FUNCS != ["list-tools", "call"] {
        return Err("toolset must expose only list-tools and call");
    }
    if CONTEXT_WORLD_EXPORTS != ["context-provider"] {
        return Err("context-plugin must export only context-provider");
    }
    if CONTEXT_FUNCS != ["collect"] {
        return Err("context-provider must expose only collect");
    }
    if HOST_IMPORTS
        != [
            "finstack:ai-host/logging@0.0.4",
            "finstack:ai-host/blobs@0.0.4",
        ]
    {
        return Err("host imports must be logging and blobs only");
    }
    let surface = format!(
        "{} {} {} {} {}",
        TOOLSET_WORLD_EXPORTS.join(" "),
        TOOLSET_FUNCS.join(" "),
        CONTEXT_WORLD_EXPORTS.join(" "),
        CONTEXT_FUNCS.join(" "),
        HOST_IMPORTS.join(" ")
    );
    if FORBIDDEN_WORLD_TOKENS
        .iter()
        .any(|token| surface.split_whitespace().any(|part| part == *token))
    {
        return Err("forbidden world token leaked into the export surface");
    }
    if TYPES_WIT.contains("@1.0.0")
        || HOST_WIT.contains("@1.0.0")
        || TOOLSET_WIT.contains("@1.0.0")
        || CONTEXT_WIT.contains("@1.0.0")
    {
        return Err("checked-in WIT sources must stay on @0.0.4");
    }
    if TOOLSET_WIT.contains("export context-provider") || TOOLSET_WIT.contains("world agent") {
        return Err("toolset world must not export nested-agent or context surfaces");
    }
    if CONTEXT_WIT.contains("export toolset")
        || CONTEXT_WIT.contains("world agent")
        || CONTEXT_WIT.contains("initialize")
        || CONTEXT_WIT.contains("warmup")
        || CONTEXT_WIT.contains("shutdown")
    {
        return Err("context world must not export toolset, nested-agent, or lifecycle functions");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::assert_experimental_surface;
    use crate::generated::{
        CONTEXT_FUNCS, CONTEXT_WORLD_EXPORTS, CRATE_VERSION, TOOLSET_WORLD_EXPORTS,
    };

    #[test]
    fn experimental_surface_is_coarse_and_0x_only() {
        assert_experimental_surface().expect("surface");
        assert_eq!(CRATE_VERSION, "0.0.3");
        assert_eq!(TOOLSET_WORLD_EXPORTS, ["toolset"]);
        assert_eq!(CONTEXT_WORLD_EXPORTS, ["context-provider"]);
        assert_eq!(CONTEXT_FUNCS, ["collect"]);
    }
}
