//! Source editing helpers for Interval.
//!
//! Span-based text surgery using AST spans from v0.8. All functions take
//! the source string and return a modified source string. The result is
//! validated by re-parsing to ensure well-formedness.
//!
//! WASM-safe: no I/O dependencies.

use crate::ast::{Block, PatternBody};
use crate::parser::parse_only;

/// Error type for source editing operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EditError {
    /// Target block not found by name.
    #[error("block '{name}' not found")]
    BlockNotFound { name: String },

    /// Step index out of range.
    #[error("step index {index} out of range (pattern has {count} steps)")]
    StepIndexOutOfRange { index: usize, count: usize },

    /// Block has no source span (synthetic or missing).
    #[error("block '{name}' has no source span")]
    NoSpan { name: String },

    /// The edit produced invalid source.
    #[error("edit produced invalid source: {message}")]
    InvalidResult { message: String },

    /// Parse error on input source.
    #[error("parse error: {message}")]
    ParseError { message: String },

    /// Header parameter not found.
    #[error("header parameter '{param}' not found in source")]
    HeaderParamNotFound { param: String },
}

/// Insert a step token at the given index in a named pattern.
///
/// Index 0 inserts before the first step; index == step_count appends at the end.
pub fn insert_step(
    source: &str,
    pattern_name: &str,
    index: usize,
    token_text: &str,
) -> Result<String, EditError> {
    let program = parse_only(source).map_err(|e| EditError::ParseError {
        message: e.to_string(),
    })?;

    let pat = find_pattern(&program.blocks, pattern_name)?;
    let steps = match &pat.body {
        PatternBody::Steps(s) => s,
        PatternBody::Expression(_) => {
            return Err(EditError::ParseError {
                message: format!("pattern '{pattern_name}' uses expression body, not step lines"),
            });
        }
    };

    if index > steps.len() {
        return Err(EditError::StepIndexOutOfRange {
            index,
            count: steps.len(),
        });
    }

    let insert_pos = if index < steps.len() {
        // Insert before the step at `index`
        steps[index]
            .span
            .ok_or_else(|| EditError::NoSpan {
                name: pattern_name.to_string(),
            })?
            .start
    } else {
        // Append after the last step. A pattern with zero step lines has
        // nothing to anchor the insertion to — report an error instead of
        // panicking on `last()`.
        let last = steps.last().ok_or(EditError::StepIndexOutOfRange {
            index,
            count: steps.len(),
        })?;
        last.span
            .ok_or_else(|| EditError::NoSpan {
                name: pattern_name.to_string(),
            })?
            .end
    };

    // Determine indentation from existing steps
    let indent = detect_step_indent(source, steps.first().and_then(|s| s.span));

    let new_line = if index < steps.len() {
        format!("{indent}{token_text}\n")
    } else {
        format!("\n{indent}{token_text}")
    };

    let mut result = String::with_capacity(source.len() + new_line.len());
    result.push_str(&source[..insert_pos]);
    result.push_str(&new_line);
    result.push_str(&source[insert_pos..]);

    validate_result(&result)?;
    Ok(result)
}

