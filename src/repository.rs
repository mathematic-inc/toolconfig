//! Pinned Git sources, fetched with the user's Git authentication.

mod cache;
mod git;
mod spec;

pub(crate) use cache::{cache_root, fetch};
pub(crate) use spec::Repository;
