//! NAS Backup's case-insensitive fnmatch rules, applied to native path names.
use std::ffi::OsStr;
use std::path::{Component, Path};

#[derive(Clone, Debug)]
enum Token {
    Star,
    Any,
    Literal(u32),
    Class {
        negated: bool,
        ranges: Vec<(u32, u32)>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Exclusions {
    patterns: Vec<(bool, Vec<Token>)>,
}

impl Exclusions {
    pub fn new(patterns: &[String]) -> Self {
        Self {
            patterns: patterns
                .iter()
                .filter_map(|pattern| {
                    let pattern = pattern.trim();
                    if pattern.is_empty() {
                        return None;
                    }
                    #[cfg(windows)]
                    let pattern = pattern.replace('\\', "/");
                    let units = pattern
                        .to_lowercase()
                        .chars()
                        .map(u32::from)
                        .collect::<Vec<_>>();
                    Some((units.contains(&u32::from('/')), tokenize(&units)))
                })
                .collect(),
        }
    }

    pub fn matches(&self, relative: &Path) -> bool {
        let components = relative
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(native_units(name)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut full = Vec::new();
        for component in &components {
            if !full.is_empty() {
                full.push(u32::from('/'));
            }
            full.extend(component);
        }
        self.patterns.iter().any(|(has_slash, pattern)| {
            if *has_slash {
                matches(pattern, &full) || {
                    let mut rooted = Vec::with_capacity(full.len() + 1);
                    rooted.push(u32::from('/'));
                    rooted.extend(&full);
                    matches(pattern, &rooted)
                }
            } else {
                components.iter().any(|part| matches(pattern, part)) || matches(pattern, &full)
            }
        })
    }

    /// A full-path pattern can match a directory without matching its children.
    /// Only component matches prove the entire subtree is excluded.
    pub fn prunes(&self, relative: &Path) -> bool {
        relative.components().any(|part| match part {
            Component::Normal(name) => {
                let value = native_units(name);
                self.patterns
                    .iter()
                    .any(|(slash, pattern)| !slash && matches(pattern, &value))
            }
            _ => false,
        })
    }
}

fn tokenize(pattern: &[u32]) -> Vec<Token> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < pattern.len() {
        match pattern[index] {
            42 => {
                if !matches!(result.last(), Some(Token::Star)) {
                    result.push(Token::Star);
                }
            }
            63 => result.push(Token::Any),
            91 => {
                let mut start = index + 1;
                let negated = pattern.get(start) == Some(&33);
                if negated {
                    start += 1;
                }
                let mut end = start;
                if pattern.get(end) == Some(&93) {
                    end += 1;
                }
                while end < pattern.len() && pattern[end] != 93 {
                    end += 1;
                }
                if end == pattern.len() {
                    result.push(Token::Literal(91));
                } else {
                    let mut ranges = Vec::new();
                    let mut cursor = start;
                    while cursor < end {
                        if cursor + 2 < end && pattern[cursor + 1] == 45 {
                            // Python fnmatch discards descending ranges.
                            if pattern[cursor] <= pattern[cursor + 2] {
                                ranges.push((pattern[cursor], pattern[cursor + 2]));
                            }
                            cursor += 3;
                        } else {
                            ranges.push((pattern[cursor], pattern[cursor]));
                            cursor += 1;
                        }
                    }
                    result.push(Token::Class { negated, ranges });
                    index = end;
                }
            }
            value => result.push(Token::Literal(value)),
        }
        index += 1;
    }
    result
}

fn matches(pattern: &[Token], value: &[u32]) -> bool {
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for token in pattern {
        let mut next = vec![false; previous.len()];
        if matches!(token, Token::Star) {
            next[0] = previous[0];
        }
        for index in 1..next.len() {
            next[index] = match token {
                Token::Star => previous[index] || next[index - 1],
                Token::Any => previous[index - 1],
                Token::Literal(expected) => previous[index - 1] && value[index - 1] == *expected,
                Token::Class { negated, ranges } => {
                    previous[index - 1]
                        && (ranges
                            .iter()
                            .any(|(start, end)| (*start..=*end).contains(&value[index - 1]))
                            != *negated)
                }
            };
        }
        previous = next;
    }
    previous[value.len()]
}

fn native_units(name: &OsStr) -> Vec<u32> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        return char::decode_utf16(name.encode_wide())
            .flat_map(|value| match value {
                Ok(value) => value.to_lowercase().map(u32::from).collect::<Vec<_>>(),
                Err(value) => vec![0x110000 + u32::from(value.unpaired_surrogate())],
            })
            .collect();
    }
    #[cfg(not(windows))]
    {
        let mut bytes = name.as_encoded_bytes();
        let mut result = Vec::new();
        while !bytes.is_empty() {
            match std::str::from_utf8(bytes) {
                Ok(text) => {
                    result.extend(text.to_lowercase().chars().map(u32::from));
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    if let Ok(text) = std::str::from_utf8(&bytes[..valid]) {
                        result.extend(text.to_lowercase().chars().map(u32::from));
                    }
                    let invalid = error.error_len().unwrap_or(bytes.len() - valid);
                    result.extend(
                        bytes[valid..valid + invalid]
                            .iter()
                            .map(|byte| 0x110000 + u32::from(*byte)),
                    );
                    bytes = &bytes[valid + invalid..];
                }
            }
        }
        result
    }
}
