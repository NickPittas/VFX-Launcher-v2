//! I5: incoming filename patterns and destination profiles.
//!
//! Two reference concepts live here, and nothing else does:
//!
//! - a [`PatternSet`], the ordered per-category filename patterns of
//!   `CleanIncomings/config/patterns.json` (shot, task, version, resolution,
//!   asset, stage);
//! - an [`IncomingProfile`], the ordered destination-path/keyword rules of
//!   `CleanIncomings/config/profiles.json`, and the routing that turns
//!   extracted tags into a destination folder.
//!
//! Ported behavior, read from the reference sources (read-only `gitclones/`
//! clones) rather than from their READMEs:
//!
//! - `mapping_utils/{shot,task,version,resolution,asset,stage}_extractor.py`:
//!   each pattern is tried, in order, as a case-insensitive regular expression
//!   searched inside the filename. An expression Python accepts but this
//!   dialect cannot compile falls back to the legacy literal containment test
//!   and is reported as a dialect issue. What each category returns is kept
//!   exactly: shot/version/resolution return the matched substring, asset and
//!   stage return the pattern text, and task returns its configured label.
//! - `mapping.py` / `IMPORTANT.md`: only the filename participates. Parent
//!   folders, project names and the source root never do, so two files with the
//!   same name in different folders extract identical tags.
//! - `mapping_utils/generate_simple_target_path.py`: exact task match, then
//!   exact asset match, then conflicting partial (substring) matches become
//!   ambiguous, and otherwise the configured footage rule is used — the rule
//!   whose keyword set equals the documented footage keywords first, then any
//!   rule containing one of them, and finally the visible `unmapped_footage`
//!   folder. Dynamic segments keep the legacy order and case: shot as written,
//!   stage/task/asset/version lowercased, resolution uppercased.
//! - `mapping_utils/generate_mappings.py` + `create_*_mapping.py`: a sequence is
//!   matched once, from its base name, instead of once per member.
//!
//! Deliberate deviations, each recorded where it is made:
//!
//! - compiled patterns cache their `Regex`, so matching a directory of
//!   thousands of members compiles each expression once;
//! - tag values are trimmed, so a pattern that captures surrounding spaces
//!   cannot create padded path segments (`os.path.normpath` hid this);
//! - rule paths are split into segments on both separators and validated, so
//!   an imported `..` or drive-absolute rule is refused instead of escaping the
//!   destination root at join time;
//! - the expressions are compiled by a backtracking engine, not by the linear
//!   automaton alone, so a pattern the legacy tool accepted keeps its meaning:
//!   look-around, backreferences (`\1`, `\k<name>`, `(?P=name)`), named groups
//!   (`(?P<name>…)`), atomic groups and the `\A`/`\Z` anchors all run. Search
//!   semantics are unchanged: `re.search` first at the leftmost position, and
//!   group 0 is the value a shot/version/resolution rule reports;
//! - that engine is bounded. Every expression is built with an explicit
//!   compile-size limit and an explicit backtracking-step limit, so a
//!   pathological rule cannot stall a worker and the UI thread never runs one.
//!   A rule that hits the step limit matches nothing *for that filename* and is
//!   reported by [`CompiledPatterns::bounded_issues`]; its text is never
//!   reinterpreted as plain text and never rewritten;
//! - a pattern the engine refuses to compile is reported and then matches
//!   nothing. It is not converted into a plain-text comparison: switching an
//!   expression to containment changes what it means, so that step stays the
//!   user's explicit choice through `RuleMode::Literal`;
//! - the pattern file gains one additive `ruleModes` object so a rule the user
//!   marked literal stays literal across a save. The reference loader reads only
//!   the six pattern keys, so the extra object is ignored there;
//! - five of the reference camera task expressions put an inline `(?i)` after a
//!   literal dot (`\\.(?i)(mxf|mov)$`). CPython rejects a global inline flag in
//!   that position and the legacy extractor then silently degraded the whole
//!   expression to a literal comparison, so those five configured rules could
//!   never match a real filename. This dialect accepts the construct, so the
//!   imported ARRI/RED/Sony/Canon/DJI rules route as their authors wrote them.
//!   Nothing is hardcoded: the same file still drives them, and the rule list
//!   stays editable.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fancy_regex::{Regex, RegexBuilder};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The pattern configuration seeded from `CleanIncomings/config/patterns.json`,
/// with support for newer ARRI clip names. It is the only seeded configuration;
/// nothing in the matching code knows a camera, shot or task name.
const REFERENCE_PATTERNS_JSON: &str = r##"{
  "shotPatterns": [
    "SC\\d{3}",
    "sh\\d{3}",
    "OLNT\\d{4}",
    "KITC\\d{4}",
    "WTFB\\d{4}",
    "IGB_\\d{4}",
    "IGB\\d{4}",
    "LIFF_\\d{4}",
    "LIFF\\d{4}",
    "SOME\\d{4}",
    "SOME0010"
  ],
  "taskPatterns": {
    "ARRI_Camera_File_Task": ["^([A-Z]_?\\d{3,4}C\\d{3})_\\d{6}_(?:\\d{6}_[A-Za-z0-9]+|[A-Z]\\d+[A-Z0-9]*)\\.(?i)(mxf|ari|mov|mp4)$"],
    "RED_Camera_File_Task": ["^([A-Z]\\d{3}_C\\d{3}_\\d{2}\\d{2}[A-Z0-9]{2})(?:_\\d{3})?\\.(?i)(R3D|mov|mp4|mxf)$"],
    "Sony_Camera_File_Task": ["^((?:[A-Z]?C\\d{3,}(?:M\\d{2})?)|(?:[A-Z]\\d{3}C\\d{3}_(?:[A-Z0-9]+_)?\\d{6}[A-Z0-9]*)|(?:R\\d{3}C\\d{3}_(?:[A-Z0-9]+_)?\\d{6}[A-Z0-9]*)|(?:[A-Z]{2,4}\\d{4,}))\\.(?i)(mxf|mp4|mov)$"],
    "Canon_Camera_File_Task": ["^((?:[A-Z0-9]{2,6}\\d{2,6}(?:_\\d{2})?)|(?:R\\d{3}C\\d{3})|(?:(?:IMG_|MVI_|_MG_)\\d{4,})|(?:[A-Z]\\d{3}R[35678]\\d{2}_.*))\\.(?i)(mxf|mp4|crm|mov)$"],
    "DJI_Camera_File_Task": ["^((?:DJI_\\d{4,})|(?:\\d{8}_\\d{6}|\\d{4}_\\d{2}_\\d{2}_\\d{2}_\\d{2}_\\d{2})(_\\d{3,})?[^.]*)\\.(?i)(mp4|mov)$"],
    "3d": [
      "3d",
      "models",
      "renders",
      "textures"
    ],
    "raw": [
      "vrayraw.",
      "vrayraw_"
    ],
    "albedo": [
      "albedo",
      "albedo_map",
      "alb"
    ],
    "beauty": [
      "arnoldbeauty",
      "beauty",
      "beauty_pass",
      "beauty_rgb",
      "color",
      "cyclescombined",
      "karmabeauty",
      "mantrabeauty",
      "octanebeauty",
      "redshiftbeauty",
      "rmanbeauty",
      "vraybeauty"
    ],
    "bump": [
      "bump",
      "bump_map"
    ],
    "cryptomatte": [
      "arnoldcryptoasset",
      "arnoldcryptomaterial",
      "arnoldcryptoobject",
      "crypt",
      "cryptomatte",
      "cryptomatte_map",
      "cyclescryptoasset",
      "cyclescryptomaterial",
      "cyclescryptoobject",
      "karmacryptoasset",
      "karmacryptomaterial",
      "karmacryptoobject",
      "mantracryptoasset",
      "mantracryptomaterial",
      "mantracryptoobject",
      "octanecryptoasset",
      "octanecryptomaterial",
      "octanecryptoobject",
      "redshiftcryptoasset",
      "redshiftcryptomaterial",
      "redshiftcryptoobject",
      "rmancryptoasset",
      "rmancryptomaterial",
      "rmancryptoobject",
      "vraycryptoasset",
      "vraycryptomaterial",
      "vraycryptoobject"
    ],
    "depth": [
      "arnoldz",
      "cyclesz",
      "depth",
      "karmaz",
      "mantraz",
      "octanezdepth",
      "redshiftzdepth",
      "rmanz",
      "vrayzdepth",
      "zdepth"
    ],
    "diffuse": [
      "arnolddiffuse",
      "arnolddiffusedirect",
      "arnolddiffuseindirect",
      "cyclesdiffusecolor",
      "cyclesdiffusedirect",
      "cyclesdiffuseindirect",
      "diff",
      "diffuse",
      "diffuse_albedo",
      "karmadiffuse",
      "mantradiffuse",
      "octanediffuse",
      "redshiftdiffusefilter",
      "redshiftdiffuselighting",
      "rmandiffuse",
      "vraydiffuse",
      "vrayrawdiffuse"
    ],
    "displacement": [
      "disp",
      "displacement",
      "displacement_map"
    ],
    "emission": [
      "arnoldemission",
      "cyclesemit",
      "karmaemission",
      "mantraemission",
      "octaneemission",
      "redshiftemission",
      "rmanemission",
      "vrayemission"
    ],
    "emissive": [
      "emiss",
      "emissive",
      "emissive_map"
    ],
    "footage": [
      "footage",
      "source",
      "video"
    ],
    "lighting": [
      "arnoldindirect",
      "gi",
      "global illumination",
      "karmaindirect",
      "redshiftgi",
      "rmanindirect",
      "vraygi",
      "vraylighting",
      "vrayrawlighting"
    ],
    "metallic": [
      "metal",
      "metallic",
      "metallic_map"
    ],
    "normal": [
      "arnoldn",
      "cyclesnormal",
      "karman",
      "mantran",
      "norm",
      "normal",
      "normal_map",
      "octanenormals",
      "redshiftnormals",
      "rmann",
      "vraynormals"
    ],
    "objectid": [
      "arnoldid",
      "karmaid",
      "mantraid",
      "redshiftid",
      "rmanid",
      "vrayid"
    ],
    "occlusion": [
      "cyclesao",
      "occl",
      "occlusion",
      "occlusion_map",
      "octaneambientocclusion",
      "vrayshadow"
    ],
    "position": [
      "pos",
      "position",
      "position_map"
    ],
    "puzzle": [
      "puz_a",
      "puz_b",
      "puz_c",
      "puz_d",
      "puz_e",
      "puz_f",
      "puz_g",
      "puz_h",
      "puz_i",
      "puz_j",
      "puz_k",
      "puz_l",
      "puz_m",
      "puz_n",
      "puz_o",
      "puz_p",
      "puz_q",
      "puz_r",
      "puz_s",
      "puz_t",
      "puz_u",
      "puz_v",
      "puz_w",
      "puz_x",
      "puz_y",
      "puz_z",
      "puzzle",
      "puzzle_map",
      "pzl"
    ],
    "refraction": [
      "arnoldtransmission",
      "cyclestransmissioncolor",
      "cyclestransmissiondirect",
      "cyclestransmissionindirect",
      "karmarefraction",
      "mantrarefract",
      "mantratransmission",
      "octanerefraction",
      "octanetransmission",
      "redshiftrefraction",
      "redshifttransmission",
      "rmantransmission",
      "vrayrawrefraction",
      "refr",
      "vrayrefraction"
    ],
    "rgb": [
      "rgb",
      "rgba"
    ],
    "roughness": [
      "rough",
      "roughness",
      "roughness_albedo"
    ],
    "specular": [
      "arnoldspecular",
      "arnoldspeculardirect",
      "arnoldspecularindirect",
      "cyclesglossycolor",
      "cyclesglossydirect",
      "cyclesglossyindirect",
      "karmaspecular",
      "mantrareflect",
      "mantraspecular",
      "octanereflection",
      "octanespecular",
      "redshiftreflection",
      "redshiftspecularfilter",
      "redshiftspecularlighting",
      "refl",
      "reflection",
      "reflection_map",
      "rmanspecular",
      "spec",
      "specular",
      "specular_albedo",
      "vrayrawreflection",
      "vrayrawspecular",
      "vrayreflection",
      "vrayspecular"
    ],
    "sss": [
      "arnoldsss",
      "karmasss",
      "mantrasss",
      "octanesss",
      "redshiftsss",
      "rmansss",
      "sss",
      "vraysss"
    ],
    "velocity": [
      "arnoldmotionvector",
      "cyclesvector",
      "karmavelocity",
      "mantravelocity",
      "octanemotionvector",
      "redshiftmotionvectors",
      "vel",
      "velocity",
      "velocity_map",
      "vrayvelocity"
    ],
    "volume": [
      "arnoldvolume",
      "cyclesvolume",
      "volume",
      "vol",
      "octanevolume",
      "redshiftvolume"
    ]
  },
  "resolutionPatterns": [
    "8k",
    "12k",
    "2k",
    "16k",
    "6k",
    "24k",
    "4k",
    "uhd",
    "hd",
    "1920x1080",
    "3840x2160"
  ],
  "versionPatterns": [
    "v\\d{3}",
    "ver\\d{3}",
    "version\\d{3}",
    "_v\\d{2,3}",
    "\\.v\\d{2,3}"
  ],
  "assetPatterns": [
    "main_arch",
    "second_arch",
    "master",
    "hero",
    "scene",
    "background",
    "vehicle",
    "character",
    "set",
    "prop",
    "env",
    "fx",
    "matte",
    "crowd",
    "imags"
  ],
  "stagePatterns": [
    "PREVIZ",
    "ANIM",
    "LAYOUT",
    "BLOCK",
    "LIGHT",
    "COMP",
    "comp",
    "plt",
    "RENDER",
    "CLEANUP",
    "DELIVERY"
  ]
}"##;

