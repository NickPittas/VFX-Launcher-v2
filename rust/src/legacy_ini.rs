//! F3 legacy INI import: the `app_settings.ini` reader for the migration.
//!
//! The legacy application read this file through Python's `configparser`
//! (`core/config.py` constructs a plain `ConfigParser()`), so this module
//! reproduces the parts of that parser whose behavior is observable in the
//! settings a user actually has, rather than inventing a friendlier dialect:
//!
//! - option names are case-insensitive (`optionxform` lowercases them); section
//!   names stay case-sensitive, so `[paths]` is not `[Paths]`;
//! - `=` and `:` both separate an option from its value, and the first one on
//!   the line wins, so `nuke_launch_cmd_linux = ghostty -e env A=B ...` keeps
//!   everything after the first `=` as the command line;
//! - a value continues on following lines that are indented deeper than the
//!   option line; continuation lines are joined with `\n` and the joined value
//!   is trimmed at the end;
//! - a blank line does not end a value (`empty_lines_in_values`), and a line
//!   whose first non-blank character is `#` or `;` is a comment that is dropped
//!   without ending the value. Inline comment prefixes are not enabled, so `#`
//!   inside a value stays part of the value;
//! - `[DEFAULT]` values are visible from every section and a section value wins
//!   over the `[DEFAULT]` value with the same name;
//! - `%(name)s` is expanded from the section, falling back to `[DEFAULT]`, `%%`
//!   is a literal `%`, any other `%` use is a syntax error, and nesting stops at
//!   [`MAX_INTERPOLATION_DEPTH`] like `MAX_INTERPOLATION_DEPTH`;
//! - a repeated section, a repeated option inside one section and a line that is
//!   neither a header nor `option = value` are errors (the strict default).
//!
//! Deliberate limits:
//!
//! - it never touches the filesystem: the caller keeps the captured raw INI
//!   bytes and passes them in, so an import leaves the source untouched;
//! - text is decoded as UTF-8 and anything else is refused. The legacy app wrote
//!   the file with the locale encoding and read it as UTF-8 when launching, so a
//!   non-UTF-8 file is reported instead of being guessed at;
//! - an error is returned, never a partial result: a malformed file can never
//!   silently reset settings this application already holds;
//! - a relative or foreign executable spelling is never resolved against the
//!   working directory. Only an absolute native path, or a bare executable name
//!   the launcher resolves through `PATH`, becomes active; every other spelling
//!   is reported with its raw value, which stays with the import metadata;
//! - values this build has no consumer for (`scan_interval_minutes`,
//!   `log_file`, `DataDir`, `db_path`) are reported as inactive instead of being
//!   mapped onto guessed behavior.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::PathBuf;

use crate::config::{self, JsonPath, LogLevel};
use crate::paths::{PlatformTag, retain_imported};

/// Maximum nesting of `%(name)s` expansions, mirroring
/// `configparser.MAX_INTERPOLATION_DEPTH`.
const MAX_INTERPOLATION_DEPTH: usize = 10;

/// Legacy section holding the shared option values.
const DEFAULT_SECTION: &str = "DEFAULT";

const PATHS: &str = "Paths";
const PROJECTS: &str = "Projects";
const LOGGING: &str = "Logging";

/// Legacy options this build imports nothing from, with the reason a user can
/// read. The raw values are preserved by the caller, not dropped.
const INACTIVE_VALUES: [(&str, &str, &str); 4] = [
    (
        PROJECTS,
        "scan_interval_minutes",
        "scheduled rescanning is not part of this build",
    ),
    (
        LOGGING,
        "log_file",
        "logging to a file is not part of this build",
    ),
    (
        PATHS,
        "DataDir",
        "this build keeps its own per-user data directory",
    ),
    (
        "Database",
        "db_path",
        "the legacy catalog is read during import and never reopened as this build's catalog",
    ),
];

