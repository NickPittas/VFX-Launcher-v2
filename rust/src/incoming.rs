//! S3: incoming source analysis — deterministic grouping of scanned incoming
//! files into single files and image sequences.
//!
//! Pure analysis over [`crate::discovery`] results: nothing here touches the
//! filesystem, database or UI, and every returned item keeps the exact
//! [`FileEntry`] paths and stamps that discovery reported. The output is the
//! typed input for the later Incoming mapping/planning packets
//! (CONTRACTS.md §5); it performs no mapping itself. Callers decide which
//! entries to analyze — a whole [`ScanSnapshot`] or the user's selected
//! subset — by passing them to [`analyze_entries`].
//!
//! Ported from the reference sources (read-only `gitclones/` clones):
//! - `CleanIncomings/python/mapping_utils/group_image_sequences.py`: the
//!   sequence-capable extension gate (`.exr .dpx .tif .tiff .jpg .jpeg .png
//!   .hdr .mov .mp4`), grouping by directory, base name and extension, the
//!   >1-member promotion rule and single-member demotion to single files.
//! - `CleanIncomings/python/mapping_utils/extract_sequence_info.py`
//!   (filename-only path): the `sequence_` / `_####` guards, the four frame
//!   patterns tried in order with their exact digit-length windows —
//!   `name.frame.ext` (1–10 digits), `name_frame.ext` (4–10),
//!   `name.frame_middle.ext` (4–10), `name_frame_middle.ext` (4–10) — the
//!   composed base-name guards (empty, `_`, contains `sequence_`) and the
//!   0–999999 frame-range cap. A `vNNN` version token never becomes a frame:
//!   the digits must directly follow the separator, so version runs stay
//!   inside the base name exactly as the legacy patterns leave them (and a
//!   bare digit run adjacent to the separators *is* a frame, exactly as the
//!   legacy patterns decide — no semantic second-guessing).
//! - `CleanIncomings/python/mapping_utils/finalize_sequences.py`: members
//!   sorted numerically by frame; the `min-max` range is a display value.
//! - `CleanIncomings/python/scanner.py`: the lowercase last-extension node
//!   field and the image/video/project/cache extension categories behind
//!   [`SingleKind`].
//! - `SearchFolderStructures/ai_folder_scanner.py`: the `prefix####ext`
//!   display convention — a display value only, never a member list.
//!
//! Deliberate deviations, each contract-mandated (CONTRACTS.md §5):
//! - group identity includes padding (legacy keyed on directory, base name
//!   and extension only), so `shot.0001.exr` and `shot.1.exr` never merge;
//! - grouping always requires the same directory (the legacy
//!   `process_file_for_sequence` variant keyed on base name and extension
//!   only and merged across directories);
//! - a frame number present on more than one member is reported as a
//!   duplicate instead of being silently collapsed;
//! - gaps between present frames are reported per sequence; missing frames
//!   are never fabricated and members are never replaced by a range;
//! - filenames that are not valid UTF-8 stay untouched as single files with
//!   their native path and are listed as unsupported names instead of
//!   passing replacement characters into any derived string;
//! - frame digit runs accept ASCII digits only (the legacy Python `\d` also
//!   accepted other Unicode decimal digits); such names simply fall back to
//!   single files;
//! - the legacy 0..=999999 range cap is kept exactly: a digit run that
//!   parses above the cap abandons only its own pattern, like the legacy
//!   loop, not the remaining separator positions.
//!
//! Stability: results are ordered by total byte orders (sequences by
//! directory, prefix, suffix, padding; members by frame then native path
//! bytes; singles by native path bytes), so the analysis of the same input
//! is identical regardless of scan iteration order.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::discovery::{FileEntry, ScanSnapshot};

/// One analyzed incoming source item: a single file or an image sequence,
/// as CONTRACTS.md §5 defines `IncomingItem`.
#[derive(Clone, Debug)]
pub enum IncomingItem {
    Single(SingleFile),
    Sequence(IncomingSequence),
}

