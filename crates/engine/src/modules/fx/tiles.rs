//! Preview tiles for the asset panel, rendered by the compositor.

use std::path::PathBuf;

/// The tile of effect `kind`, rendered and cached on first use.
pub fn effect_tile(kind: &str, size: (u32, u32)) -> Result<PathBuf, String> {
    let _ = (kind, size);
    Err("effect tiles are not rendered yet".into())
}