/// The reference destination profiles, copied verbatim from
/// `CleanIncomings/config/profiles.json`.
const REFERENCE_PROFILES_JSON: &str = r##"{
  "Simple Project": [
    {
      "3D/Renders": [
        "main_arch",
        "second_arch",
        "hero",
        "background",
        "vehicle",
        "character",
        "prop",
        "env",
        "fx",
        "matte",
        "crowd",
        "imags",
        "PREVIZ",
        "ANIM",
        "LAYOUT",
        "BLOCK",
        "LIGHT",
        "RENDER",
        "3d",
        "alb",
        "albedo",
        "albedo_map",
        "arnoldbeauty",
        "arnoldcryptoasset",
        "arnoldcryptomaterial",
        "arnoldcryptoobject",
        "arnolddiffuse",
        "arnolddiffusedirect",
        "arnolddiffuseindirect",
        "arnoldemission",
        "arnoldid",
        "arnoldindirect",
        "arnoldmotionvector",
        "arnoldn",
        "arnoldspecular",
        "arnoldspeculardirect",
        "arnoldspecularindirect",
        "arnoldsss",
        "arnoldtransmission",
        "arnoldvolume",
        "arnoldz",
        "beauty"
      ]
    },
    {
      "Projects/Nuke": [
        "COMP",
        "comp",
        "CLEANUP"
      ]
    },
    {
      "Video/Footage": [
        "MAIN",
        "footage",
        "source",
        "video",
        "plate",
        "plt",
        "plates",
        "DELIVERY"
      ]
    },
    {
      "Video/Footage/ARRI": [
        "ARRI_Camera_File_Task"
      ]
    },
    {
      "Video/Footage/RED": [
        "RED_Camera_File_Task"
      ]
    },
    {
      "Video/Footage/Sony": [
        "Sony_Camera_File_Task"
      ]
    },
    {
      "Video/Footage/Canon": [
        "Canon_Camera_File_Task"
      ]
    },
    {
      "Video/Footage/DJI": [
        "DJI_Camera_File_Task"
      ]
    }
  ],
  "Sphere Project": [
    {
      "04_vfx": [
        "3d",
        "albedo",
        "beauty",
        "bump",
        "depth",
        "diffuse",
        "displacement",
        "emission",
        "lighting",
        "metallic",
        "normal",
        "objectid",
        "occlusion",
        "position",
        "refraction",
        "rgb",
        "roughness",
        "specular",
        "sss",
        "volume",
        "main_arch",
        "second_arch",
        "hero",
        "background",
        "vehicle",
        "character",
        "prop",
        "env",
        "fx",
        "matte",
        "crowd",
        "imags",
        "PREVIZ",
        "FINAL",
        "ANIM",
        "LAYOUT",
        "BLOCK",
        "LIGHT",
        "RENDER",
        "cryptomatte",
        "puzzle",
        "emissive",
        "velocity"
      ]
    },
    {
      "05_comp": [
        "COMP",
        "oomp",
        "CLEANUP"
      ]
    },
    {
      "01_plates": [
        "footage",
        "source",
        "video",
        "plate",
        "plates",
        "plt",
        "DELIVERY"
      ]
    }
  ]
}"##;