/// Remove a step at the given index from a named pattern.
pub fn remove_step(source: &str, pattern_name: &str, index: usize) -> Result<String, EditError> {
    let program = parse_only(source).map_err(|e| EditError::ParseError {
        message: e.to_string(),
    })?;

    let pat = find_pattern(&program.blocks, pattern_name)?;
    let steps = match &pat.body {
        PatternBody::Steps(s) => s,
        PatternBody::Expression(_) => {
            return Err(EditError::ParseError {
                message: format!("pattern '{pattern_name}' uses expression body"),
            });
        }
    };

    if index >= steps.len() {
        return Err(EditError::StepIndexOutOfRange {
            index,
            count: steps.len(),
        });
    }

    let span = steps[index].span.ok_or_else(|| EditError::NoSpan {
        name: pattern_name.to_string(),
    })?;

    // Find the full line containing this step's span.start
    let line_start = source[..span.start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_end = source[line_start..]
        .find('\n')
        .map(|p| line_start + p + 1) // include the trailing newline
        .unwrap_or(source.len());
    // Remove from line_start to line_end (removes the whole line including its newline,
    // but preserves the preceding newline that ends the previous line).
    let remove_start = line_start;
    let remove_end = line_end;

    let mut result = String::with_capacity(source.len());
    result.push_str(&source[..remove_start]);
    result.push_str(&source[remove_end..]);

    validate_result(&result)?;
    Ok(result)
}

/// Replace a step at the given index in a named pattern with new text.
pub fn replace_step(
    source: &str,
    pattern_name: &str,
    index: usize,
    new_text: &str,
) -> Result<String, EditError> {
    let program = parse_only(source).map_err(|e| EditError::ParseError {
        message: e.to_string(),
    })?;

    let pat = find_pattern(&program.blocks, pattern_name)?;
    let steps = match &pat.body {
        PatternBody::Steps(s) => s,
        PatternBody::Expression(_) => {
            return Err(EditError::ParseError {
                message: format!("pattern '{pattern_name}' uses expression body"),
            });
        }
    };

    if index >= steps.len() {
        return Err(EditError::StepIndexOutOfRange {
            index,
            count: steps.len(),
        });
    }

    let span = steps[index].span.ok_or_else(|| EditError::NoSpan {
        name: pattern_name.to_string(),
    })?;

    // Find where the actual step tokens start (skip leading whitespace in the span)
    let content_start = source[span.start..span.end]
        .find(|c: char| !c.is_whitespace())
        .map(|offset| span.start + offset)
        .unwrap_or(span.start);

    let mut result = String::with_capacity(source.len());
    result.push_str(&source[..content_start]);
    result.push_str(new_text);
    result.push_str(&source[span.end..]);

    validate_result(&result)?;
    Ok(result)
}

/// Set or add an annotation on a step in a named pattern.
///
/// `annotation` should be the full annotation text (e.g., "vel:110", "gate:0.8").
pub fn set_annotation(
    source: &str,
    pattern_name: &str,
    step_index: usize,
    annotation: &str,
) -> Result<String, EditError> {
    let program = parse_only(source).map_err(|e| EditError::ParseError {
        message: e.to_string(),
    })?;

    let pat = find_pattern(&program.blocks, pattern_name)?;
    let steps = match &pat.body {
        PatternBody::Steps(s) => s,
        PatternBody::Expression(_) => {
            return Err(EditError::ParseError {
                message: format!("pattern '{pattern_name}' uses expression body"),
            });
        }
    };

    if step_index >= steps.len() {
        return Err(EditError::StepIndexOutOfRange {
            index: step_index,
            count: steps.len(),
        });
    }

    let span = steps[step_index].span.ok_or_else(|| EditError::NoSpan {
        name: pattern_name.to_string(),
    })?;

    // Insert annotation bracket at the end of the step line (before newline)
    let line_end = source[span.start..]
        .find('\n')
        .map(|p| span.start + p)
        .unwrap_or(span.end);
    let annotation_text = format!("[{annotation}]");

    let mut result = String::with_capacity(source.len() + annotation_text.len() + 1);
    result.push_str(&source[..line_end]);
    result.push_str(&annotation_text);
    result.push_str(&source[line_end..]);

    validate_result(&result)?;
    Ok(result)
}

/// Add a transform to a track's play expression.
///
/// `transform_text` is the transform call (e.g., "reverse", "transpose(2)").
pub fn add_transform(
    source: &str,
    track_name: &str,
    transform_text: &str,
) -> Result<String, EditError> {
    let program = parse_only(source).map_err(|e| EditError::ParseError {
        message: e.to_string(),
    })?;

    let track = find_track(&program.blocks, track_name)?;
    let span = track.span.ok_or_else(|| EditError::NoSpan {
        name: track_name.to_string(),
    })?;

    // Find the play: directive within the track block, skipping comment lines
    let track_source = &source[span.start..span.end];
    let play_abs_start = track_source
        .lines()
        .scan(0usize, |offset, line| {
            let line_offset = *offset;
            *offset += line.len() + 1; // +1 for newline
            Some((line_offset, line))
        })
        .find(|(_, line)| {
            let trimmed = line.trim_start();
            trimmed.starts_with("play:") && !trimmed.starts_with("//")
        })
        .map(|(offset, line)| {
            let trimmed_offset = line.len() - line.trim_start().len();
            span.start + offset + trimmed_offset
        })
        .ok_or_else(|| EditError::ParseError {
            message: format!("track '{track_name}' has no play: directive"),
        })?;
    let play_line_end = source[play_abs_start..]
        .find('\n')
        .map(|p| play_abs_start + p)
        .unwrap_or(span.end);

    let transform_suffix = format!(" -> {transform_text}");

    let mut result = String::with_capacity(source.len() + transform_suffix.len());
    result.push_str(&source[..play_line_end]);
    result.push_str(&transform_suffix);
    result.push_str(&source[play_line_end..]);

    validate_result(&result)?;
    Ok(result)
}

/// Set a parameter on a track block.
///
/// If the parameter already exists, its value is replaced. If not, it is appended
/// to the track declaration line.
pub fn set_track_param(
    source: &str,
    track_name: &str,
    param: &str,
    value: &str,
) -> Result<String, EditError> {
    let program = parse_only(source).map_err(|e| EditError::ParseError {
        message: e.to_string(),
    })?;

    let track = find_track(&program.blocks, track_name)?;
    let span = track.span.ok_or_else(|| EditError::NoSpan {
        name: track_name.to_string(),
    })?;

    // Find the @track declaration line
    let track_line_end = source[span.start..]
        .find('\n')
        .map(|p| span.start + p)
        .unwrap_or(span.end);
    let decl_line = &source[span.start..track_line_end];

    // Check if param already exists
    let param_pattern = format!("{param}=");
    if let Some(param_pos) = decl_line.find(&param_pattern) {
        let abs_param_start = span.start + param_pos;
        let value_start = abs_param_start + param_pattern.len();
        // Find end of value (next whitespace or end of line)
        let value_end = source[value_start..]
            .find(|c: char| c.is_whitespace())
            .map(|p| value_start + p)
            .unwrap_or(track_line_end);

        let mut result = String::with_capacity(source.len());
        result.push_str(&source[..value_start]);
        result.push_str(value);
        result.push_str(&source[value_end..]);

        validate_result(&result)?;
        Ok(result)
    } else {
        // Append param before newline
        let new_param = format!(" {param}={value}");
        let mut result = String::with_capacity(source.len() + new_param.len());
        result.push_str(&source[..track_line_end]);
        result.push_str(&new_param);
        result.push_str(&source[track_line_end..]);

        validate_result(&result)?;
        Ok(result)
    }
}

/// Set a global header parameter value.
///
/// Replaces the value of an existing header directive (e.g., "@bpm 120" → "@bpm 140").
pub fn set_header_param(source: &str, param: &str, value: &str) -> Result<String, EditError> {
    let program = parse_only(source).map_err(|e| EditError::ParseError {
        message: e.to_string(),
    })?;

    // Scope the search to the header span to avoid matching directives in comments
    // or block bodies. Fall back to full source if header has no span.
    let search_end = program.header.span.map(|s| s.end).unwrap_or(source.len());
    let search_region = &source[..search_end];

    let directive = format!("@{param}");
    let pos = search_region
        .find(&directive)
        .ok_or_else(|| EditError::HeaderParamNotFound {
            param: param.to_string(),
        })?;

    let after_directive = pos + directive.len();
    // Skip whitespace after directive
    let value_start = source[after_directive..]
        .find(|c: char| !c.is_whitespace() || c == '\n')
        .map(|p| after_directive + p)
        .unwrap_or(after_directive);

    // If we hit a newline, the directive has no value (shouldn't happen for headers)
    if source[value_start..].starts_with('\n') || value_start >= source.len() {
        // Append value
        let new_text = format!(" {value}");
        let mut result = String::with_capacity(source.len() + new_text.len());
        result.push_str(&source[..after_directive]);
        result.push_str(&new_text);
        result.push_str(&source[after_directive..]);

        validate_result(&result)?;
        return Ok(result);
    }

    // Find end of current value (next newline or end of file)
    let value_end = source[value_start..]
        .find('\n')
        .map(|p| value_start + p)
        .unwrap_or(source.len());

    let mut result = String::with_capacity(source.len());
    result.push_str(&source[..value_start]);
    result.push_str(value);
    result.push_str(&source[value_end..]);

    validate_result(&result)?;
    Ok(result)
}

// ── Helpers ─────────────────────────────────────────────────────────

fn find_pattern<'a>(
    blocks: &'a [Block],
    name: &str,
) -> Result<&'a crate::ast::PatternBlock, EditError> {
    for block in blocks {
        if let Block::Pattern(p) = block {
            if p.name == name {
                return Ok(p);
            }
        }
    }
    Err(EditError::BlockNotFound {
        name: name.to_string(),
    })
}