/// A scanned file that is not part of a sequence. Ambiguous single-frame
/// grouping stays an individual file, exactly like the legacy single-member
/// groups, so a later refinement can still promote it.
#[derive(Clone, Debug)]
pub struct SingleFile {
    pub file: FileEntry,
    pub kind: SingleKind,
    /// The filename is not valid UTF-8, so the frame patterns could not be
    /// applied losslessly; the native path is untouched and the file is also
    /// listed in [`IncomingAnalysis::unsupported_names`].
    pub unsupported_name: bool,
}

/// Media category of a single file, from the extension lists of the legacy
/// scanner (`CleanIncomings/python/scanner.py` `supported_extensions`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SingleKind {
    Image,
    Video,
    Project,
    Cache,
    Other,
}

/// One grouped image sequence. `prefix` is the legacy base name (the frame
/// digits and their separator excluded; for the middle-part patterns the
/// legacy composition `base_middle`), `suffix` is the lowercased extension
/// including the leading dot, and `padding` is the digit count of the
/// scanned frame numbers. All three plus the directory are group identity:
/// sequences that differ in any of them never merge.
#[derive(Clone, Debug)]
pub struct IncomingSequence {
    pub directory: PathBuf,
    pub prefix: String,
    pub suffix: String,
    pub padding: usize,
    /// Which legacy frame pattern the members matched; `None` when members
    /// matched different patterns but share group identity.
    pub pattern: Option<SequencePattern>,
    /// Every actual member, sorted numerically by frame (ties by native path
    /// bytes). Gaps are retained; missing frames are never inserted.
    pub members: Vec<SequenceMember>,
    /// Missing-frame runs between present members, ascending by frame.
    pub gaps: Vec<FrameGap>,
    /// Frame numbers carried by more than one member, ascending; both
    /// members are retained losslessly.
    pub duplicate_frames: Vec<DuplicateFrame>,
}

/// One actual frame file of a sequence, CONTRACTS.md §5 `SequenceMember`.
#[derive(Clone, Debug)]
pub struct SequenceMember {
    pub frame: i64,
    pub file: FileEntry,
}

/// Which of the legacy `extract_sequence_info` filename patterns formed the
/// group or member (patterns are display/evidence only, not identity).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequencePattern {
    /// Legacy pattern 1: `name.frame.ext`, 1–10 digits.
    DotFrame,
    /// Legacy pattern 2: `name_frame.ext`, 4–10 digits.
    UnderscoreFrame,
    /// Legacy pattern 3: `name.frame_middle.ext`; the recorded prefix is the
    /// legacy composition `name_middle`.
    DotFrameWithMiddle,
    /// Legacy pattern 4: `name_frame_middle.ext`; prefix composed the same way.
    UnderscoreFrameWithMiddle,
}

/// A missing-frame run between two present members (both endpoints exist).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameGap {
    pub after_frame: i64,
    pub before_frame: i64,
    pub missing_frames: u64,
}

/// One frame number found on more than one member of the same group.
#[derive(Clone, Debug)]
pub struct DuplicateFrame {
    pub frame: i64,
    pub files: Vec<PathBuf>,
}

/// Full analysis of one set of scanned entries.
#[derive(Clone, Debug)]
pub struct IncomingAnalysis {
    /// Sequences first (sorted), then single files (sorted); see the module
    /// stability note for the exact orders.
    pub items: Vec<IncomingItem>,
    pub counts: IncomingCounts,
    /// Native paths of files whose names are not valid UTF-8 and therefore
    /// could not be pattern-analyzed. Each of them also appears as a single
    /// item with [`SingleFile::unsupported_name`] set; nothing else can
    /// appear here, so this list never carries a lossy spelling.
    pub unsupported_names: Vec<PathBuf>,
}

/// Counts that reconcile by construction: every analyzed file lands in
/// exactly one bucket, so `input_files == sequence_files + single_files`,
/// and one sequence member file is one actual frame, so
/// `sequence_files == frames` (never a padded or inferred range count).
/// `input_files` is the caller's selected source items; `unsupported_names`
/// is a subset of `single_files`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IncomingCounts {
    pub input_files: usize,
    pub sequences: usize,
    pub sequence_files: usize,
    pub frames: usize,
    pub single_files: usize,
    pub unsupported_names: usize,
}

/// Analyze scanned entries (a whole snapshot's or the user's selected
/// subset). Deterministic and total: no I/O, no failure cases.
pub fn analyze_snapshot(snapshot: &ScanSnapshot) -> IncomingAnalysis {
    analyze_entries(&snapshot.entries)
}