/// The documented footage keywords (`generate_simple_target_path.py`
/// `DEFAULT_FOOTAGE_KEYWORDS`), lowercase as the reference declares them.
pub const DEFAULT_FOOTAGE_KEYWORDS: [&str; 5] = ["footage", "video", "source", "plate", "plates"];

/// Folder the legacy generator uses when a profile has no footage rule at all.
/// It is a real segment in the reviewed destination, never a silent skip.
pub const UNMAPPED_FALLBACK: &str = "unmapped_footage";

/// One tag category, in the order the reference extractors run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TagKind {
    Shot,
    Stage,
    Task,
    Asset,
    Resolution,
    Version,
}

impl TagKind {
    /// Every category, in the reference extraction order.
    pub const ALL: [Self; 6] = [
        Self::Shot,
        Self::Stage,
        Self::Task,
        Self::Asset,
        Self::Resolution,
        Self::Version,
    ];

    /// Display and editor label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Shot => "Shot",
            Self::Stage => "Stage",
            Self::Task => "Task",
            Self::Asset => "Asset",
            Self::Resolution => "Resolution",
            Self::Version => "Version",
        }
    }

    /// The tag's own short name, as the proposals and the editor order them.
    pub fn tag_key(self) -> &'static str {
        match self {
            Self::Shot => "shot",
            Self::Stage => "stage",
            Self::Task => "task",
            Self::Asset => "asset",
            Self::Resolution => "resolution",
            Self::Version => "version",
        }
    }

    /// The `patterns.json` key that holds this category.
    pub fn file_key(self) -> &'static str {
        match self {
            Self::Shot => "shotPatterns",
            Self::Stage => "stagePatterns",
            Self::Task => "taskPatterns",
            Self::Asset => "assetPatterns",
            Self::Resolution => "resolutionPatterns",
            Self::Version => "versionPatterns",
        }
    }
}

/// How one pattern is interpreted. `Regex` is the imported default; `Literal`
/// is either an explicit user choice or the recorded outcome of a compile
/// failure, exactly like the reference `re.error` branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleMode {
    Regex,
    Literal,
}

impl RuleMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Regex => "Regex",
            Self::Literal => "Literal",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "regex" => Some(Self::Regex),
            "literal" => Some(Self::Literal),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Regex => "regex",
            Self::Literal => "literal",
        }
    }
}

/// One imported pattern. `task` is the configured label for a task rule: the
/// `taskPatterns` object key, which is what the reference task extractor
/// returns as the tag value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternRule {
    pub task: Option<String>,
    pub text: String,
    pub mode: RuleMode,
}

impl PatternRule {
    pub fn new(text: impl Into<String>, mode: RuleMode) -> Self {
        Self {
            task: None,
            text: text.into(),
            mode,
        }
    }

    pub fn task_rule(task: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            task: Some(task.into()),
            text: text.into(),
            mode: RuleMode::Regex,
        }
    }
}

/// One actionable problem with a rule: syntax this dialect refuses, or an
/// expression that hit the bounded execution limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialectIssue {
    pub kind: TagKind,
    pub task: Option<String>,
    pub text: String,
    pub message: String,
}

impl DialectIssue {
    /// One line naming the rule and what to do about it.
    pub fn describe(&self) -> String {
        let category = match &self.task {
            Some(task) => format!("{} rule for '{task}'", self.kind.label()),
            None => format!("{} pattern", self.kind.label()),
        };
        format!("{category} \"{}\": {}", self.text, self.message)
    }
}

/// The ordered pattern configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternSet {
    shot: Vec<PatternRule>,
    stage: Vec<PatternRule>,
    task: Vec<PatternRule>,
    asset: Vec<PatternRule>,
    resolution: Vec<PatternRule>,
    version: Vec<PatternRule>,
}

impl Default for PatternSet {
    fn default() -> Self {
        Self::reference_defaults()
    }
}

impl PatternSet {
    /// The reference `config/patterns.json` contents, used as the starting
    /// configuration so a first run behaves like the tool it ports. It is data,
    /// not matching logic: routing has no built-in pattern of its own, and the
    /// default is read through the same parser as an explicit import.
    pub fn reference_defaults() -> Self {
        Self::from_json(REFERENCE_PATTERNS_JSON).unwrap_or_else(|_| Self {
            shot: Vec::new(),
            stage: Vec::new(),
            task: Vec::new(),
            asset: Vec::new(),
            resolution: Vec::new(),
            version: Vec::new(),
        })
    }

    pub fn rules(&self, kind: TagKind) -> &[PatternRule] {
        match kind {
            TagKind::Shot => &self.shot,
            TagKind::Stage => &self.stage,
            TagKind::Task => &self.task,
            TagKind::Asset => &self.asset,
            TagKind::Resolution => &self.resolution,
            TagKind::Version => &self.version,
        }
    }

    pub fn rules_mut(&mut self, kind: TagKind) -> &mut Vec<PatternRule> {
        match kind {
            TagKind::Shot => &mut self.shot,
            TagKind::Stage => &mut self.stage,
            TagKind::Task => &mut self.task,
            TagKind::Asset => &mut self.asset,
            TagKind::Resolution => &mut self.resolution,
            TagKind::Version => &mut self.version,
        }
    }

