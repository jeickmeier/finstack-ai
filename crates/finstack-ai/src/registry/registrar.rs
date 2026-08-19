use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_runtime::{
    ComponentId, ComponentRef, ContextProvider, Digest, JournalStore, Middleware, Model, Observer,
    RawJson, Toolset,
};

use super::errors::{RegisteredComponentDescriptor, RegistrationError, RegistrationEvent};
use super::extension::{Extension, ExtensionDescriptor};
use super::resolve::Registry;
use super::types::{
    ComponentAlias, ComponentFactory, ComponentKind, DuplicatePolicy, MAX_REGISTERED_COMPONENTS,
    ReadyComponent, RegistrationMetadata,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FactoryConfiguration {
    Empty,
    Digest(Digest),
}

impl FactoryConfiguration {
    pub(super) fn from_raw(configuration: Option<&RawJson>) -> Self {
        configuration.map_or(Self::Empty, |value| Self::Digest(value.digest()))
    }
}

pub(super) enum RegistrationSlot<T: ?Sized> {
    Ready {
        component: ReadyComponent<T>,
        factory_configuration: Option<FactoryConfiguration>,
        model_warmed: bool,
    },
    Factory(Arc<dyn ComponentFactory<T>>),
}

impl<T: ?Sized> Clone for RegistrationSlot<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Ready {
                component,
                factory_configuration,
                model_warmed,
            } => Self::Ready {
                component: component.clone(),
                factory_configuration: *factory_configuration,
                model_warmed: *model_warmed,
            },
            Self::Factory(factory) => Self::Factory(Arc::clone(factory)),
        }
    }
}

pub(super) struct TypedRegistration<T: ?Sized> {
    pub(super) descriptor: RegisteredComponentDescriptor,
    pub(super) slot: RegistrationSlot<T>,
}

impl<T: ?Sized> Clone for TypedRegistration<T> {
    fn clone(&self) -> Self {
        Self {
            descriptor: self.descriptor.clone(),
            slot: self.slot.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) enum RegisteredEntry {
    Model(TypedRegistration<dyn Model>),
    Toolset(TypedRegistration<dyn Toolset>),
    ContextProvider(TypedRegistration<dyn ContextProvider>),
    Middleware(TypedRegistration<dyn Middleware>),
    Store(TypedRegistration<dyn JournalStore>),
    Observer(TypedRegistration<dyn Observer>),
}

impl RegisteredEntry {
    pub(super) fn descriptor(&self) -> &RegisteredComponentDescriptor {
        match self {
            Self::Model(value) => &value.descriptor,
            Self::Toolset(value) => &value.descriptor,
            Self::ContextProvider(value) => &value.descriptor,
            Self::Middleware(value) => &value.descriptor,
            Self::Store(value) => &value.descriptor,
            Self::Observer(value) => &value.descriptor,
        }
    }
}

/// Mutable transaction builder for one deterministic extension registry.
#[derive(Default)]
pub struct Registrar {
    sources: BTreeMap<ComponentId, ExtensionDescriptor>,
    entries: BTreeMap<ComponentId, RegisteredEntry>,
    aliases: BTreeMap<ComponentAlias, ComponentId>,
    pub(super) events: Vec<RegistrationEvent>,
    active_source: Option<ExtensionDescriptor>,
}

impl Registrar {
    /// Construct an empty registrar.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sources: BTreeMap::new(),
            entries: BTreeMap::new(),
            aliases: BTreeMap::new(),
            events: Vec::new(),
            active_source: None,
        }
    }

    /// Register one extension atomically under its declared source.
    ///
    /// # Errors
    ///
    /// Rejects duplicate sources, nested extension transactions, and any typed
    /// registration error. Failed extension transactions leave no partial state.
    pub fn register_extension(
        &mut self,
        extension: &dyn Extension,
    ) -> Result<(), RegistrationError> {
        if self.active_source.is_some() {
            return Err(RegistrationError::Reentrant);
        }
        let source = extension.descriptor();
        if self.sources.contains_key(&source.id) {
            return Err(RegistrationError::SourceDuplicate {
                extension_source: source.id,
            });
        }
        let entries = self.entries.clone();
        let aliases = self.aliases.clone();
        let event_len = self.events.len();
        self.active_source = Some(source.clone());
        let result = extension.register(self);
        self.active_source = None;
        match result {
            Ok(()) => {
                self.sources.insert(source.id.clone(), source);
                Ok(())
            }
            Err(error) => {
                self.entries = entries;
                self.aliases = aliases;
                self.events.truncate(event_len);
                Err(error)
            }
        }
    }