/// Analyze an explicit entry list; the caller picks the source items.
pub fn analyze_entries(entries: &[FileEntry]) -> IncomingAnalysis {
    let mut groups: HashMap<GroupKey, Vec<GroupMember>> = HashMap::new();
    let mut singles: Vec<SingleFile> = Vec::new();
    let mut unsupported: Vec<PathBuf> = Vec::new();

    for (index, entry) in entries.iter().enumerate() {
        let name = match entry.path.file_name() {
            Some(name) => name,
            None => {
                unsupported.push(entry.path.clone());
                singles.push(SingleFile {
                    file: entry.clone(),
                    kind: SingleKind::Other,
                    unsupported_name: true,
                });
                continue;
            }
        };
        let extension = last_extension(name.as_encoded_bytes());
        let kind = single_kind(extension);
        let Some(name_text) = name.to_str() else {
            unsupported.push(entry.path.clone());
            singles.push(SingleFile {
                file: entry.clone(),
                kind,
                unsupported_name: true,
            });
            continue;
        };
        let Some(extension) = extension else {
            singles.push(SingleFile {
                file: entry.clone(),
                kind,
                unsupported_name: false,
            });
            continue;
        };
        if !is_sequence_capable(extension) {
            singles.push(SingleFile {
                file: entry.clone(),
                kind,
                unsupported_name: false,
            });
            continue;
        }
        match extract_frame_info(name_text) {
            Some(info) => {
                let key = GroupKey {
                    directory: entry
                        .path
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_default(),
                    prefix: info.prefix,
                    suffix: info.suffix,
                    padding: info.padding,
                };
                groups.entry(key).or_default().push(GroupMember {
                    frame: info.frame,
                    pattern: info.pattern,
                    index,
                });
            }
            None => singles.push(SingleFile {
                file: entry.clone(),
                kind,
                unsupported_name: false,
            }),
        }
    }

    let mut grouped: Vec<(GroupKey, Vec<GroupMember>)> = groups.into_iter().collect();
    grouped.sort_by(|(left, _), (right, _)| compare_group_keys(&left, &right));

    let mut items: Vec<IncomingItem> = Vec::new();
    let mut sequence_files = 0usize;
    for (key, members) in grouped {
        let mut members = members;
        members.sort_by(|left, right| {
            left.frame.cmp(&right.frame).then_with(|| {
                path_bytes(&entries[left.index].path).cmp(path_bytes(&entries[right.index].path))
            })
        });
        if members.len() <= 1 {
            for member in members {
                // Same group identity means the same suffix, which is the
                // node's normalized extension these members passed the gate on.
                singles.push(SingleFile {
                    file: entries[member.index].clone(),
                    kind: single_kind(Some(key.suffix.as_bytes())),
                    unsupported_name: false,
                });
            }
            continue;
        }
        let pattern = if members
            .iter()
            .all(|member| member.pattern == members[0].pattern)
        {
            Some(members[0].pattern)
        } else {
            None
        };
        let (gaps, duplicates) = split_gaps_and_duplicates(&members, entries);
        sequence_files += members.len();
        items.push(IncomingItem::Sequence(IncomingSequence {
            directory: key.directory,
            prefix: key.prefix,
            suffix: key.suffix,
            padding: key.padding,
            pattern,
            members: members
                .into_iter()
                .map(|member| SequenceMember {
                    frame: member.frame,
                    file: entries[member.index].clone(),
                })
                .collect(),
            gaps,
            duplicate_frames: duplicates,
        }));
    }

    singles.sort_by(|left, right| path_bytes(&left.file.path).cmp(path_bytes(&right.file.path)));
    unsupported.sort_by(|left, right| path_bytes(left).cmp(path_bytes(right)));

    let counts = IncomingCounts {
        input_files: entries.len(),
        sequences: items.len(),
        sequence_files,
        frames: sequence_files,
        single_files: singles.len(),
        unsupported_names: unsupported.len(),
    };
    items.extend(singles.into_iter().map(IncomingItem::Single));
    IncomingAnalysis {
        items,
        counts,
        unsupported_names: unsupported,
    }
}

