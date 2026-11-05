//! Pure Nuke script (`.nk`) content generation.
//!
//! Ported from the FolderStructurer reference sources:
//! `common/nuke_template.py` (`build_nuke_script_template`), the resolution
//! presets and script placement in `tabs/structure_tab.py` (`res_presets`,
//! `create_nuke_scripts_in_05_comp`, `replicate_template_structure`) and the
//! sequence/shot naming in `tabs/add_to_existing_project_tab.py`.
//!
//! Generation is pure: validated options plus a shot target produce UTF-8 script
//! text or bytes that S1 can place in `OperationPlan::CreateFile`. Nothing here
//! writes files or launches Nuke, and Nuke's acceptance of the emitted text is
//! not verified by this module.

use std::error::Error;
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Legacy default FPS (`StructureTab.combo_fps` selection).
pub const DEFAULT_FPS: f64 = 24.0;
/// Legacy default width (`StructureTab.spin_width`).
pub const DEFAULT_WIDTH: u32 = 1920;
/// Legacy default height (`StructureTab.spin_height`).
pub const DEFAULT_HEIGHT: u32 = 1080;
/// Legacy script version token; the template always emits `v001`.
pub const DEFAULT_SCRIPT_VERSION: u32 = 1;

/// Legacy `QSpinBox` dimension range in `StructureTab`.
const MAX_DIMENSION: u32 = 20_000;
const MAX_SCRIPT_VERSION: u32 = 999;

/// Two-line legacy header: Windows Nuke shebang path and script version.
const HEADER: &str = "#! C:/Program Files/Nuke15.1v5/nuke-15.1.5.dll -nx\nversion 15.1 v5\n";

const WINDOW_LAYOUT: &str = r##"define_window_layout_xml {<?xml version="1.0" encoding="UTF-8"?>
<layout version="1.0">
    <window x="-1" y="-8" w="2560" h="1369" maximized="1" screen="0">
        <splitter orientation="1">
            <split size="53"/>
            <dock id="" hideTitles="1" activePageId="Toolbar.1">
                <page id="Toolbar.1"/>
            </dock>
            <split size="1895"/>
            <splitter orientation="2">
                <split size="1328"/>
                <dock id="" activePageId="DAG.1" focus="true">
                    <page id="Curve Editor.1"/>
                    <page id="DopeSheet.1"/>
                    <page id="DAG.1"/>
                </dock>
            </splitter>
            <split size="604"/>
            <splitter orientation="2">
                <split size="1127"/>
                <dock id="" activePageId="Properties.1">
                    <page id="Properties.1"/>
                </dock>
                <split size="197"/>
                <dock id="" activePageId="Progress.1">
                    <page id="Progress.1"/>
                    <page id="Pixel Analyzer.1"/>
                </dock>
            </splitter>
        </splitter>
    </window>
    <window x="2560" y="-8" w="2560" h="1417" maximized="1" screen="1">
        <splitter orientation="2">
            <split size="1417"/>
            <dock id="" activePageId="Viewer.1">
                <page id="Viewer.1"/>
            </dock>
        </splitter>
    </window>
</layout>
}"##;

const PROXY_LINES: &str = r##" proxy_format "4000 4000 0 0 4000 4000 1 4K Proxy LL180 Sphere"
 proxySetting always"##;

// Legacy leaves a trailing space after the quoted value; kept for byte parity.
const SCALED_PROXY_LINES: &str = r##" proxy_type scale
 proxy_format "1024 778 0 0 1024 778 1 1K_Super_35(full-ap)" "##;

const ACES_COLOR_LINES: &str = r##" colorManagement OCIO
 OCIO_config aces_1.2
 defaultViewerLUT "OCIO LUTs"
 workingSpaceLUT scene_linear
 monitorLut "OCIO LUTs"
 monitorOutLUT "OCIO LUTs"
 int8Lut matte_paint
 int16Lut texture_paint
 logLut compositing_log
 floatLut scene_linear"##;

const NUKE_COLOR_LINES: &str = r##" colorManagement Nuke
 workingSpaceLUT linear
 monitorLut sRGB
 monitorOutLUT rec709
 int8Lut sRGB
 int16Lut sRGB
 logLut Cineon
 floatLut linear"##;

/// One legacy `StructureTab.res_presets` entry: UI presets map an exact pixel
/// size to the label emitted in the `.nk` `format` knob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolutionPreset {
    pub width: u32,
    pub height: u32,
    pub label: &'static str,
}

/// Legacy presets in dictionary insertion order; the first exact match wins.
pub const RESOLUTION_PRESETS: [ResolutionPreset; 9] = [
    ResolutionPreset {
        width: 1920,
        height: 1080,
        label: "HD_1080",
    },
    ResolutionPreset {
        width: 3840,
        height: 2160,
        label: "UHD_4K",
    },
    ResolutionPreset {
        width: 4096,
        height: 3112,
        label: "4K_Super_35",
    },
    ResolutionPreset {
        width: 4096,
        height: 2160,
        label: "4K_DCP",
    },
    ResolutionPreset {
        width: 4096,
        height: 4096,
        label: "4K_square",
    },
    ResolutionPreset {
        width: 4000,
        height: 4000,
        label: "4K_Sphere",
    },
    ResolutionPreset {
        width: 8000,
        height: 8000,
        label: "8K_Sphere",
    },
    ResolutionPreset {
        width: 10000,
        height: 10000,
        label: "10K_Sphere",
    },
    ResolutionPreset {
        width: 12000,
        height: 12000,
        label: "12K_Sphere",
    },
];

