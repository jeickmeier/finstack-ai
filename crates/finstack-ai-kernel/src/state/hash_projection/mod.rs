//! Dedicated schema-1 state-hash projections with recursive explicit nulls.

mod projections;
mod schema;

pub(super) use schema::{
    KernelStateHashV1, KernelStateHashV2, KernelStateHashV3, KernelStateHashV4, KernelStateHashV5,
    KernelStateHashV6,
};