fn find_track<'a>(
    blocks: &'a [Block],
    name: &str,
) -> Result<&'a crate::ast::TrackBlock, EditError> {
    for block in blocks {
        if let Block::Track(t) = block {
            if t.name == name {
                return Ok(t);
            }
        }
    }
    Err(EditError::BlockNotFound {
        name: name.to_string(),
    })
}

fn detect_step_indent(source: &str, first_step_span: Option<crate::error::Span>) -> String {
    if let Some(span) = first_step_span {
        // Find the start of the line containing this step
        let line_start = source[..span.start].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let indent = &source[line_start..span.start];
        // Only take whitespace characters
        indent.chars().take_while(|c| c.is_whitespace()).collect()
    } else {
        "  ".to_string() // default 2-space indent
    }
}

fn validate_result(source: &str) -> Result<(), EditError> {
    parse_only(source).map_err(|e| EditError::InvalidResult {
        message: e.to_string(),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC_SOURCE: &str = "\
@scale root=C mode=major
@pattern p unit=1/4
  ^1
  ^3
  ^5
  ^3
@track melody ch=1
  play: p * 1
";

    #[test]
    fn test_replace_step() {
        let result = replace_step(BASIC_SOURCE, "p", 1, "^2").unwrap();
        assert!(result.contains("^2"));
        assert!(!result.contains("  ^3\n  ^5")); // first ^3 replaced
    }

    #[test]
    fn test_remove_step() {
        let result = remove_step(BASIC_SOURCE, "p", 0).unwrap();
        // Should still parse, and have one fewer step
        let program = parse_only(&result).unwrap();
        let pat = program
            .blocks
            .iter()
            .find_map(|b| {
                if let Block::Pattern(p) = b {
                    Some(p)
                } else {
                    None
                }
            })
            .unwrap();
        if let PatternBody::Steps(steps) = &pat.body {
            assert_eq!(steps.len(), 3); // was 4, removed 1
        }
    }

    #[test]
    fn test_insert_step() {
        let result = insert_step(BASIC_SOURCE, "p", 0, "^7").unwrap();
        assert!(result.contains("^7"));
        let program = parse_only(&result).unwrap();
        let pat = program
            .blocks
            .iter()
            .find_map(|b| {
                if let Block::Pattern(p) = b {
                    Some(p)
                } else {
                    None
                }
            })
            .unwrap();
        if let PatternBody::Steps(steps) = &pat.body {
            assert_eq!(steps.len(), 5); // was 4, inserted 1
        }
    }

    #[test]
    fn test_set_header_param() {
        let source = "@bpm 120\n@ts 4/4\n@pattern p unit=1/4\n  ^1\n@track t ch=1\n  play: p\n";
        let result = set_header_param(source, "bpm", "140").unwrap();
        assert!(result.contains("@bpm 140"));
        assert!(!result.contains("@bpm 120"));
    }

    #[test]
    fn test_set_track_param_existing() {
        let source = "@pattern p unit=1/4\n  ^1\n@track melody ch=1 oct=4\n  play: p\n";
        let result = set_track_param(source, "melody", "oct", "5").unwrap();
        assert!(result.contains("oct=5"));
        assert!(!result.contains("oct=4"));
    }

    #[test]
    fn test_set_track_param_new() {
        let source = "@pattern p unit=1/4\n  ^1\n@track melody ch=1\n  play: p\n";
        let result = set_track_param(source, "melody", "vel", "100").unwrap();
        assert!(result.contains("vel=100"));
    }

    #[test]
    fn test_block_not_found() {
        let result = replace_step(BASIC_SOURCE, "nonexistent", 0, "^1");
        assert!(matches!(result, Err(EditError::BlockNotFound { .. })));
    }

    #[test]
    fn test_step_index_out_of_range() {
        let result = replace_step(BASIC_SOURCE, "p", 99, "^1");
        assert!(matches!(result, Err(EditError::StepIndexOutOfRange { .. })));
    }

    #[test]
    fn test_insert_step_empty_pattern_errors() {
        // A pattern with zero step lines must produce an error, not a
        // panic, when inserting at index 0 (the append path had an
        // unchecked `steps.last().unwrap()`).
        let source = "@pattern p unit=1/4\n@track t ch=1\n  play: p\n";
        let result = insert_step(source, "p", 0, "^1");
        assert!(matches!(
            result,
            Err(EditError::StepIndexOutOfRange { index: 0, count: 0 })
        ));
    }
}