    /// Configured task labels, in the order the reference dictionary declared
    /// them. Order is precedence when several camera tasks could match.
    pub fn task_labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = Vec::new();
        for rule in &self.task {
            let Some(task) = &rule.task else {
                continue;
            };
            if !labels.iter().any(|known| known == task) {
                labels.push(task.clone());
            }
        }
        labels
    }

    /// Number of rules in one category.
    pub fn count(&self, kind: TagKind) -> usize {
        self.rules(kind).len()
    }

    pub fn total_rules(&self) -> usize {
        TagKind::ALL.iter().map(|kind| self.count(*kind)).sum()
    }

    /// Removes one rule, keeping the order of the rest.
    pub fn remove(&mut self, kind: TagKind, index: usize) -> bool {
        let rules = self.rules_mut(kind);
        if index >= rules.len() {
            return false;
        }
        rules.remove(index);
        true
    }

    /// Moves one rule by `delta` positions within its category; the reference
    /// evaluates rules in file order, so this changes precedence.
    pub fn move_rule(&mut self, kind: TagKind, index: usize, delta: isize) -> bool {
        let rules = self.rules_mut(kind);
        let Some(target) = offset(index, delta, rules.len()) else {
            return false;
        };
        rules.swap(index, target);
        true
    }

    /// Every pattern this dialect cannot compile, with the reason.
    pub fn dialect_issues(&self) -> Vec<DialectIssue> {
        let mut issues = Vec::new();
        for kind in TagKind::ALL {
            for rule in self.rules(kind) {
                let Some(message) = Self::dialect_message(rule) else {
                    continue;
                };
                issues.push(DialectIssue {
                    kind,
                    task: rule.task.clone(),
                    text: rule.text.clone(),
                    message,
                });
            }
        }
        issues
    }

    fn dialect_message(rule: &PatternRule) -> Option<String> {
        if rule.mode == RuleMode::Literal {
            return None;
        }
        compile_rule(rule).err().map(|error| {
            format!(
                "{error}; this rule is skipped until it is rewritten for this dialect or marked Literal — it is not matched as plain text"
            )
        })
    }

    /// Compiles every pattern once, so a directory of thousands of members still
    /// pays for parsing each expression only once. Explicit literal rules are
    /// containment matchers; an expression the engine refuses is reported by
    /// [`Self::dialect_issues`] and matches nothing.
    pub fn compile(&self) -> CompiledPatterns {
        let mut task: Vec<(String, Vec<CompiledRule>)> = Vec::new();
        for rule in &self.task {
            let Some(label) = rule.task.clone() else {
                continue;
            };
            let compiled = CompiledRule::new(rule);
            match task.iter_mut().find(|(known, _)| known == &label) {
                Some((_, rules)) => rules.push(compiled),
                None => task.push((label, vec![compiled])),
            }
        }
        CompiledPatterns {
            shot: compile_rules(&self.shot),
            stage: compile_rules(&self.stage),
            task,
            asset: compile_rules(&self.asset),
            resolution: compile_rules(&self.resolution),
            version: compile_rules(&self.version),
        }
    }

    /// The `patterns.json` text: the six reference keys in reference order,
    /// plus the additive `ruleModes` record.
    pub fn to_json(&self) -> String {
        let groups = self.task_groups();
        let modes = RuleModes {
            shot: modes(&self.shot),
            stage: modes(&self.stage),
            // The reference file can only express task rules grouped by label, so
            // this flat list follows the serialized `taskPatterns` order. A list
            // in edit order would re-attach modes to the wrong rules on load.
            task: groups
                .iter()
                .flat_map(|(_, _, modes)| modes.clone())
                .collect(),
            asset: modes(&self.asset),
            resolution: modes(&self.resolution),
            version: modes(&self.version),
        };
        let tasks: Vec<(String, Vec<String>)> = groups
            .into_iter()
            .map(|(label, texts, _)| (label, texts))
            .collect();
        let document = PatternsDocument {
            shot: texts(&self.shot),
            task: OrderedPairs(&tasks),
            version: texts(&self.version),
            resolution: texts(&self.resolution),
            asset: texts(&self.asset),
            stage: texts(&self.stage),
            rule_modes: modes,
        };
        serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".to_string())
    }

    /// Task rules grouped by configured label, each with its mode in the same
    /// order: exactly the shape `taskPatterns` + `ruleModes.task` round-trips.
    fn task_groups(&self) -> Vec<(String, Vec<String>, Vec<String>)> {
        let mut grouped: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
        for rule in &self.task {
            let Some(label) = rule.task.clone() else {
                continue;
            };
            let mode = rule.mode.name().to_string();
            match grouped.iter_mut().find(|(known, _, _)| known == &label) {
                Some((_, texts, modes)) => {
                    texts.push(rule.text.clone());
                    modes.push(mode);
                }
                None => grouped.push((label, vec![rule.text.clone()], vec![mode])),
            }
        }
        grouped
    }

    /// Reads the reference file shape. A missing key means "no patterns in that
    /// category", exactly as the reference `dict.get(key, [])` does.
    pub fn from_json(text: &str) -> Result<Self, PatternError> {
        let parsed: PatternsDocumentIn = serde_json::from_str(text).map_err(|error| {
            PatternError::format(None, format!("the pattern file is not readable: {error}"))
        })?;
        let modes = parsed.rule_modes.unwrap_or_default();
        let mut set = Self {
            shot: zip_rules(parsed.shot, &modes.shot),
            stage: zip_rules(parsed.stage, &modes.stage),
            task: Vec::new(),
            asset: zip_rules(parsed.asset, &modes.asset),
            resolution: zip_rules(parsed.resolution, &modes.resolution),
            version: zip_rules(parsed.version, &modes.version),
        };
        let mut task_modes = modes.task.iter();
        for (label, texts) in parsed.task {
            for text in texts {
                let mode = task_modes
                    .next()
                    .and_then(|name| RuleMode::from_name(name))
                    .unwrap_or(RuleMode::Regex);
                set.task.push(PatternRule {
                    task: Some(label.clone()),
                    text,
                    mode,
                });
            }
        }
        Ok(set)
    }
}

fn offset(index: usize, delta: isize, length: usize) -> Option<usize> {
    if delta == 0 || length == 0 {
        return None;
    }
    let target = index as isize + delta;
    if target < 0 || target >= length as isize {
        return None;
    }
    Some(target as usize)
}

fn texts(rules: &[PatternRule]) -> Vec<String> {
    rules.iter().map(|rule| rule.text.clone()).collect()
}

fn modes(rules: &[PatternRule]) -> Vec<String> {
    rules
        .iter()
        .map(|rule| rule.mode.name().to_string())
        .collect()
}

fn zip_rules(texts: Vec<String>, modes: &[String]) -> Vec<PatternRule> {
    texts
        .into_iter()
        .enumerate()
        .map(|(index, text)| PatternRule {
            task: None,
            text,
            mode: modes
                .get(index)
                .and_then(|name| RuleMode::from_name(name))
                .unwrap_or(RuleMode::Regex),
        })
        .collect()
}

/// Ordered map serialization. `serde_json`'s default `Map` sorts keys, and both
/// pattern and profile order is precedence, so the reference file order is
/// emitted explicitly instead of sorted.
struct OrderedPairs<'a, T>(&'a [(String, T)]);

impl<T: Serialize> Serialize for OrderedPairs<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

#[derive(Serialize)]
struct PatternsDocument<'a> {
    #[serde(rename = "shotPatterns")]
    shot: Vec<String>,
    #[serde(rename = "taskPatterns")]
    task: OrderedPairs<'a, Vec<String>>,
    #[serde(rename = "versionPatterns")]
    version: Vec<String>,
    #[serde(rename = "resolutionPatterns")]
    resolution: Vec<String>,
    #[serde(rename = "assetPatterns")]
    asset: Vec<String>,
    #[serde(rename = "stagePatterns")]
    stage: Vec<String>,
    #[serde(rename = "ruleModes")]
    rule_modes: RuleModes,
}

#[derive(Default, Serialize, Deserialize)]
struct RuleModes {
    #[serde(default)]
    shot: Vec<String>,
    #[serde(default)]
    stage: Vec<String>,
    #[serde(default)]
    task: Vec<String>,
    #[serde(default)]
    asset: Vec<String>,
    #[serde(default)]
    resolution: Vec<String>,
    #[serde(default)]
    version: Vec<String>,
}

#[derive(Deserialize)]
struct PatternsDocumentIn {
    #[serde(rename = "shotPatterns", default)]
    shot: Vec<String>,
    #[serde(
        rename = "taskPatterns",
        default,
        deserialize_with = "ordered_string_lists"
    )]
    task: Vec<(String, Vec<String>)>,
    #[serde(rename = "versionPatterns", default)]
    version: Vec<String>,
    #[serde(rename = "resolutionPatterns", default)]
    resolution: Vec<String>,
    #[serde(rename = "assetPatterns", default)]
    asset: Vec<String>,
    #[serde(rename = "stagePatterns", default)]
    stage: Vec<String>,
    #[serde(rename = "ruleModes", default)]
    rule_modes: Option<RuleModes>,
}

fn ordered_string_lists<'de, D>(deserializer: D) -> Result<Vec<(String, Vec<String>)>, D::Error>
where
    D: Deserializer<'de>,
{
    struct Lists;
    impl<'de> Visitor<'de> for Lists {
        type Value = Vec<(String, Vec<String>)>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an object of string lists")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some((key, values)) = access.next_entry::<String, Vec<String>>()? {
                out.push((key, values));
            }
            Ok(out)
        }
    }
    deserializer.deserialize_map(Lists)
}