impl IncomingItem {
    /// Every actual file behind this item: one for a single file, every
    /// member for a sequence. The expanded transfer plan must list exactly
    /// these files (CONTRACTS.md §5).
    pub fn files(&self) -> Vec<&FileEntry> {
        match self {
            Self::Single(single) => vec![&single.file],
            Self::Sequence(sequence) => {
                sequence.members.iter().map(|member| &member.file).collect()
            }
        }
    }

    /// Actual file count behind this item; the total over all items equals
    /// [`IncomingCounts::input_files`].
    pub fn file_count(&self) -> usize {
        match self {
            Self::Single(_) => 1,
            Self::Sequence(sequence) => sequence.members.len(),
        }
    }
}

impl IncomingSequence {
    /// Actual frame count: one member file is one frame. This is a count of
    /// present files, never an inferred range.
    pub fn frame_count(&self) -> usize {
        self.members.len()
    }

    /// True when every present frame is unique and consecutive.
    pub fn is_contiguous(&self) -> bool {
        self.gaps.is_empty() && self.duplicate_frames.is_empty()
    }

    /// Display-only `min-max` range as the legacy finalize step printed it.
    /// Never a member list or a transfer name.
    pub fn frame_range_display(&self) -> String {
        match (self.members.first(), self.members.last()) {
            (Some(first), Some(last)) => format!("{}-{}", first.frame, last.frame),
            // Sequences always have at least two members; unreachable.
            _ => String::new(),
        }
    }

    /// Display-only `prefix####suffix` name in the
    /// `SearchFolderStructures/ai_folder_scanner.py` convention (always four
    /// `#`, regardless of padding, with the frame separator when the pattern
    /// identifies it). Never a member list or a transfer name.
    pub fn display_name(&self) -> String {
        let separator = match self.pattern {
            Some(SequencePattern::DotFrame | SequencePattern::DotFrameWithMiddle) => ".",
            Some(SequencePattern::UnderscoreFrame | SequencePattern::UnderscoreFrameWithMiddle) => {
                "_"
            }
            None => "",
        };
        format!("{}{}####{}", self.prefix, separator, self.suffix)
    }
}

/// Group identity: directory, prefix, suffix and padding must all match
/// (CONTRACTS.md §5); different values never merge.
#[derive(PartialEq, Eq, Hash)]
struct GroupKey {
    directory: PathBuf,
    prefix: String,
    suffix: String,
    padding: usize,
}

struct GroupMember {
    frame: i64,
    pattern: SequencePattern,
    index: usize,
}

fn compare_group_keys(left: &GroupKey, right: &GroupKey) -> Ordering {
    path_bytes(&left.directory)
        .cmp(path_bytes(&right.directory))
        .then_with(|| left.prefix.as_bytes().cmp(right.prefix.as_bytes()))
        .then_with(|| left.suffix.as_bytes().cmp(right.suffix.as_bytes()))
        .then_with(|| left.padding.cmp(&right.padding))
}

/// Split the sorted members into (gaps, duplicates). Runs of equal frames
/// become one duplicate entry; consecutive distinct frames closer than one
/// apart produce nothing, otherwise one gap entry.
fn split_gaps_and_duplicates(
    members: &[GroupMember],
    entries: &[FileEntry],
) -> (Vec<FrameGap>, Vec<DuplicateFrame>) {
    let mut gaps = Vec::new();
    let mut duplicates = Vec::new();
    let mut index = 0;
    while index < members.len() {
        let frame = members[index].frame;
        let mut run_end = index;
        while run_end < members.len() && members[run_end].frame == frame {
            run_end += 1;
        }
        if run_end - index > 1 {
            duplicates.push(DuplicateFrame {
                frame,
                files: members[index..run_end]
                    .iter()
                    .map(|member| entries[member.index].path.clone())
                    .collect(),
            });
        }
        if run_end < members.len() {
            let next = members[run_end].frame;
            let missing = (next - frame - 1) as u64;
            if missing > 0 {
                gaps.push(FrameGap {
                    after_frame: frame,
                    before_frame: next,
                    missing_frames: missing,
                });
            }
        }
        index = run_end;
    }
    (gaps, duplicates)
}

