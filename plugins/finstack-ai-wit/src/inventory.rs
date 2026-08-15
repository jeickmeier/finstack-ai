//! World-surface inventory and experimental version policy.

use crate::generated::{
    AI_HOST_PACKAGE, AI_TOOLSET_PACKAGE, AI_TYPES_PACKAGE, CRATE_VERSION, FORBIDDEN_WORLD_TOKENS,
    HOST_IMPORTS, PUBLISHED_PACKAGES, TOOLSET_FUNCS, TOOLSET_WORLD_EXPORTS,
};

/// Checked-in `types.wit`.
pub const TYPES_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-types/types.wit");
/// Checked-in `host.wit`.
pub const HOST_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-host/host.wit");
/// Checked-in `toolset.wit`.
pub const TOOLSET_WIT: &str = include_str!("../wit/v0.0.4/finstack-ai-toolset/toolset.wit");

/// Confirm the experimental world exports only coarse `toolset` operations.
///
/// # Errors
///
/// Returns a static reason when the generated inventory drifts from A06/A07.
pub fn assert_experimental_surface() -> Result<(), &'static str> {
    if CRATE_VERSION == "1.0.0" {
        return Err("plugin crate version 1.0.0 is blocked until framework 1.0");
    }
    if PUBLISHED_PACKAGES != [AI_TYPES_PACKAGE, AI_HOST_PACKAGE, AI_TOOLSET_PACKAGE] {
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
    if HOST_IMPORTS
        != [
            "finstack:ai-host/logging@0.0.4",
            "finstack:ai-host/blobs@0.0.4",
        ]
    {
        return Err("host imports must be logging and blobs only");
    }
    let surface = format!(
        "{} {} {}",
        TOOLSET_WORLD_EXPORTS.join(" "),
        TOOLSET_FUNCS.join(" "),
        HOST_IMPORTS.join(" ")
    );
    if FORBIDDEN_WORLD_TOKENS
        .iter()
        .any(|token| surface.split_whitespace().any(|part| part == *token))
    {
        return Err("forbidden world token leaked into the toolset export surface");
    }
    if TYPES_WIT.contains("@1.0.0") || HOST_WIT.contains("@1.0.0") || TOOLSET_WIT.contains("@1.0.0")
    {
        return Err("checked-in WIT sources must stay on @0.0.4");
    }
    if TOOLSET_WIT.contains("export context-provider") || TOOLSET_WIT.contains("world agent") {
        return Err("toolset world must not export nested-agent or context surfaces");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::assert_experimental_surface;
    use crate::generated::{CRATE_VERSION, TOOLSET_WORLD_EXPORTS};

    #[test]
    fn experimental_surface_is_coarse_and_0x_only() {
        assert_experimental_surface().expect("surface");
        assert_eq!(CRATE_VERSION, "0.0.3");
        assert_eq!(TOOLSET_WORLD_EXPORTS, ["toolset"]);
    }
}
