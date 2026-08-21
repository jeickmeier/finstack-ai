//! Versioned `kernel-state` hash projections (schemas 1–7) with recursive explicit nulls.

mod projections;
mod schema;

pub(super) use schema::{
    KernelStateHashV1, KernelStateHashV2, KernelStateHashV3, KernelStateHashV4, KernelStateHashV5,
    KernelStateHashV6, KernelStateHashV7,
};