/// The legacy `Path.suffix` shape: the final dot starts the extension only
/// when at least one non-dot byte precedes it; the dot is included.
fn last_extension(name: &[u8]) -> Option<&[u8]> {
    let dot = name.iter().rposition(|byte| *byte == b'.')?;
    if dot == 0 || name[..dot].iter().all(|byte| *byte == b'.') {
        return None;
    }
    Some(&name[dot..])
}

/// Sequence-capable gate from `group_image_sequences.SEQUENCE_EXTENSIONS`.
/// The legacy set also listed extension spellings without the leading dot,
/// which a suffix comparison (always dotted, always lowercased by the
/// scanner) could never match; only the dotted forms are kept.
fn is_sequence_capable(extension: &[u8]) -> bool {
    const SEQUENCE_EXTENSIONS: [&[u8]; 10] = [
        b".exr", b".dpx", b".tif", b".tiff", b".jpg", b".jpeg", b".png", b".hdr", b".mov", b".mp4",
    ];
    let folded = ascii_lowercase(extension);
    SEQUENCE_EXTENSIONS
        .iter()
        .any(|known| *known == folded.as_slice())
}

/// Single-file media category from `scanner.py` `supported_extensions`.
fn single_kind(extension: Option<&[u8]>) -> SingleKind {
    const IMAGE: [&[u8]; 8] = [
        b".exr", b".dpx", b".tiff", b".tif", b".jpg", b".jpeg", b".png", b".hdr",
    ];
    const VIDEO: [&[u8]; 7] = [
        b".mov", b".mp4", b".avi", b".mkv", b".mxf", b".r3d", b".braw",
    ];
    const PROJECT: [&[u8]; 7] = [b".nk", b".hip", b".ma", b".mb", b".blend", b".c4d", b".aep"];
    const CACHE: [&[u8]; 7] = [
        b".abc", b".bgeo", b".vdb", b".ass", b".usd", b".usda", b".usdc",
    ];
    let Some(extension) = extension else {
        return SingleKind::Other;
    };
    let folded = ascii_lowercase(extension);
    if IMAGE.iter().any(|known| *known == folded.as_slice()) {
        SingleKind::Image
    } else if VIDEO.iter().any(|known| *known == folded.as_slice()) {
        SingleKind::Video
    } else if PROJECT.iter().any(|known| *known == folded.as_slice()) {
        SingleKind::Project
    } else if CACHE.iter().any(|known| *known == folded.as_slice()) {
        SingleKind::Cache
    } else {
        SingleKind::Other
    }
}

fn ascii_lowercase(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|byte| byte.to_ascii_lowercase()).collect()
}

/// Byte view of a native path for deterministic ordering (no lossy text).
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

struct FrameInfo {
    prefix: String,
    suffix: String,
    frame: i64,
    padding: usize,
    pattern: SequencePattern,
}

struct Candidate<'name> {
    prefix: &'name str,
    digits: &'name str,
    middle: Option<&'name str>,
    pattern: SequencePattern,
}