/// One compiled pattern: a cached expression, an explicit literal rule, or an
/// expression the engine refused (which matches nothing).
#[derive(Clone, Debug)]
struct CompiledRule {
    text: String,
    matcher: Matcher,
    /// Set when running this rule hit the engine's bounded execution limit. It
    /// describes the pattern, not one filename, so it stays set and is reported
    /// by [`CompiledPatterns::bounded_issues`].
    bounded: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
enum Matcher {
    /// A compiled expression. `is_match`/`find` are searches, like `re.search`:
    /// the first match may start anywhere in the filename.
    Regex(Regex),
    /// Explicit literal mode: case-insensitive containment of the rule text.
    Literal(String),
    /// The engine refused the expression. It matches nothing and the rule is
    /// reported; the text is never reinterpreted as containment.
    Unsupported,
}

impl CompiledRule {
    fn new(rule: &PatternRule) -> Self {
        let matcher = match rule.mode {
            RuleMode::Literal => Matcher::Literal(rule.text.to_lowercase()),
            RuleMode::Regex => match compile_rule(rule) {
                Ok(regex) => Matcher::Regex(regex),
                Err(_) => Matcher::Unsupported,
            },
        };
        Self {
            text: rule.text.clone(),
            matcher,
            bounded: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The reference "regex" answer: whether the expression matched anywhere.
    fn matched(&self, filename: &str) -> bool {
        match &self.matcher {
            Matcher::Regex(regex) => match regex.is_match(filename) {
                Ok(matched) => matched,
                Err(_) => {
                    self.stop_on_bound();
                    false
                }
            },
            Matcher::Literal(folded) => filename.to_lowercase().contains(folded),
            Matcher::Unsupported => false,
        }
    }

    /// The reference "simple string" answer: the pattern text itself.
    fn text_on_match(&self, filename: &str) -> Option<String> {
        self.matched(filename).then(|| self.text.clone())
    }

    /// The reference regex answer for shot/version/resolution: the matched
    /// substring, or the pattern text when an explicit literal rule matched.
    fn value_on_match(&self, filename: &str) -> Option<String> {
        match &self.matcher {
            Matcher::Regex(regex) => match regex.find(filename) {
                Ok(hit) => hit.map(|hit| hit.as_str().to_string()),
                Err(_) => {
                    self.stop_on_bound();
                    None
                }
            },
            Matcher::Literal(folded) => filename
                .to_lowercase()
                .contains(folded)
                .then(|| self.text.clone()),
            Matcher::Unsupported => None,
        }
    }

    /// Records that the bound stopped this rule. The runtime limit is not a
    /// reason to invent a different meaning for the expression, so the filename
    /// simply gets no answer from this rule.
    fn stop_on_bound(&self) {
        self.bounded.store(true, Ordering::Relaxed);
    }
}

fn compile_rules(rules: &[PatternRule]) -> Vec<CompiledRule> {
    rules.iter().map(CompiledRule::new).collect()
}

/// Backtracking steps allowed for one rule against one filename. A rule is
/// attempted at most once per filename and filenames are a few hundred bytes, so
/// a legitimate look-around or backreference expression needs a small fraction of
/// this. Reaching it never changes the pattern: the rule has no answer for that
/// filename and is reported instead.
const BACKTRACK_LIMIT: usize = 200_000;

/// Compile bounds, in delegated-automaton bytes and VM instructions. A pattern
/// that needs more is refused and reported rather than turning the configuration
/// load into an unbounded compile.
const DELEGATE_SIZE_LIMIT: usize = 1 << 20;
const MAX_PROGRAM_SIZE: usize = 20_000;

fn compile_rule(rule: &PatternRule) -> Result<Regex, String> {
    // The reference compiled every pattern with `re.IGNORECASE`.
    RegexBuilder::new(&rule.text)
        .case_insensitive(true)
        .delegate_size_limit(DELEGATE_SIZE_LIMIT)
        .max_prog_size(MAX_PROGRAM_SIZE)
        .backtrack_limit(BACKTRACK_LIMIT)
        .build()
        .map_err(|error| {
            error
                .to_string()
                .replace("regex parse error:\n", "")
                .replace('\n', " ")
                .trim()
                .to_string()
        })
}

/// One compiled configuration, reused for every filename in a run.
#[derive(Clone, Debug)]
pub struct CompiledPatterns {
    shot: Vec<CompiledRule>,
    stage: Vec<CompiledRule>,
    task: Vec<(String, Vec<CompiledRule>)>,
    asset: Vec<CompiledRule>,
    resolution: Vec<CompiledRule>,
    version: Vec<CompiledRule>,
}

impl CompiledPatterns {
    /// Rules that hit the bounded execution limit while matching. Each one
    /// answered "no match" for the affected filenames; no expression was
    /// rewritten and no rule was converted to plain text.
    pub fn bounded_issues(&self) -> Vec<DialectIssue> {
        let mut issues = Vec::new();
        let mut collect = |kind: TagKind, task: Option<String>, rule: &CompiledRule| {
            if rule.bounded.load(Ordering::Relaxed) {
                issues.push(DialectIssue {
                    kind,
                    task,
                    text: rule.text.clone(),
                    message: bounded_message(),
                });
            }
        };
        for (kind, rules) in [
            (TagKind::Shot, &self.shot),
            (TagKind::Stage, &self.stage),
            (TagKind::Asset, &self.asset),
            (TagKind::Resolution, &self.resolution),
            (TagKind::Version, &self.version),
        ] {
            for rule in rules {
                collect(kind, None, rule);
            }
        }
        for (label, rules) in &self.task {
            for rule in rules {
                collect(TagKind::Task, Some(label.clone()), rule);
            }
        }
        issues
    }

    /// Extracts every category from one filename. Only the filename is
    /// consulted, per the reference rule.
    pub fn extract(&self, filename: &str) -> TagSet {
        TagSet {
            shot: first_value(&self.shot, filename),
            stage: first_text(&self.stage, filename),
            task: self
                .task
                .iter()
                .find(|(_, rules)| rules.iter().any(|rule| rule.matched(filename)))
                .map(|(label, _)| label.clone()),
            asset: first_text(&self.asset, filename),
            resolution: first_value(&self.resolution, filename),
            version: first_value(&self.version, filename),
        }
    }
}

fn bounded_message() -> String {
    format!(
        "stopped at the bounded execution limit ({BACKTRACK_LIMIT} backtracking steps per filename) and matched nothing there — the expression is unchanged; simplify it, or mark it Literal if plain-text matching is what was meant"
    )
}

fn first_value(rules: &[CompiledRule], filename: &str) -> Option<String> {
    rules.iter().find_map(|rule| rule.value_on_match(filename))
}

fn first_text(rules: &[CompiledRule], filename: &str) -> Option<String> {
    rules.iter().find_map(|rule| rule.text_on_match(filename))
}

/// The tags of one item, in reference category order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagSet {
    pub shot: Option<String>,
    pub stage: Option<String>,
    pub task: Option<String>,
    pub asset: Option<String>,
    pub resolution: Option<String>,
    pub version: Option<String>,
}

impl TagSet {
    pub fn get(&self, kind: TagKind) -> Option<&str> {
        match kind {
            TagKind::Shot => self.shot.as_deref(),
            TagKind::Stage => self.stage.as_deref(),
            TagKind::Task => self.task.as_deref(),
            TagKind::Asset => self.asset.as_deref(),
            TagKind::Resolution => self.resolution.as_deref(),
            TagKind::Version => self.version.as_deref(),
        }
    }

    pub fn set(&mut self, kind: TagKind, value: Option<String>) {
        let value = value.map(|text| text.trim().to_string());
        let value = value.filter(|text| !text.is_empty());
        match kind {
            TagKind::Shot => self.shot = value,
            TagKind::Stage => self.stage = value,
            TagKind::Task => self.task = value,
            TagKind::Asset => self.asset = value,
            TagKind::Resolution => self.resolution = value,
            TagKind::Version => self.version = value,
        }
    }

    /// True when no category matched at all.
    pub fn is_empty(&self) -> bool {
        TagKind::ALL.iter().all(|kind| self.get(*kind).is_none())
    }
}

/// Explicit per-item tag edits. `Some("")` clears a tag; `None` keeps the
/// extracted value. Batch edits apply the same shape to every selected item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagEdits {
    pub shot: Option<String>,
    pub stage: Option<String>,
    pub task: Option<String>,
    pub asset: Option<String>,
    pub resolution: Option<String>,
    pub version: Option<String>,
}

impl TagEdits {
    pub fn get(&self, kind: TagKind) -> Option<&String> {
        match kind {
            TagKind::Shot => self.shot.as_ref(),
            TagKind::Stage => self.stage.as_ref(),
            TagKind::Task => self.task.as_ref(),
            TagKind::Asset => self.asset.as_ref(),
            TagKind::Resolution => self.resolution.as_ref(),
            TagKind::Version => self.version.as_ref(),
        }
    }

