//! Application paths, settings and recent projects.
//!
//! Owns everything that is true of the *installation* rather than of any one
//! project: where caches live, what the default canvas is, which projects were
//! opened recently.
//!
//! The rule that keeps this honest: anything under [`paths::cache_root`] must
//! be safe to delete at any moment. Derived data goes there; user data never
//! does. That is what makes a "clear cache" button a one-line operation
//! instead of a risk assessment.

pub mod commands;
pub mod paths;
pub mod settings;

pub use settings::{RecentProject, RecentProjects, Settings};
