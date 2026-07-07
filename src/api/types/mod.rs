//! API request/response DTOs split by domain. Cross-cutting types live in `common`.

mod accounts;
mod auth;
mod backup;
mod common;
mod models;
mod providers;
mod router;
mod settings;
mod shares;
mod usage;

pub(in crate::api) use accounts::*;
pub(in crate::api) use auth::*;
pub(in crate::api) use backup::*;
pub(in crate::api) use common::*;
pub(in crate::api) use models::*;
pub(in crate::api) use providers::*;
pub(in crate::api) use router::*;
pub(in crate::api) use settings::*;
pub(in crate::api) use shares::*;
pub(in crate::api) use usage::*;