/// Imports one captured `app_settings.ini` over `base`.
///
/// `base` is the settings this installation already has: the returned value is
/// `base` with only the values the INI actually provides replaced, so an option
/// the INI omits keeps what the user already configured here. `selected_username`
/// and every other field the INI does not describe stay exactly as they were.
///
/// The second return value is the report: one readable line per legacy value
/// that was not applied (inactive options, unusable executable spellings,
/// unusable project folder names, unknown log levels). Nothing is dropped
/// silently — the caller records these next to the captured raw bytes.
///
/// `Err` means the file could not be imported and nothing was applied.
pub fn import_settings(
    bytes: &[u8],
    base: &config::Settings,
) -> Result<(config::Settings, Vec<String>), String> {
    import_settings_encoding(bytes, base, "utf-8")
}

/// Encoding is chosen explicitly at import; raw bytes remain in the capture.
pub fn import_settings_encoding(
    bytes: &[u8],
    base: &config::Settings,
    label: &str,
) -> Result<(config::Settings, Vec<String>), String> {
    let encoding =
        encoding_rs::Encoding::for_label_no_replacement(label.as_bytes()).ok_or_else(|| {
            format!(
                "The INI encoding {label:?} is not supported; choose the original file's encoding."
            )
        })?;
    let (text, invalid) = encoding.decode_with_bom_removal(bytes);
    if invalid {
        return Err(format!(
            "The INI contains invalid {} data; choose its original encoding. Nothing was imported.",
            encoding.name()
        ));
    }
    let ini = Ini::parse(&text)?;
    let mut settings = base.clone();
    let mut issues = Vec::new();

    if let Some(value) = ini.get(PATHS, "nuke_path")? {
        import_application_path(
            &mut settings.applications.nuke_path,
            "Paths/nuke_path",
            "Nuke",
            &value,
            &mut issues,
        );
    }
    if let Some(value) = ini.get(PATHS, "after_effects_path")? {
        import_application_path(
            &mut settings.applications.after_effects_path,
            "Paths/after_effects_path",
            "After Effects",
            &value,
            &mut issues,
        );
    }
    // A command line, not a path: it is tokenized at launch and never resolved
    // here, so it is stored as the user wrote it.
    if let Some(value) = ini.get(PATHS, "nuke_launch_cmd_linux")? {
        let command = value.trim();
        if !command.is_empty() {
            settings.applications.nuke_launch_cmd_linux = Some(command.to_string());
        }
    }
    if let Some(value) = ini.get(PROJECTS, "project_directories")? {
        import_project_directory_names(
            &mut settings.project_discovery.project_directory_names,
            &value,
            &mut issues,
        );
    }
    if let Some(value) = ini.get(LOGGING, "log_level")? {
        let spelling = value.trim();
        if !spelling.is_empty() {
            match log_level(spelling) {
                Some(level) => settings.log_level = level,
                None => issues.push(format!(
                    "The log level {spelling:?} from [Logging] log_level is not one of DEBUG, INFO, WARNING, ERROR or CRITICAL; the previous level is kept."
                )),
            }
        }
    }

    for (section, option, reason) in INACTIVE_VALUES {
        let Some(value) = ini.get(section, option)? else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        issues.push(format!(
            "The legacy value {value:?} from [{section}] {option} is not applied by this build: {reason}. It stays with the import metadata."
        ));
    }

    const MAPPED: &[(&str, &str)] = &[
        (PATHS, "nuke_path"),
        (PATHS, "after_effects_path"),
        (PATHS, "nuke_launch_cmd_linux"),
        (PROJECTS, "project_directories"),
        (LOGGING, "log_level"),
    ];
    for (section, values) in &ini.sections {
        for option in values.keys() {
            let known = MAPPED.iter().any(|(s, k)| *s == section && *k == option)
                || INACTIVE_VALUES
                    .iter()
                    .any(|(s, k, _)| *s == section && k.to_lowercase() == *option);
            if !known {
                issues.push(format!("The additional original setting [{section}] {option} is retained in capture.json; it has no active setting in this application."));
            }
        }
    }
    for option in ini.defaults.keys() {
        issues.push(format!("The original shared [DEFAULT] setting {option} is retained in capture.json. Recognized values are inherited only by sections present in the INI."));
    }

    Ok((settings, issues))
}

// ---------------------------------------------------------------------------
// Settings mapping
// ---------------------------------------------------------------------------

