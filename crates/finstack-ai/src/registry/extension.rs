use finstack_ai_runtime::{ComponentId, PortObject, Version};

use super::errors::RegistrationError;
use super::registrar::Registrar;

/// Explicit trust assumption for an extension source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionTrust {
    /// The component runs in-process with the full authority of its host.
    TrustedInProcess,
}

/// Identity and trust metadata for one extension registration source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionDescriptor {
    /// Namespaced extension identity.
    pub id: ComponentId,
    /// Extension semantic version.
    pub version: Version,
    /// Explicit trust assumption.
    pub trust: ExtensionTrust,
}

impl ExtensionDescriptor {
    /// Describe a trusted in-process extension source.
    #[must_use]
    pub const fn trusted_in_process(id: ComponentId, version: Version) -> Self {
        Self {
            id,
            version,
            trust: ExtensionTrust::TrustedInProcess,
        }
    }
}

/// Self-contained native or host extension that contributes typed registrations.
pub trait Extension: PortObject {
    /// Return the immutable source/trust descriptor.
    fn descriptor(&self) -> ExtensionDescriptor;

    /// Add typed registrations to the active source transaction.
    ///
    /// # Errors
    ///
    /// Returns a stable registration error. The registrar rolls back every
    /// registration made by this extension when the method fails.
    fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError>;
}
