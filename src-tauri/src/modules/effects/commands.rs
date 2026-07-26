//! The `#[tauri::command]` surface for effects.
//!
//! The webview never touches a package: it names one and gets back a
//! description. Every path here is one the user supplied, so it is treated as
//! untrusted — see `package::safe_join`.

use serde::Serialize;

use super::package::{EffectPackage, Parameter};

#[derive(Debug, Serialize)]
pub struct EffectDescription {
    pub name: String,
    pub origin: String,
    pub version: Option<String>,
    pub links: Vec<LinkDescription>,
    pub parameters: Vec<Parameter>,
    /// What this runtime cannot do with this package, in prose. Empty means it
    /// should render.
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LinkDescription {
    pub kind: String,
    pub path: String,
    pub zorder: f64,
    pub scene_root: Option<String>,
}

/// Open a package and describe it, without building any GPU state.
///
/// Deliberately separate from loading the runtime: the inspector needs the
/// parameter list the moment the user picks a file, and compiling shaders for
/// a package they are about to reject is wasted.
#[tauri::command]
pub fn effects_describe(path: String) -> Result<EffectDescription, String> {
    let package = EffectPackage::open(&path).map_err(|e| e.to_string())?;

    let links: Vec<LinkDescription> = package
        .links()
        .iter()
        .enumerate()
        .map(|(index, link)| LinkDescription {
            kind: link.kind.clone(),
            path: link.path.clone(),
            zorder: link.zorder,
            scene_root: package.scene_root(index),
        })
        .collect();

    let mut limitations = Vec::new();
    if links.is_empty() {
        limitations.push(
            "This package declares no render links. Model-only and Lynx-Studio packages \
             are like this by design and have nothing to draw."
                .to_string(),
        );
    }
    for link in &links {
        let Some(root) = &link.scene_root else {
            limitations.push(format!("{}: no scene or prefab root", link.path));
            continue;
        };
        match package.read(root) {
            Ok(bytes) if bytes.starts_with(super::assets::BINARY_MAGIC) => limitations.push(
                format!(
                    "{root} is in the binary %SerializedFormat%@ encoding. This runtime reads \
                     the YAML twin of that format only; see modules/effects/assets.rs."
                ),
            ),
            Ok(_) => {}
            Err(error) => limitations.push(format!("{root}: {error}")),
        }
    }

    Ok(EffectDescription {
        name: package.name().to_string(),
        origin: package.origin().to_string(),
        version: package.config().version.clone(),
        links,
        parameters: package.parameters().to_vec(),
        limitations,
    })
}