/// Legacy label rule: matching preset label, otherwise `Custom_<width>x<height>`.
pub fn resolution_label(width: u32, height: u32) -> String {
    RESOLUTION_PRESETS
        .iter()
        .find(|preset| preset.width == width && preset.height == height)
        .map(|preset| preset.label.to_owned())
        .unwrap_or_else(|| format!("Custom_{width}x{height}"))
}

/// Legacy structure variant that decides the script's parent folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructureLayout {
    /// Hardcoded Sphere layout; scripts live under `05_comp/`.
    Sphere,
    /// Hardcoded Kent layout; scripts live under `Projects/Nuke/`.
    Kent,
}

impl StructureLayout {
    /// Folder that collects per-shot comp folders. Legacy reads these labels
    /// from editable settings (`folder_05_comp`, `folder_projects`,
    /// `folder_projects_nuke`); these are their shipped defaults.
    pub fn comp_root(self) -> PathBuf {
        match self {
            Self::Sphere => PathBuf::from("05_comp"),
            Self::Kent => Path::new("Projects").join("Nuke"),
        }
    }
}

/// Options embedded in a generated script.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NukeScriptOptions {
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub proxy: bool,
    pub aces: bool,
}

impl Default for NukeScriptOptions {
    fn default() -> Self {
        Self {
            fps: DEFAULT_FPS,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            proxy: false,
            aces: false,
        }
    }
}

/// Where a script goes and how it is named.
#[derive(Clone, Copy, Debug)]
pub struct ScriptTarget<'a> {
    /// Parent of the `[sequence/]<shot>/project/` chain, e.g.
    /// [`StructureLayout::Sphere::comp_root`]. Passed in because legacy folder
    /// labels are editable settings.
    pub comp_root: &'a Path,
    /// `None` or `Some("")` means a shot without a sequence, matching the
    /// legacy `if seq` truthiness test.
    pub sequence: Option<&'a str>,
    pub shot: &'a str,
    pub version: u32,
}

impl<'a> ScriptTarget<'a> {
    pub fn new(comp_root: &'a Path, sequence: Option<&'a str>, shot: &'a str) -> Self {
        Self {
            comp_root,
            sequence,
            shot,
            version: DEFAULT_SCRIPT_VERSION,
        }
    }

    fn sequence_name(&self) -> Option<&'a str> {
        self.sequence
            .map(str::trim)
            .filter(|sequence| !sequence.is_empty())
    }

    /// Legacy file name: `<seq>_<shot>_comp_v001.nk`, or `<shot>_comp_v001.nk`
    /// when the shot has no sequence.
    pub fn file_name(&self) -> Result<String, NukeError> {
        validate_version(self.version)?;
        if let Some(sequence) = self.sequence_name() {
            validate_name(NameField::Sequence, sequence)?;
        }
        let shot = self.shot.trim();
        validate_name(NameField::Shot, shot)?;
        let name = match self.sequence_name() {
            Some(sequence) => {
                format!("{sequence}_{shot}_comp_v{:03}.nk", self.version)
            }
            None => format!("{shot}_comp_v{:03}.nk", self.version),
        };
        validate_name(NameField::FileName, &name)?;
        Ok(name)
    }

    /// Legacy path: `[seq/]<shot>/project/<file name>` below the comp root.
    pub fn relative_path(&self) -> Result<PathBuf, NukeError> {
        for component in self.comp_root.components() {
            let Component::Normal(name) = component else {
                return Err(NukeError::InvalidCompRoot {
                    path: self.comp_root.to_owned(),
                });
            };
            let Some(name) = name.to_str() else {
                return Err(NukeError::InvalidCompRoot {
                    path: self.comp_root.to_owned(),
                });
            };
            validate_name(NameField::CompFolder, name)?;
        }
        let file_name = self.file_name()?;
        let mut path = PathBuf::from(self.comp_root);
        if let Some(sequence) = self.sequence_name() {
            path.push(sequence);
        }
        path.push(self.shot.trim());
        path.push("project");
        path.push(file_name);
        Ok(path)
    }
}

/// Complete `.nk` document for `target`, ported from
/// `build_nuke_script_template`; whole-document runtime parity is unverified.
pub fn script_text(
    target: &ScriptTarget<'_>,
    options: &NukeScriptOptions,
) -> Result<String, NukeError> {
    validate_options(options)?;
    let file_name = target.file_name()?;
    Ok(format!(
        "{HEADER}{WINDOW_LAYOUT}\n{}\n{}\n",
        root_block(&file_name, options),
        viewer_block(options.fps)
    ))
}

