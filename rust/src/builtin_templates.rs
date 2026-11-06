//! Structure templates compiled into the launcher.
//!
//! Three presets ship with the binary, read from the plain-text manifests under
//! `rust/assets/structure/`:
//! - `launcher-structure.txt` is the dated preset the Python launcher shipped as
//!   `ui_slick/structure.txt`, copied verbatim (33 folders). The source copied
//!   that tree unchanged. The merged editor adds sequence/shot markers to its
//!   existing Projects/Nuke folder so its shot editor has a usable destination.
//! - `motiongraphics.txt` and `vfx.txt` are the FolderStructurer reference trees
//!   (`gitclones/FolderStructurer/Templates/MotionGraphics` and `.../VFX`),
//!   including their `sequences.txt`/`shots.txt` marker directories and the empty
//!   `01_plates/plate_manifest.txt` the source replicated into every project.
//!
//! `.gitkeep` files are not part of a template: they only kept an otherwise empty
//! folder in Git, so a folder that held one is still listed here as a directory.
//! The VFX preset's `08_output/[date]` folder is kept verbatim — the source never
//! substituted a date into that name.
//!
//! Manifest grammar: one entry per line, `#` starts a comment. A line with no tab
//! is a directory. A tab introduces comma-separated directives: `file` for a
//! regular file replicated with empty contents, `seq`/`shot` for the marker
//! files. Paths never contain a tab, so the split needs no quoting.

use std::path::Path;

use crate::structure::{StructureTemplate, TemplateMarkers};
use crate::transfers::{TransferCategory, TransferError};

/// The dated preset manifest (`ui_slick/structure.txt`), verbatim.
pub const LAUNCHER_MANIFEST: &str = include_str!("../assets/structure/launcher-structure.txt");
/// FolderStructurer's MotionGraphics template tree.
pub const MOTION_GRAPHICS_MANIFEST: &str = include_str!("../assets/structure/motiongraphics.txt");
/// FolderStructurer's VFX template tree.
pub const VFX_MANIFEST: &str = include_str!("../assets/structure/vfx.txt");

/// Display name of the dated launcher preset.
pub const LAUNCHER_NAME: &str = "Launcher";
/// Display name of FolderStructurer's MotionGraphics template.
pub const MOTION_GRAPHICS_NAME: &str = "MotionGraphics";
/// Display name of FolderStructurer's VFX template.
pub const VFX_NAME: &str = "VFX";

/// Every built-in template, Launcher first. Pure: the manifests are compiled in,
/// nothing is read from or written to disk.
///
/// A malformed manifest is reported instead of guessed, so the caller can show
/// the error rather than install a wrong tree.
pub fn builtin_templates() -> Result<Vec<StructureTemplate>, TransferError> {
    Ok(vec![
        manifest_template("launcher", LAUNCHER_NAME, LAUNCHER_MANIFEST)?,
        manifest_template(
            "motiongraphics",
            MOTION_GRAPHICS_NAME,
            MOTION_GRAPHICS_MANIFEST,
        )?,
        manifest_template("vfx", VFX_NAME, VFX_MANIFEST)?,
    ])
}

fn manifest_template(
    id: &str,
    name: &str,
    manifest: &str,
) -> Result<StructureTemplate, TransferError> {
    let mut template = StructureTemplate::new(id, name);
    for (index, raw) in manifest.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (path, directives) = match line.split_once('\t') {
            Some((path, directives)) => (path.trim_end(), directives.trim()),
            None => (line, ""),
        };
        let mut markers = TemplateMarkers::default();
        let mut is_file = false;
        for directive in directives
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            match directive {
                "file" => is_file = true,
                "seq" => markers.sequences = true,
                "shot" => markers.shots = true,
                other => {
                    return Err(manifest_error(
                        name,
                        index,
                        format!("unknown directive {other:?}"),
                    ));
                }
            }
        }
        if id == "launcher" && path == "Projects/Nuke" {
            markers = TemplateMarkers {
                sequences: true,
                shots: true,
            };
        }
        if is_file && markers.any() {
            return Err(manifest_error(
                name,
                index,
                "a file cannot carry sequence/shot markers",
            ));
        }
        let path = Path::new(path);
        let added = if is_file {
            template.add_file(path, Vec::new())
        } else {
            template.add_directory(path, markers)
        };
        added.map_err(|error| manifest_error(name, index, error.message()))?;
    }
    Ok(template)
}

fn manifest_error(name: &str, index: usize, message: impl Into<String>) -> TransferError {
    TransferError::new(
        TransferCategory::InvalidInput,
        None,
        format!(
            "the built-in {name} template manifest line {} is invalid: {}",
            index + 1,
            message.into()
        ),
    )
}