    /// Register one ready `Model` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn model(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn Model>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Model, |descriptor| {
            RegisteredEntry::Model(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one `Model` factory.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    #[cfg(test)]
    pub(crate) fn model_factory(
        &mut self,
        metadata: RegistrationMetadata,
        factory: Arc<dyn ComponentFactory<dyn Model>>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Model, |descriptor| {
            RegisteredEntry::Model(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Factory(factory),
            })
        })
    }

    /// Register one ready `Toolset` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn toolset(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn Toolset>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Toolset, |descriptor| {
            RegisteredEntry::Toolset(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one ready `ContextProvider` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn context_provider(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn ContextProvider>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::ContextProvider, |descriptor| {
            RegisteredEntry::ContextProvider(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one ready `Middleware` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn middleware(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn Middleware>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Middleware, |descriptor| {
            RegisteredEntry::Middleware(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one ready `JournalStore` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn store(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn JournalStore>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Store, |descriptor| {
            RegisteredEntry::Store(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one ready `Observer` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn observer(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn Observer>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Observer, |descriptor| {
            RegisteredEntry::Observer(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Freeze the registrar into a reusable registry.
    ///
    /// Selected factories are cached as ready handles by [`Registry::resolve`].
    #[must_use]
    pub fn into_registry(self) -> Registry {
        Registry {
            entries: self.entries,
            aliases: self.aliases,
            events: self.events.into(),
            #[cfg(test)]
            lookup_count: 0,
        }
    }

    fn insert(
        &mut self,
        metadata: RegistrationMetadata,
        kind: ComponentKind,
        build: impl FnOnce(RegisteredComponentDescriptor) -> RegisteredEntry,
    ) -> Result<(), RegistrationError> {
        let source = self
            .active_source
            .clone()
            .ok_or(RegistrationError::SourceRequired)?;
        let existing = self.entries.get(&metadata.id);
        let replaced_source = match (&metadata.duplicate_policy, existing) {
            (DuplicatePolicy::Reject, Some(existing)) => {
                return Err(RegistrationError::Duplicate {
                    component: metadata.id,
                    kind: existing.descriptor().kind,
                    existing_source: existing.descriptor().source.id.clone(),
                    attempted_source: source.id,
                });
            }
            (DuplicatePolicy::Replace { expected_source }, Some(existing))
                if &existing.descriptor().source.id == expected_source
                    && existing.descriptor().kind == kind =>
            {
                Some(existing.descriptor().source.id.clone())
            }
            (DuplicatePolicy::Replace { expected_source }, existing) => {
                return Err(RegistrationError::ReplacementRejected {
                    component: metadata.id,
                    expected_source: expected_source.clone(),
                    actual_source: existing.map(|entry| entry.descriptor().source.id.clone()),
                    attempted_source: source.id,
                    attempted_kind: kind,
                    actual_kind: existing.map(|entry| entry.descriptor().kind),
                });
            }
            (DuplicatePolicy::Reject, None) => None,
        };
        if existing.is_none() && self.entries.len() >= MAX_REGISTERED_COMPONENTS {
            return Err(RegistrationError::ComponentLimit {
                limit: MAX_REGISTERED_COMPONENTS,
            });
        }
        for alias in &metadata.aliases {
            if let Some(existing_component) = self.aliases.get(alias)
                && existing_component != &metadata.id
            {
                let existing_source = self.entries.get(existing_component).map_or_else(
                    || source.id.clone(),
                    |entry| entry.descriptor().source.id.clone(),
                );
                return Err(RegistrationError::AliasConflict {
                    alias: alias.clone(),
                    component: metadata.id,
                    existing_component: existing_component.clone(),
                    existing_source,
                    attempted_source: source.id,
                });
            }
        }
        if let Some(existing) = self.entries.get(&metadata.id) {
            for alias in existing.descriptor().aliases.iter() {
                self.aliases.remove(alias);
            }
        }
        let component = ComponentRef::new(metadata.id.clone(), Some(metadata.version));
        let aliases: Arc<[ComponentAlias]> = metadata.aliases.into();
        let descriptor = RegisteredComponentDescriptor {
            component: component.clone(),
            kind,
            source: source.clone(),
            aliases: Arc::clone(&aliases),
            configuration_schema: metadata.configuration_schema,
        };
        for alias in aliases.iter() {
            self.aliases.insert(alias.clone(), metadata.id.clone());
        }
        self.entries.insert(metadata.id, build(descriptor));
        self.events.push(RegistrationEvent {
            component,
            kind,
            source: source.id,
            replaced_source,
        });
        Ok(())
    }
}