/// UTF-8 bytes of [`script_text`], ready for `OperationPlan::CreateFile`.
pub fn script_bytes(
    target: &ScriptTarget<'_>,
    options: &NukeScriptOptions,
) -> Result<Vec<u8>, NukeError> {
    Ok(script_text(target, options)?.into_bytes())
}

fn root_block(file_name: &str, options: &NukeScriptOptions) -> String {
    let file_name = quote_scalar(file_name);
    let width = options.width;
    let height = options.height;
    let fps = options.fps;
    let label = resolution_label(width, height);
    let proxy = if options.proxy {
        PROXY_LINES
    } else {
        SCALED_PROXY_LINES
    };
    let color = if options.aces {
        ACES_COLOR_LINES
    } else {
        NUKE_COLOR_LINES
    };
    let format_line = format!(" format \"{width} {height} 0 0 {width} {height} 1 {label}\"");
    format!(
        "Root {{\n inputs 0\n name {file_name}\n fps {fps:.2}\n{format_line}\n{proxy}\n{color}\n}}\n"
    )
}

/// Serialize one literal Tcl scalar without substitutions or token splitting.
fn quote_scalar(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        if matches!(character, '\\' | '"' | '$' | '[' | ']') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

fn viewer_block(fps: f64) -> String {
    format!(
        "Viewer {{\n inputs 0\n frame 1\n frame_range 1-100\n fps {fps:.8}\n name Viewer1\n xpos -40\n ypos -9\n}}"
    )
}

fn validate_options(options: &NukeScriptOptions) -> Result<(), NukeError> {
    if !options.fps.is_finite() || options.fps <= 0.0 || format!("{:.2}", options.fps) == "0.00" {
        return Err(NukeError::InvalidFps { fps: options.fps });
    }
    if !(1..=MAX_DIMENSION).contains(&options.width)
        || !(1..=MAX_DIMENSION).contains(&options.height)
    {
        return Err(NukeError::InvalidResolution {
            width: options.width,
            height: options.height,
        });
    }
    Ok(())
}

fn validate_version(version: u32) -> Result<(), NukeError> {
    if !(1..=MAX_SCRIPT_VERSION).contains(&version) {
        return Err(NukeError::InvalidVersion { version });
    }
    Ok(())
}

/// Conservative portable path-component policy; script quoting is separate.
fn validate_name(field: NameField, value: &str) -> Result<(), NukeError> {
    let reason = if value.is_empty() {
        Some("is empty")
    } else if value.len() > 255 {
        Some("is longer than 255 bytes")
    } else if value != value.trim() {
        Some("has leading or trailing whitespace")
    } else if value == "." || value == ".." {
        Some("is a relative path component")
    } else if value.chars().any(|c| c == '/' || c == '\\' || c == '\0') {
        Some("contains a path separator or NUL")
    } else if value.chars().any(char::is_control) {
        Some("contains a control character")
    } else if value
        .chars()
        .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        Some("contains a character forbidden in a portable file name")
    } else if value.ends_with('.') {
        Some("ends with a dot")
    } else if reserved_component(value) {
        Some("is a reserved Windows device name")
    } else {
        None
    };
    match reason {
        None => Ok(()),
        Some(reason) => Err(NukeError::UnrepresentableName {
            field,
            value: value.to_owned(),
            reason,
        }),
    }
}

fn reserved_component(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or("").to_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
}

/// Input that carries a name into the script text and the script path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameField {
    Sequence,
    Shot,
    FileName,
    CompFolder,
}

impl fmt::Display for NameField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sequence => f.write_str("sequence"),
            Self::Shot => f.write_str("shot"),
            Self::FileName => f.write_str("assembled file"),
            Self::CompFolder => f.write_str("comp folder"),
        }
    }
}

/// A value that cannot be represented safely in a `.nk` script or path.
#[derive(Clone, Debug, PartialEq)]
pub enum NukeError {
    InvalidCompRoot {
        path: PathBuf,
    },
    UnrepresentableName {
        field: NameField,
        value: String,
        reason: &'static str,
    },
    InvalidFps {
        fps: f64,
    },
    InvalidResolution {
        width: u32,
        height: u32,
    },
    InvalidVersion {
        version: u32,
    },
}

impl fmt::Display for NukeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCompRoot { path } => {
                write!(f, "comp folder {path:?} must be a plain relative path")
            }
            Self::UnrepresentableName {
                field,
                value,
                reason,
            } => {
                write!(
                    f,
                    "{field} name {value:?} cannot be used in a Nuke script: it {reason}"
                )
            }
            Self::InvalidFps { fps } => {
                write!(
                    f,
                    "Nuke script fps {fps} must remain positive at two-decimal precision"
                )
            }
            Self::InvalidResolution { width, height } => write!(
                f,
                "Nuke script resolution {width}x{height} is outside 1..={MAX_DIMENSION}"
            ),
            Self::InvalidVersion { version } => write!(
                f,
                "Nuke script version {version} is outside 1..={MAX_SCRIPT_VERSION}"
            ),
        }
    }
}

impl Error for NukeError {}