/// Port of the filename-only path of `extract_sequence_info`: the top
/// guards, then the four patterns in order. Each pattern yields at most one
/// candidate (the leftmost separator position its regex reaches); a
/// candidate failing the guards or the frame-range cap abandons only its
/// own pattern, like the legacy loop.
fn extract_frame_info(name: &str) -> Option<FrameInfo> {
    if name.contains("sequence_") || name == "_####" || name.ends_with("_####") {
        return None;
    }
    let extension_dot = name.as_bytes().iter().rposition(|byte| *byte == b'.')?;
    let extension = &name[extension_dot + 1..];
    // `(\.[^.]+)$`: at least one non-dot character after the final dot.
    if extension.is_empty() {
        return None;
    }
    let name_part = &name[..extension_dot];
    let candidates = [
        dot_frame(name_part).map(|(prefix, digits)| Candidate {
            prefix,
            digits,
            middle: None,
            pattern: SequencePattern::DotFrame,
        }),
        underscore_frame(name_part).map(|(prefix, digits)| Candidate {
            prefix,
            digits,
            middle: None,
            pattern: SequencePattern::UnderscoreFrame,
        }),
        dot_frame_middle(name_part).map(|(prefix, digits, middle)| Candidate {
            prefix,
            digits,
            middle: Some(middle),
            pattern: SequencePattern::DotFrameWithMiddle,
        }),
        underscore_frame_middle(name_part).map(|(prefix, digits, middle)| Candidate {
            prefix,
            digits,
            middle: Some(middle),
            pattern: SequencePattern::UnderscoreFrameWithMiddle,
        }),
    ];
    for candidate in candidates.into_iter().flatten() {
        // For the middle-part patterns the legacy composition is
        // `base_middle` (underscore, even after a dot pattern).
        let prefix = match candidate.middle {
            Some(middle) => format!("{}_{}", candidate.prefix, middle),
            None => candidate.prefix.to_string(),
        };
        if prefix.is_empty() || prefix == "_" || prefix.contains("sequence_") {
            continue;
        }
        let Ok(frame) = candidate.digits.parse::<u64>() else {
            continue;
        };
        if frame > 999_999 {
            continue;
        }
        return Some(FrameInfo {
            prefix,
            // Gate-passed extensions are pure ASCII, so ASCII folding here is
            // exactly the legacy `lower()` comparison form of the node field.
            suffix: String::from_utf8(ascii_lowercase(extension.as_bytes()))
                .expect("ASCII case folding preserves UTF-8"),
            frame: frame as i64,
            padding: candidate.digits.len(),
            pattern: candidate.pattern,
        });
    }
    None
}

/// Legacy pattern 1: the leftmost dot whose remainder to the extension dot
/// is exactly 1–10 ASCII digits (`base.digits.ext`).
fn dot_frame(name_part: &str) -> Option<(&str, &str)> {
    let newline = name_part.find('\n');
    for (index, character) in name_part.char_indices() {
        if character != '.' || index == 0 {
            continue;
        }
        if newline.is_some_and(|position| index > position) {
            // `.+?` cannot span a newline; no later dot is reachable either.
            return None;
        }
        let remainder = &name_part[index + 1..];
        if is_digit_run(remainder, 1, 10) {
            return Some((&name_part[..index], remainder));
        }
    }
    None
}

/// Legacy pattern 2: the leftmost underscore whose remainder to the
/// extension dot is exactly 4–10 ASCII digits (`base_digits.ext`).
fn underscore_frame(name_part: &str) -> Option<(&str, &str)> {
    let newline = name_part.find('\n');
    for (index, character) in name_part.char_indices() {
        if character != '_' || index == 0 {
            continue;
        }
        if newline.is_some_and(|position| index > position) {
            return None;
        }
        let remainder = &name_part[index + 1..];
        if is_digit_run(remainder, 4, 10) {
            return Some((&name_part[..index], remainder));
        }
    }
    None
}

/// Legacy patterns 3/4 shared shape: `base<sep>digits_middle` with 4–10
/// digits, a non-empty middle (which may not span a newline, like the
/// legacy `.+?`), and `base` non-empty. Returns (base, digits, middle).
fn separator_frame_middle(name_part: &str, separator: u8) -> Option<(&str, &str, &str)> {
    let newline = name_part.find('\n');
    for (index, character) in name_part.char_indices() {
        if character != separator as char || index == 0 {
            continue;
        }
        if newline.is_some_and(|position| index > position) {
            return None;
        }
        let after = &name_part[index + 1..];
        let run_length = after
            .bytes()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if !(4..=10).contains(&run_length) {
            continue;
        }
        if after.as_bytes().get(run_length) != Some(&b'_') {
            continue;
        }
        let middle = &after[run_length + 1..];
        if middle.is_empty() || middle.contains('\n') {
            continue;
        }
        return Some((&name_part[..index], &after[..run_length], middle));
    }
    None
}

fn dot_frame_middle(name_part: &str) -> Option<(&str, &str, &str)> {
    separator_frame_middle(name_part, b'.')
}

fn underscore_frame_middle(name_part: &str) -> Option<(&str, &str, &str)> {
    separator_frame_middle(name_part, b'_')
}

/// The whole remainder is ASCII digits, `min..=max` of them — the exact
/// reachability of `\d{min,max}` followed by the extension dot.
fn is_digit_run(text: &str, min: usize, max: usize) -> bool {
    text.len() >= min && text.len() <= max && text.bytes().all(|byte| byte.is_ascii_digit())
}