fn import_application_path(
    target: &mut Option<JsonPath>,
    reported: &str,
    label: &str,
    value: &str,
    issues: &mut Vec<String>,
) {
    let spelling = value.trim();
    // An empty option is the legacy "not configured" state, not a value to
    // import: the setting this installation already has is kept.
    if spelling.is_empty() {
        return;
    }
    match executable_path(spelling) {
        Ok(path) => *target = Some(JsonPath::new(path)),
        Err(reason) => issues.push(format!(
            "The {label} value {spelling:?} from {reported} stays inactive: {reason}. It is kept with the import metadata."
        )),
    }
}

/// An absolute native path, or a bare executable name the launcher resolves
/// through `PATH`. Anything else is refused with a reason instead of dialed
/// against a base the import cannot know.
fn executable_path(spelling: &str) -> Result<PathBuf, String> {
    if spelling.chars().any(char::is_control) {
        return Err("it contains a control character".to_string());
    }
    let imported = retain_imported(OsStr::new(spelling));
    match imported.tag {
        PlatformTag::Posix | PlatformTag::Windows => match imported.native {
            Some(path) if path.is_absolute() => Ok(path),
            _ => Err(format!(
                "it is a {} path, but this build runs on {}",
                platform_name(imported.tag),
                host_platform()
            )),
        },
        PlatformTag::Unknown => {
            let bare_name = !spelling.contains(['/', '\\', ':'])
                && !spelling.starts_with('~')
                && spelling != "."
                && spelling != "..";
            if bare_name {
                // One component with no separator: `PATH` search at launch.
                Ok(PathBuf::from(spelling))
            } else {
                Err(
                    "it is neither an absolute path nor a bare executable name, and a relative path is never resolved against the working directory"
                        .to_string(),
                )
            }
        }
    }
}

fn platform_name(tag: PlatformTag) -> &'static str {
    match tag {
        PlatformTag::Posix => "POSIX",
        PlatformTag::Windows => "Windows",
        PlatformTag::Unknown => "unrecognized",
    }
}

fn host_platform() -> &'static str {
    if cfg!(windows) { "Windows" } else { "POSIX" }
}

/// Legacy `project_directories` is a comma-separated list of folder names; the
/// scanner used `[name.strip() for name in raw.split(',')]`, so only the
/// surrounding whitespace is insignificant. Case variants are kept: on a
/// case-sensitive filesystem `Project` and `project` are different folders.
fn import_project_directory_names(target: &mut Vec<String>, value: &str, issues: &mut Vec<String>) {
    let mut names = Vec::new();
    for part in value.split(',') {
        let name = part.trim();
        if name.is_empty() {
            continue;
        }
        if name.contains('/') || name.contains('\\') || name.chars().any(char::is_control) {
            issues.push(format!(
                "The project directory name {name:?} from [Projects] project_directories is not a single folder name; it is skipped."
            ));
            continue;
        }
        names.push(name.to_string());
    }
    if names.is_empty() {
        if !value.trim().is_empty() {
            issues.push(
                "The legacy [Projects] project_directories value contains no usable folder name; the previous names are kept."
                    .to_string(),
            );
        }
        return;
    }
    *target = names;
}