    pub fn set(&mut self, kind: TagKind, value: Option<String>) {
        match kind {
            TagKind::Shot => self.shot = value,
            TagKind::Stage => self.stage = value,
            TagKind::Task => self.task = value,
            TagKind::Asset => self.asset = value,
            TagKind::Resolution => self.resolution = value,
            TagKind::Version => self.version = value,
        }
    }

    pub fn is_empty(&self) -> bool {
        TagKind::ALL.iter().all(|kind| self.get(*kind).is_none())
    }

    /// Applies the edits over the extracted tags.
    pub fn apply(&self, extracted: &TagSet) -> TagSet {
        let mut tags = extracted.clone();
        for kind in TagKind::ALL {
            if let Some(text) = self.get(kind) {
                tags.set(kind, Some(text.clone()));
            }
        }
        tags
    }
}

/// Outcome class of one proposal, `CONTRACTS.md` §5 `MappingState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingState {
    /// A task or asset rule matched exactly.
    Matched,
    /// No rule matched and the footage rule (or the visible fallback folder) was
    /// used.
    FootageFallback,
    /// Two or more distinct rules match parts of the same tag.
    Ambiguous,
    /// No profile rule at all, so no destination exists.
    Unmatched,
    /// The computed destination is not a usable relative path.
    Invalid,
}

impl MappingState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Matched => "Matched",
            Self::FootageFallback => "Footage",
            Self::Ambiguous => "Ambiguous",
            Self::Unmatched => "Unmatched",
            Self::Invalid => "Invalid",
        }
    }

    /// Only a resolved destination may be executed.
    pub fn is_executable(self) -> bool {
        matches!(self, Self::Matched | Self::FootageFallback)
    }
}

/// One ordered destination-path rule: a relative destination folder and the
/// keywords that select it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingRule {
    pub path: String,
    pub keywords: Vec<String>,
}

impl IncomingRule {
    /// The rule's folder segments, validated as a plain relative path.
    pub fn segments(&self) -> Result<Vec<String>, String> {
        split_relative(&self.path)
    }
}

/// One named profile: ordered rules, `CONTRACTS.md` §5 `IncomingProfile`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingProfile {
    pub name: String,
    pub rules: Vec<IncomingRule>,
}

impl IncomingProfile {
    pub fn new(name: impl Into<String>, rules: Vec<IncomingRule>) -> Self {
        Self {
            name: name.into(),
            rules,
        }
    }

    /// The reference `config/profiles.json` contents, read through the same
    /// parser as an explicit import.
    pub fn reference_defaults() -> Vec<Self> {
        ProfileSet::from_json(REFERENCE_PROFILES_JSON)
            .map(|set| set.profiles)
            .unwrap_or_default()
    }

    /// The legacy `{"name": ..., "rules": [...]}` object this profile exports.
    pub fn to_json(&self) -> String {
        let rules: Vec<RuleObject> = self
            .rules
            .iter()
            .map(|rule| RuleObject(vec![(rule.path.clone(), rule.keywords.clone())]))
            .collect();
        let document = ProfileDocument {
            name: self.name.clone(),
            rules,
        };
        serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".to_string())
    }
}

/// One profile's rule object, keeping the reference single-key-per-object shape
/// while preserving rule order.
struct RuleObject(Vec<(String, Vec<String>)>);

impl Serialize for RuleObject {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        OrderedPairs(&self.0).serialize(serializer)
    }
}

#[derive(Serialize)]
struct ProfileDocument {
    name: String,
    rules: Vec<RuleObject>,
}

/// The ordered profile collection of one `profiles.json` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileSet {
    pub profiles: Vec<IncomingProfile>,
}

impl Default for ProfileSet {
    fn default() -> Self {
        Self {
            profiles: IncomingProfile::reference_defaults(),
        }
    }
}

impl ProfileSet {
    pub fn names(&self) -> Vec<String> {
        self.profiles.iter().map(|p| p.name.clone()).collect()
    }

    pub fn find(&self, name: &str) -> Option<&IncomingProfile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    pub fn position(&self, name: &str) -> Option<usize> {
        self.profiles
            .iter()
            .position(|profile| profile.name == name)
    }

    /// Adds a profile or replaces the one with the same name in place, so an
    /// import cannot silently duplicate a name.
    pub fn upsert(&mut self, profile: IncomingProfile) {
        match self
            .profiles
            .iter_mut()
            .find(|known| known.name == profile.name)
        {
            Some(slot) => *slot = profile,
            None => self.profiles.push(profile),
        }
    }

    /// The legacy file shape: profile name → ordered rule objects.
    pub fn to_json(&self) -> String {
        let entries: Vec<(String, Vec<RuleObject>)> = self
            .profiles
            .iter()
            .map(|profile| {
                let rules = profile
                    .rules
                    .iter()
                    .map(|rule| RuleObject(vec![(rule.path.clone(), rule.keywords.clone())]))
                    .collect();
                (profile.name.clone(), rules)
            })
            .collect();
        let document = OrderedPairs(&entries);
        serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".to_string())
    }

    /// Reads the legacy map shape, one `{rules: [...]}` profile object, or a
    /// bare rule list. Every accepted shape keeps rule order.
    pub fn from_json(text: &str) -> Result<Self, PatternError> {
        let parsed: ProfilesFile = serde_json::from_str(text).map_err(|error| {
            PatternError::format(
                None,
                format!("the profile file is not readable as rules: {error}"),
            )
        })?;
        let profiles = match parsed {
            ProfilesFile::Single { name, rules } => vec![IncomingProfile::new(
                name.unwrap_or_else(|| "Imported profile".to_string()),
                flatten_rules(rules)?,
            )],
            ProfilesFile::List(rules) => vec![IncomingProfile::new(
                "Imported profile",
                flatten_rules(rules)?,
            )],
            ProfilesFile::Map(entries) => {
                let mut profiles = Vec::with_capacity(entries.len());
                for (name, value) in entries {
                    let (inner_name, rules) = match value {
                        ProfileValue::List(rules) => (None, rules),
                        ProfileValue::Rules { name, rules } => (name, rules),
                    };
                    profiles.push(IncomingProfile::new(
                        inner_name.unwrap_or(name),
                        flatten_rules(rules)?,
                    ));
                }
                profiles
            }
        };
        if profiles.is_empty() {
            return Err(PatternError::format(
                None,
                "the profile file declares no profile",
            ));
        }
        Ok(Self { profiles })
    }
}

fn flatten_rules(rules: Vec<RawRule>) -> Result<Vec<IncomingRule>, PatternError> {
    let mut out = Vec::new();
    for rule in rules {
        for (path, keywords) in rule.0 {
            let path = path.trim().to_string();
            if path.is_empty() {
                return Err(PatternError::format(
                    None,
                    "a profile rule names an empty destination folder",
                ));
            }
            out.push(IncomingRule {
                path,
                keywords: keywords
                    .into_iter()
                    .map(|word| word.trim().to_string())
                    .filter(|word| !word.is_empty())
                    .collect(),
            });
        }
    }
    Ok(out)
}

/// One rule object: ordered destination → keywords pairs.
#[derive(Deserialize)]
struct RawRule(#[serde(deserialize_with = "ordered_string_lists")] Vec<(String, Vec<String>)>);

#[derive(Deserialize)]
#[serde(untagged)]
enum ProfileValue {
    List(Vec<RawRule>),
    Rules {
        rules: Vec<RawRule>,
        #[serde(default)]
        name: Option<String>,
    },
}

fn ordered_profile_values<'de, D>(deserializer: D) -> Result<Vec<(String, ProfileValue)>, D::Error>
where
    D: Deserializer<'de>,
{
    struct Profiles;
    impl<'de> Visitor<'de> for Profiles {
        type Value = Vec<(String, ProfileValue)>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an object of profiles")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some((name, value)) = access.next_entry::<String, ProfileValue>()? {
                out.push((name, value));
            }
            Ok(out)
        }
    }
    deserializer.deserialize_map(Profiles)
}

