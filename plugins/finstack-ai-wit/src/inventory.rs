//! World-surface inventory and dual-major version policy.

use crate::generated::{
    AI_CONTEXT_PACKAGE, AI_CONTEXT_PACKAGE_V1, AI_HOST_PACKAGE, AI_HOST_PACKAGE_V1,
    AI_TOOLSET_PACKAGE, AI_TOOLSET_PACKAGE_V1, AI_TYPES_PACKAGE, AI_TYPES_PACKAGE_V1,
    CONTEXT_FUNCS, CONTEXT_WORLD_EXPORTS, FORBIDDEN_WORLD_TOKENS, HOST_IMPORTS, HOST_IMPORTS_V1,
    PUBLISHED_PACKAGES, PUBLISHED_PACKAGES_V004, PUBLISHED_PACKAGES_V100, TOOLSET_FUNCS,
    TOOLSET_WORLD_EXPORTS,
};

/// Checked-in `types.wit` (`@0.0.4`).
pub const TYPES_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-types/types.wit");
/// Checked-in `host.wit` (`@0.0.4`).
pub const HOST_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-host/host.wit");
/// Checked-in `toolset.wit` (`@0.0.4`).
pub const TOOLSET_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-toolset/toolset.wit");
/// Checked-in `context.wit` (`@0.0.4`).
pub const CONTEXT_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-context/context.wit");
/// Checked-in `types.wit` (`@1.0.0`).
pub const TYPES_WIT_V1: &str = include_str!("../wit/v1.0.0/finstack-ai-types/types.wit");
/// Checked-in `host.wit` (`@1.0.0`).
pub const HOST_WIT_V1: &str = include_str!("../wit/v1.0.0/finstack-ai-host/host.wit");
/// Checked-in `toolset.wit` (`@1.0.0`).
pub const TOOLSET_WIT_V1: &str = include_str!("../wit/v1.0.0/finstack-ai-toolset/toolset.wit");
/// Checked-in `context.wit` (`@1.0.0`).
pub const CONTEXT_WIT_V1: &str = include_str!("../wit/v1.0.0/finstack-ai-context/context.wit");

/// Confirm both majors export only the coarse toolset and context operations.
///
/// # Errors
///
/// Returns a static reason when the generated inventory drifts from the freeze.
pub fn assert_experimental_surface() -> Result<(), &'static str> {
    assert_published_packages()?;
    assert_coarse_exports()?;
    assert_source_versions()?;
    Ok(())
}

fn assert_published_packages() -> Result<(), &'static str> {
    if PUBLISHED_PACKAGES_V004
        != [
            AI_TYPES_PACKAGE,
            AI_HOST_PACKAGE,
            AI_TOOLSET_PACKAGE,
            AI_CONTEXT_PACKAGE,
        ]
    {
        return Err("published @0.0.4 packages drifted");
    }
    if PUBLISHED_PACKAGES_V100
        != [
            AI_TYPES_PACKAGE_V1,
            AI_HOST_PACKAGE_V1,
            AI_TOOLSET_PACKAGE_V1,
            AI_CONTEXT_PACKAGE_V1,
        ]
    {
        return Err("published @1.0.0 packages drifted");
    }
    if PUBLISHED_PACKAGES
        != [
            AI_TYPES_PACKAGE,
            AI_HOST_PACKAGE,
            AI_TOOLSET_PACKAGE,
            AI_CONTEXT_PACKAGE,
            AI_TYPES_PACKAGE_V1,
            AI_HOST_PACKAGE_V1,
            AI_TOOLSET_PACKAGE_V1,
            AI_CONTEXT_PACKAGE_V1,
        ]
    {
        return Err("published dual-major packages drifted");
    }
    Ok(())
}

fn assert_coarse_exports() -> Result<(), &'static str> {
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
    if HOST_IMPORTS_V1
        != [
            "finstack:ai-host/logging@1.0.0",
            "finstack:ai-host/blobs@1.0.0",
        ]
    {
        return Err("@1.0.0 host imports must be logging and blobs only");
    }
    let surface = format!(
        "{} {} {} {} {} {}",
        TOOLSET_WORLD_EXPORTS.join(" "),
        TOOLSET_FUNCS.join(" "),
        CONTEXT_WORLD_EXPORTS.join(" "),
        CONTEXT_FUNCS.join(" "),
        HOST_IMPORTS.join(" "),
        HOST_IMPORTS_V1.join(" ")
    );
    if FORBIDDEN_WORLD_TOKENS
        .iter()
        .any(|token| surface.split_whitespace().any(|part| part == *token))
    {
        return Err("forbidden world token leaked into the export surface");
    }
    Ok(())
}

fn assert_source_versions() -> Result<(), &'static str> {
    if TYPES_WIT.contains("@1.0.0")
        || HOST_WIT.contains("@1.0.0")
        || TOOLSET_WIT.contains("@1.0.0")
        || CONTEXT_WIT.contains("@1.0.0")
    {
        return Err("checked-in @0.0.4 WIT sources must stay on @0.0.4");
    }
    if !TYPES_WIT_V1.contains("@1.0.0")
        || !HOST_WIT_V1.contains("@1.0.0")
        || !TOOLSET_WIT_V1.contains("@1.0.0")
        || !CONTEXT_WIT_V1.contains("@1.0.0")
        || TYPES_WIT_V1.contains("@0.0.4")
        || HOST_WIT_V1.contains("@0.0.4")
        || TOOLSET_WIT_V1.contains("@0.0.4")
        || CONTEXT_WIT_V1.contains("@0.0.4")
    {
        return Err("checked-in @1.0.0 WIT sources must stay on @1.0.0");
    }
    for source in [TOOLSET_WIT, TOOLSET_WIT_V1] {
        if source.contains("export context-provider") || source.contains("world agent") {
            return Err("toolset world must not export nested-agent or context surfaces");
        }
    }
    for source in [CONTEXT_WIT, CONTEXT_WIT_V1] {
        if source.contains("export toolset")
            || source.contains("world agent")
            || source.contains("initialize")
            || source.contains("warmup")
            || source.contains("shutdown")
        {
            return Err(
                "context world must not export toolset, nested-agent, or lifecycle functions",
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::assert_experimental_surface;
    use crate::generated::{
        CONTEXT_FUNCS, CONTEXT_WORLD_EXPORTS, CRATE_VERSION, PUBLISHED_PACKAGES,
        TOOLSET_WORLD_EXPORTS,
    };

    #[test]
    fn experimental_surface_is_coarse_and_dual_major() {
        assert_experimental_surface().expect("surface");
        assert_eq!(CRATE_VERSION, env!("CARGO_PKG_VERSION"));
        assert_eq!(TOOLSET_WORLD_EXPORTS, ["toolset"]);
        assert_eq!(CONTEXT_WORLD_EXPORTS, ["context-provider"]);
        assert_eq!(CONTEXT_FUNCS, ["collect"]);
        assert!(PUBLISHED_PACKAGES.contains(&"finstack:ai-types@1.0.0"));
    }
}