/// `LogLevel` has four levels; the legacy combo box offered five spellings.
fn log_level(value: &str) -> Option<LogLevel> {
    match value.trim().to_ascii_uppercase().as_str() {
        "DEBUG" => Some(LogLevel::Debug),
        "INFO" => Some(LogLevel::Info),
        "WARNING" | "WARN" => Some(LogLevel::Warn),
        "ERROR" | "CRITICAL" => Some(LogLevel::Error),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parsed file: `[DEFAULT]` values and one map per named section.
struct Ini {
    defaults: BTreeMap<String, String>,
    sections: BTreeMap<String, BTreeMap<String, String>>,
}

impl Ini {
    fn parse(text: &str) -> Result<Self, String> {
        let mut builder = Builder::default();
        for (index, line) in text_lines(text).enumerate() {
            builder.line(line, index + 1)?;
        }
        Ok(builder.finish())
    }

    /// Raw (uninterpolated) value, section first, then `[DEFAULT]`, exactly the
    /// `ChainMap` a `ConfigParser.get` interpolates against.
    fn option(&self, section: &str, option: &str) -> Option<&str> {
        if section != DEFAULT_SECTION && !self.sections.contains_key(section) {
            return None;
        }
        if let Some(values) = self.sections.get(section) {
            if let Some(value) = values.get(option) {
                return Some(value);
            }
        }
        self.defaults.get(option).map(String::as_str)
    }

    fn get(&self, section: &str, option: &str) -> Result<Option<String>, String> {
        let Some(value) = self.option(section, option) else {
            return Ok(None);
        };
        if !value.contains('%') {
            return Ok(Some(value.to_string()));
        }
        let mut expanded = String::new();
        self.expand(section, option, &mut expanded, value, 1)?;
        Ok(Some(expanded))
    }

    /// `BasicInterpolation._interpolate_some`, including the depth counter that
    /// starts at 1 and increments only through a nested reference.
    fn expand(
        &self,
        section: &str,
        option: &str,
        out: &mut String,
        rest: &str,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_INTERPOLATION_DEPTH {
            return Err(format!(
                "[{section}] {option}: more than {MAX_INTERPOLATION_DEPTH} nested %(key)s references"
            ));
        }
        let mut rest = rest;
        while !rest.is_empty() {
            let Some(percent) = rest.find('%') else {
                out.push_str(rest);
                return Ok(());
            };
            out.push_str(&rest[..percent]);
            rest = &rest[percent..];
            let mut characters = rest.char_indices();
            characters.next();
            match characters.next() {
                Some((_, '%')) => {
                    out.push('%');
                    rest = &rest[2..];
                }
                Some((_, '(')) => {
                    // `_KEYCRE = %\(([^)]+)\)s`
                    let Some(close) = rest[2..].find(')') else {
                        return Err(interpolation_syntax(section, option, rest));
                    };
                    if close == 0 || !rest[2 + close + 1..].starts_with('s') {
                        return Err(interpolation_syntax(section, option, rest));
                    }
                    let name = rest[2..2 + close].to_lowercase();
                    rest = &rest[2 + close + 2..];
                    let Some(value) = self.option(section, &name) else {
                        return Err(format!(
                            "[{section}] {option}: the interpolated option {name:?} is defined in neither this section nor [DEFAULT]"
                        ));
                    };
                    if value.contains('%') {
                        self.expand(section, option, out, value, depth + 1)?;
                    } else {
                        out.push_str(value);
                    }
                }
                _ => return Err(interpolation_syntax(section, option, rest)),
            }
        }
        Ok(())
    }
}

fn interpolation_syntax(section: &str, option: &str, rest: &str) -> String {
    format!("[{section}] {option}: `%` must be followed by `%` or `(name)s`, found {rest:?}")
}

/// Parse-time state. Values stay as line lists until the whole file parsed,
/// because continuations append to the option started earlier.
#[derive(Default)]
struct Builder {
    defaults: BTreeMap<String, Vec<String>>,
    sections: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    seen_sections: HashSet<String>,
    seen_options: HashSet<(String, String)>,
    section: Option<String>,
    option: Option<String>,
    /// Indentation of the last header or option line; a deeper line continues
    /// the current value.
    indent_level: usize,
}

impl Builder {
    fn line(&mut self, raw: &str, lineno: usize) -> Result<(), String> {
        let trimmed = raw.trim();
        let clean = match trimmed.as_bytes().first() {
            Some(b'#') | Some(b';') => "",
            _ => trimmed,
        };
        if clean.is_empty() {
            // A comment line is dropped without adding an empty value line.
            if clean == trimmed {
                self.blank_line();
            }
            return Ok(());
        }

        let indent = raw.len() - raw.trim_start().len();
        let continuation = match (&self.section, &self.option) {
            (Some(section), Some(option)) if indent > self.indent_level => {
                Some((section.clone(), option.clone()))
            }
            _ => None,
        };
        if let Some((section, option)) = continuation {
            return match self.values_mut(&section, &option) {
                Some(values) => {
                    values.push(clean.to_string());
                    Ok(())
                }
                None => Err(format!(
                    "line {lineno}: the continued option {option:?} is missing from its section"
                )),
            };
        }

        self.indent_level = indent;
        if let Some(header) = section_header(clean) {
            return self.begin_section(header, lineno);
        }
        if self.section.is_none() {
            return Err(format!(
                "line {lineno}: {:?} appears before any [section] header",
                trimmed
            ));
        }
        let (name, value) = split_option(clean).ok_or_else(|| {
            format!("line {lineno}: {trimmed:?} is neither a [section] header nor an `option = value` line")
        })?;
        self.begin_option(name, value, lineno)
    }

    fn values_mut(&mut self, section: &str, option: &str) -> Option<&mut Vec<String>> {
        if section == DEFAULT_SECTION {
            self.defaults.get_mut(option)
        } else {
            self.sections.get_mut(section)?.get_mut(option)
        }
    }

    /// A blank line is appended to the value being read, so the newline it
    /// leaves survives the join; a blank line before any value adds nothing.
    fn blank_line(&mut self) {
        let (Some(section), Some(option)) = (self.section.clone(), self.option.clone()) else {
            return;
        };
        if let Some(values) = self.values_mut(&section, &option) {
            values.push(String::new());
        }
    }

    fn begin_section(&mut self, header: &str, lineno: usize) -> Result<(), String> {
        if header != DEFAULT_SECTION {
            if !self.seen_sections.insert(header.to_string()) {
                return Err(format!(
                    "line {lineno}: section [{header}] appears more than once"
                ));
            }
            self.sections.entry(header.to_string()).or_default();
        }
        self.section = Some(header.to_string());
        self.option = None;
        Ok(())
    }

    fn begin_option(&mut self, name: &str, value: &str, lineno: usize) -> Result<(), String> {
        let section = self
            .section
            .clone()
            .ok_or_else(|| format!("line {lineno}: option {name:?} has no section"))?;
        let name = name.to_lowercase();
        if !self.seen_options.insert((section.clone(), name.clone())) {
            return Err(format!(
                "line {lineno}: option {name:?} appears more than once in [{section}]"
            ));
        }
        let values = if section == DEFAULT_SECTION {
            self.defaults.entry(name.clone()).or_default()
        } else {
            self.sections
                .entry(section)
                .or_default()
                .entry(name.clone())
                .or_default()
        };
        values.push(value.to_string());
        self.option = Some(name);
        Ok(())
    }

    fn finish(self) -> Ini {
        Ini {
            defaults: self
                .defaults
                .into_iter()
                .map(|(option, values)| (option, join_values(values)))
                .collect(),
            sections: self
                .sections
                .into_iter()
                .map(|(section, options)| {
                    let options = options
                        .into_iter()
                        .map(|(option, values)| (option, join_values(values)))
                        .collect();
                    (section, options)
                })
                .collect(),
        }
    }
}

/// `_join_multiline_values`: join with `\n` and trim the end only.
fn join_values(values: Vec<String>) -> String {
    values.join("\n").trim_end().to_string()
}

/// Python's `SECTCRE` (`\[(?P<header>.+)\]`) applied with `match`, which is not
/// end-anchored: after a `]` closes a non-empty header, trailing text is ignored.
fn section_header(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    let close = rest.rfind(']')?;
    if close == 0 {
        return None;
    }
    Some(&rest[..close])
}

/// Python's `OPTCRE` with the default `=`/`:` delimiters: an option name cannot
/// contain a delimiter, so the first one on the line separates name and value,
/// and surrounding whitespace is not part of either.
fn split_option(line: &str) -> Option<(&str, &str)> {
    let delimiter = line.find(['=', ':'])?;
    let name = line[..delimiter].trim_end();
    if name.is_empty() {
        return None;
    }
    Some((name, line[delimiter + 1..].trim()))
}

/// Python's universal-newline iteration: `\n`, `\r\n` and a bare `\r` each end
/// one line, and a trailing terminator does not produce a final empty line.
fn text_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        match rest.find(['\r', '\n']) {
            None => {
                let line = rest;
                rest = "";
                Some(line)
            }
            Some(end) => {
                let line = &rest[..end];
                let bytes = rest.as_bytes();
                let mut skip = end + 1;
                if bytes[end] == b'\r' && bytes.get(skip) == Some(&b'\n') {
                    skip += 1;
                }
                rest = &rest[skip..];
                Some(line)
            }
        }
    })
}