/// A single profile object is tried first because its `rules` member is
/// required, so a legacy `{"Profile A": [...]}` map still parses as the map it
/// is.
#[derive(Deserialize)]
#[serde(untagged)]
enum ProfilesFile {
    Single {
        rules: Vec<RawRule>,
        #[serde(default)]
        name: Option<String>,
    },
    List(Vec<RawRule>),
    Map(#[serde(deserialize_with = "ordered_profile_values")] Vec<(String, ProfileValue)>),
}

/// One route resolution: the destination folder segments plus how they were
/// chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub state: MappingState,
    /// Destination-relative folder segments.
    pub segments: Vec<String>,
    /// The rule path the segments start with, for the review rows.
    pub base: Option<String>,
    /// Distinct candidate folders when the match is ambiguous.
    pub alternatives: Vec<String>,
    pub used_footage_fallback: bool,
    pub reason: String,
}

impl Route {
    fn resolved(
        rule: &IncomingRule,
        tags: &TagSet,
        state: MappingState,
        used_footage_fallback: bool,
        reason: String,
    ) -> Self {
        match rule.segments() {
            Ok(mut segments) => {
                segments.extend(dynamic_segments(tags));
                Self {
                    state,
                    segments,
                    base: Some(rule.path.clone()),
                    alternatives: Vec::new(),
                    used_footage_fallback,
                    reason,
                }
            }
            Err(error) => Self {
                state: MappingState::Invalid,
                segments: Vec::new(),
                base: Some(rule.path.clone()),
                alternatives: Vec::new(),
                used_footage_fallback,
                reason: error,
            },
        }
    }

    /// The folder as one relative display path, regardless of platform.
    pub fn folder_display(&self) -> String {
        self.segments.join("/")
    }
}

/// A dot-prefixed folder keyword matches the complete file extension before tag routing.
pub fn route_file(profile: &IncomingProfile, tags: &TagSet, filename: &str) -> Route {
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str());
    if let Some(extension) = extension {
        let matches: Vec<_> = profile
            .rules
            .iter()
            .filter(|rule| {
                rule.keywords.iter().any(|word| {
                    word.trim()
                        .strip_prefix('.')
                        .is_some_and(|word| word.eq_ignore_ascii_case(extension))
                })
            })
            .collect();
        if let Some(rule) = matches.first() {
            if matches.iter().any(|other| other.path != rule.path) {
                return Route {
                    state: MappingState::Ambiguous,
                    segments: Vec::new(),
                    base: None,
                    alternatives: matches.iter().map(|rule| rule.path.clone()).collect(),
                    used_footage_fallback: false,
                    reason: format!("extension .{extension} matches multiple destination folders"),
                };
            }
            return Route::resolved(
                rule,
                tags,
                MappingState::Matched,
                false,
                format!("extension .{extension} matched this rule"),
            );
        }
    }
    route(profile, tags)
}

/// Resolves a destination folder from a profile and one item's tags, following
/// `generate_simple_target_path` step by step.
pub fn route(profile: &IncomingProfile, tags: &TagSet) -> Route {
    let task = normalized(tags.task.as_deref());
    let asset = normalized(tags.asset.as_deref());

    // 1. Exact task match, in rule order.
    if let Some(task) = &task {
        if let Some(rule) = profile.rules.iter().find(|rule| {
            rule.keywords
                .iter()
                .any(|word| normalized(Some(word)).as_deref() == Some(task.as_str()))
        }) {
            return Route::resolved(
                rule,
                tags,
                MappingState::Matched,
                false,
                format!(
                    "task '{}' matched this rule",
                    tags.get(TagKind::Task).unwrap_or_default()
                ),
            );
        }
    }

    // 2. Exact asset match.
    if let Some(asset) = &asset {
        if let Some(rule) = profile.rules.iter().find(|rule| {
            rule.keywords
                .iter()
                .any(|word| normalized(Some(word)).as_deref() == Some(asset.as_str()))
        }) {
            return Route::resolved(
                rule,
                tags,
                MappingState::Matched,
                false,
                format!(
                    "asset '{}' matched this rule",
                    tags.get(TagKind::Asset).unwrap_or_default()
                ),
            );
        }
    }

    // 3. Partial matches on the same value that point at different rules are
    // ambiguous, and an ambiguous proposal is never executable.
    let keyword_paths = keyword_paths(profile);
    for (kind, value) in [(TagKind::Task, &task), (TagKind::Asset, &asset)] {
        let Some(value) = value else {
            continue;
        };
        let label = kind.label().to_lowercase();
        let mut hits: Vec<(&String, &String)> = keyword_paths
            .iter()
            .filter(|(keyword, _)| value.contains(keyword.as_str()))
            .collect();
        // Sorted by keyword, as the reference sorted its options.
        hits.sort_by(|left, right| left.0.cmp(right.0));
        let mut paths: Vec<String> = Vec::new();
        for (_, path) in &hits {
            if !paths.iter().any(|known| known == *path) {
                paths.push((*path).clone());
            }
        }
        if paths.len() > 1 {
            let keywords: Vec<&str> = hits.iter().map(|(keyword, _)| keyword.as_str()).collect();
            return Route {
                state: MappingState::Ambiguous,
                segments: Vec::new(),
                base: None,
                alternatives: paths.clone(),
                used_footage_fallback: false,
                reason: format!(
                    "{label} '{}' contains keywords that route to {} different folders ({}); edit the tags or add an exact rule",
                    tags.get(kind).unwrap_or_default(),
                    paths.len(),
                    keywords.join(", ")
                ),
            };
        }
    }

    // 4. Footage fallback: the documented default rule, then any rule holding a
    // default keyword, then the visible unmapped folder. The reference compared
    // the rule's keyword *set* with the documented set, so both sides are
    // compared order-free; a profile with no rules at all falls through to the
    // same visible fallback folder instead of being refused outright.
    let mut default_set: Vec<String> = DEFAULT_FOOTAGE_KEYWORDS
        .iter()
        .map(|word| word.to_lowercase())
        .collect();
    default_set.sort();
    default_set.dedup();
    if let Some(rule) = profile.rules.iter().find(|rule| {
        let mut words: Vec<String> = rule
            .keywords
            .iter()
            .map(|word| word.to_lowercase())
            .collect();
        words.sort();
        words.dedup();
        words == default_set
    }) {
        return Route::resolved(
            rule,
            tags,
            MappingState::FootageFallback,
            true,
            "no task or asset rule matched; the default footage rule applies".to_string(),
        );
    }
    if let Some(rule) = profile.rules.iter().find(|rule| {
        rule.keywords.iter().any(|word| {
            let word = word.to_lowercase();
            DEFAULT_FOOTAGE_KEYWORDS.contains(&word.as_str())
        })
    }) {
        return Route::resolved(
            rule,
            tags,
            MappingState::FootageFallback,
            true,
            "no task or asset rule matched; the footage rule containing a default keyword applies"
                .to_string(),
        );
    }
    Route {
        state: MappingState::FootageFallback,
        segments: {
            let mut segments = vec![UNMAPPED_FALLBACK.to_string()];
            segments.extend(dynamic_segments(tags));
            segments
        },
        base: Some(UNMAPPED_FALLBACK.to_string()),
        alternatives: Vec::new(),
        used_footage_fallback: true,
        reason:
            "no rule matched and no footage rule is configured; this folder is the visible fallback"
                .to_string(),
    }
}

fn normalized(value: Option<&str>) -> Option<String> {
    value
        .map(|text| text.trim().to_lowercase())
        .filter(|text| !text.is_empty())
}

/// Keyword → folder map with the reference "last rule wins" behavior, ordered by
/// keyword so an ambiguous resolution reports a stable list.
fn keyword_paths(profile: &IncomingProfile) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for rule in &profile.rules {
        for keyword in &rule.keywords {
            let keyword = keyword.trim().to_lowercase();
            if keyword.is_empty() {
                continue;
            }
            map.insert(keyword, rule.path.trim().to_string());
        }
    }
    map
}

/// `shot/stage/task/asset/resolution/version`, in reference order and case.
fn dynamic_segments(tags: &TagSet) -> Vec<String> {
    let order = [
        TagKind::Shot,
        TagKind::Stage,
        TagKind::Task,
        TagKind::Asset,
        TagKind::Resolution,
        TagKind::Version,
    ];
    let mut segments = Vec::new();
    for kind in order {
        let Some(value) = tags.get(kind) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let segment = match kind {
            // Shot keeps the case the filename used.
            TagKind::Shot => value.to_string(),
            // Resolution is uppercased, as the reference does.
            TagKind::Resolution => value.to_uppercase(),
            // Stage, task, asset and version are lowercased.
            _ => value.to_lowercase(),
        };
        // A dynamic value cannot become a path separator: it comes from one
        // filename component, so this only guards against separator characters
        // that a Windows filename can contain in an imported rule.
        let segment = segment.replace(['/', '\\'], "_");
        segments.push(segment);
    }
    segments
}

/// Splits a rule path on either separator into validated relative segments:
/// empty and `.` are dropped, `..` and anything absolute is refused.
pub fn split_relative(path: &str) -> Result<Vec<String>, String> {
    if path.trim_start().starts_with(['/', '\\']) {
        return Err("Choose a folder inside the destination, using a relative path.".to_string());
    }
    let mut segments = Vec::new();
    for part in path.split(['/', '\\']) {
        let part = part.trim();
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(format!(
                "rule folder \"{path}\" contains '..', which would leave the destination root"
            ));
        }
        if part.contains(':') {
            return Err(format!(
                "rule folder \"{path}\" contains a drive or stream separator"
            ));
        }
        segments.push(part.to_string());
    }
    if segments.is_empty() {
        return Err(format!("rule folder \"{path}\" names no folder"));
    }
    Ok(segments)
}

/// Filesystem location of the editable pattern and profile files.
#[derive(Clone, Debug)]
pub struct PatternStore {
    directory: PathBuf,
}

impl PatternStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            directory: data_dir.join("incoming"),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn patterns_path(&self) -> PathBuf {
        self.directory.join("patterns.json")
    }

    pub fn profiles_path(&self) -> PathBuf {
        self.directory.join("profiles.json")
    }

    /// Reads both files. A missing file yields the reference defaults and is
    /// written so the user has something to edit; `seeded` says that happened.
    pub fn load(&self) -> Result<StoredPatterns, PatternError> {
        fs::create_dir_all(&self.directory).map_err(|error| {
            PatternError::io(
                Some(&self.directory),
                format!("the pattern folder could not be created: {error}"),
            )
        })?;
        let (mut patterns, mut patterns_seeded) = self.load_one(
            &self.patterns_path(),
            PatternSet::reference_defaults(),
            PatternSet::to_json,
            PatternSet::from_json,
        )?;
        // Upgrade only the unchanged shipped ARRI rule; preserve user edits.
        for rule in patterns.rules_mut(TagKind::Task) {
            if rule.task.as_deref() == Some("ARRI_Camera_File_Task")
                && rule.mode == RuleMode::Regex
                && rule.text
                    == r"^([A-Z]\d{3}C\d{3})_\d{6}_[A-Z]\d+[A-Z0-9]*\.(?i)(mxf|ari|mov|mp4)$"
            {
                rule.text = r"^([A-Z]_?\d{3,4}C\d{3})_\d{6}_(?:\d{6}_[A-Za-z0-9]+|[A-Z]\d+[A-Z0-9]*)\.(?i)(mxf|ari|mov|mp4)$".into();
                patterns_seeded = true;
            }
        }
        if patterns_seeded {
            self.save_patterns(&patterns)?;
        }
        let (profiles, profiles_seeded) = self.load_one(
            &self.profiles_path(),
            ProfileSet::default(),
            ProfileSet::to_json,
            ProfileSet::from_json,
        )?;
        Ok(StoredPatterns {
            patterns,
            profiles,
            seeded: patterns_seeded || profiles_seeded,
        })
    }

    fn load_one<T: Clone>(
        &self,
        path: &Path,
        default: T,
        to_json: fn(&T) -> String,
        from_json: fn(&str) -> Result<T, PatternError>,
    ) -> Result<(T, bool), PatternError> {
        match fs::read_to_string(path) {
            Ok(text) => Ok((from_json(&text).map_err(|error| error.at(path))?, false)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_text(path, &to_json(&default))?;
                Ok((default, true))
            }
            Err(error) => Err(PatternError::io(
                Some(path),
                format!("the file could not be read: {error}"),
            )),
        }
    }

    pub fn save_patterns(&self, set: &PatternSet) -> Result<(), PatternError> {
        write_text(&self.patterns_path(), &set.to_json())
    }

    pub fn save_profiles(&self, set: &ProfileSet) -> Result<(), PatternError> {
        write_text(&self.profiles_path(), &set.to_json())
    }

    /// Reads a `patterns.json` the user chose, without changing the saved file.
    pub fn import_patterns(&self, path: &Path) -> Result<PatternSet, PatternError> {
        let text = read_text(path)?;
        PatternSet::from_json(&text).map_err(|error| error.at(path))
    }

    /// Reads a profile file in any accepted shape.
    pub fn import_profiles(&self, path: &Path) -> Result<ProfileSet, PatternError> {
        let text = read_text(path)?;
        ProfileSet::from_json(&text).map_err(|error| error.at(path))
    }

    /// Writes text the user chose as an export target.
    pub fn write_export(&self, path: &Path, text: &str) -> Result<(), PatternError> {
        write_text(path, text)
    }
}

/// `patterns.json`/`profiles.json` state as loaded from disk.
pub struct StoredPatterns {
    pub patterns: PatternSet,
    pub profiles: ProfileSet,
    /// True when a default file was created because none existed yet.
    pub seeded: bool,
}

fn read_text(path: &Path) -> Result<String, PatternError> {
    fs::read_to_string(path).map_err(|error| {
        PatternError::io(Some(path), format!("the file could not be read: {error}"))
    })
}

/// Replaces the file through a fresh temporary name in the same directory, so a
/// failed save cannot leave a half-written configuration behind.
fn write_text(path: &Path, text: &str) -> Result<(), PatternError> {
    let parent = path
        .parent()
        .ok_or_else(|| PatternError::io(Some(path), "the file has no parent directory"))?;
    fs::create_dir_all(parent).map_err(|error| {
        PatternError::io(
            Some(parent),
            format!("the pattern folder could not be created: {error}"),
        )
    })?;
    let mut temporary = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    temporary.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let temporary = parent.join(temporary);
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(PatternError::io(
            Some(&temporary),
            format!("the file could not be written: {error}"),
        ));
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(PatternError::io(
            Some(path),
            format!("the file could not be replaced: {error}"),
        ));
    }
    Ok(())
}

static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternErrorKind {
    Io,
    Format,
}

/// One configuration failure, with the file it came from when known.
#[derive(Clone, Debug)]
pub struct PatternError {
    kind: PatternErrorKind,
    path: Option<PathBuf>,
    message: String,
}

impl PatternError {
    pub fn io(path: Option<&Path>, message: impl Into<String>) -> Self {
        Self {
            kind: PatternErrorKind::Io,
            path: path.map(Path::to_path_buf),
            message: message.into(),
        }
    }

    pub fn format(path: Option<&Path>, message: impl Into<String>) -> Self {
        Self {
            kind: PatternErrorKind::Format,
            path: path.map(Path::to_path_buf),
            message: message.into(),
        }
    }

    fn at(mut self, path: &Path) -> Self {
        if self.path.is_none() {
            self.path = Some(path.to_path_buf());
        }
        self
    }

    pub fn kind(&self) -> PatternErrorKind {
        self.kind
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PatternError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => write!(formatter, "{}: {}", path.display(), self.message),
            None => write!(formatter, "{}", self.message),
        }
    }
}

impl std::error::Error for PatternError {}
