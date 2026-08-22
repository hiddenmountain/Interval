//! Two-pass recursive descent parser for Interval.
//!
//! Pass 1: Extract global header directives (`@ppq`, `@bpm`, `@ts`, `@title`, `@seed`).
//! Pass 2: Parse all blocks (`@harmony`, `@pattern`, `@track`, `@drummap`) with
//! header values available for tick calculations.
//!
//! The parser produces a typed AST (see `ast` module). All validation that can be
//! done at parse time is performed here; semantic validation (e.g., forward reference
//! detection, unit compatibility) happens during compilation.

use crate::ast::{
    Annotation, Bar, BarChord, BpmBlock, BpmEntry, CcValue, ChordSymbol, DrumMapBlock,
    GlobalHeader, HarmonyBlock, Inversion, PatternBlock, PatternBody, ScaleBlock, ScaleEntry,
    Section, StepLine, StepToken, TimingValue, TonalContext, TrackBlock, TrackContent, TsBlock,
    TsEntry, VoicingStrategy,
};
use crate::error::{CompileError, CompileResult, Span};
use crate::harmony::{parse_chord_symbol_with_context, parse_note_name};
use crate::lexer::{SpannedToken, Token};

/// Maximum nesting depth for recursive constructs (subdivision brackets,
/// variant pools, and parenthesized pattern expressions). Prevents stack
/// overflow on adversarial input like a file of 100k `(`.
const MAX_NESTING_DEPTH: usize = 64;

/// Maximum repetition / bar count. Repeat expressions and `@bars` are
/// materialized eagerly during resolution; an unbounded count would allow
/// a tiny source file to exhaust memory.
const MAX_REPEAT_COUNT: i64 = 100_000;

/// Parser state: walks a token stream with lookahead.
pub struct Parser {
    /// The token stream.
    tokens: Vec<SpannedToken>,
    /// Current position in the token stream.
    pos: usize,
    /// Tonal context from `@scale` block, used for Roman numeral resolution.
    tonal_context: TonalContext,
    /// Current nesting depth of recursive constructs (brackets, parens).
    nesting_depth: usize,
    /// Duplicate-detection flags for header directives. Stored on the
    /// parser (not as locals of `parse_header`) because header parsing can
    /// resume after a scalar `@scale` block (see `parse_header_directives`)
    /// and duplicates must be caught across the split.
    saw_ppq: bool,
    saw_bpm: bool,
    saw_ts: bool,
    saw_title: bool,
    saw_seed: bool,
}

impl Parser {
    /// Create a new parser from a token stream.
    pub fn new(tokens: Vec<SpannedToken>) -> Self {
        Self {
            tokens,
            pos: 0,
            tonal_context: TonalContext::default(),
            nesting_depth: 0,
            saw_ppq: false,
            saw_bpm: false,
            saw_ts: false,
            saw_title: false,
            saw_seed: false,
        }
    }

    /// Enter a nested construct, erroring if the depth limit is exceeded.
    ///
    /// On error the counter is left incremented; that is fine because a
    /// parse error aborts the whole parse and the parser is discarded.
    fn enter_nested(&mut self) -> CompileResult<()> {
        self.nesting_depth += 1;
        if self.nesting_depth > MAX_NESTING_DEPTH {
            return Err(CompileError::ParseError {
                message: format!(
                    "nesting too deep: more than {MAX_NESTING_DEPTH} levels of brackets/parentheses"
                ),
                span: self.current_span(),
            });
        }
        Ok(())
    }

    /// Leave a nested construct.
    fn exit_nested(&mut self) {
        self.nesting_depth = self.nesting_depth.saturating_sub(1);
    }

    /// Set the tonal context (called after parsing `@scale`).
    pub fn set_tonal_context(&mut self, tc: TonalContext) {
        self.tonal_context = tc;
    }

    /// Get the current tonal context.
    pub fn tonal_context(&self) -> &TonalContext {
        &self.tonal_context
    }

    /// Peek at the current token without consuming it.
    fn peek(&self) -> Option<&Token> {
        self.skip_comments_peek()
    }

    /// Peek at the current token, skipping comments, without consuming.
    fn skip_comments_peek(&self) -> Option<&Token> {
        let mut pos = self.pos;
        while pos < self.tokens.len() {
            match &self.tokens[pos].token {
                Token::Comment => pos += 1,
                tok => return Some(tok),
            }
        }
        None
    }

    /// Peek at the current spanned token, skipping comments.
    fn peek_spanned(&self) -> Option<&SpannedToken> {
        let mut pos = self.pos;
        while pos < self.tokens.len() {
            match &self.tokens[pos].token {
                Token::Comment => pos += 1,
                _ => return Some(&self.tokens[pos]),
            }
        }
        None
    }

    /// Advance past any comments.
    fn skip_comments(&mut self) {
        while self.pos < self.tokens.len() {
            if matches!(self.tokens[self.pos].token, Token::Comment) {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Consume the current token and advance.
    fn advance(&mut self) -> Option<&SpannedToken> {
        self.skip_comments();
        if self.pos < self.tokens.len() {
            let tok = &self.tokens[self.pos];
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    /// Consume the current token if it matches the expected type.
    fn expect(&mut self, expected: &Token) -> CompileResult<&SpannedToken> {
        self.skip_comments();
        if self.pos >= self.tokens.len() {
            return Err(CompileError::ParseError {
                message: format!("unexpected end of input, expected {expected:?}"),
                span: self.eof_span(),
            });
        }
        if &self.tokens[self.pos].token == expected {
            let tok = &self.tokens[self.pos];
            self.pos += 1;
            Ok(tok)
        } else {
            let actual = &self.tokens[self.pos];
            Err(CompileError::ParseError {
                message: format!("expected {expected:?}, found {:?}", actual.token),
                span: Span::new(actual.start, actual.end),
            })
        }
    }

    /// Skip newlines (and comments).
    fn skip_newlines(&mut self) {
        loop {
            self.skip_comments();
            if self.pos < self.tokens.len() && matches!(self.tokens[self.pos].token, Token::Newline)
            {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Get a span for the end of input.
    fn eof_span(&self) -> Span {
        if let Some(last) = self.tokens.last() {
            Span::new(last.end, last.end)
        } else {
            Span::new(0, 0)
        }
    }

    /// Get the span of the current token, or EOF span if at end.
    pub fn current_span(&self) -> Span {
        if let Some(tok) = self.peek_spanned() {
            Span::new(tok.start, tok.end)
        } else {
            self.eof_span()
        }
    }

    /// Get the span of the most recently consumed token.
    ///
    /// Returns `Span(0, 0)` if no tokens have been consumed yet.
    fn prev_span(&self) -> Span {
        // Walk backwards from self.pos - 1, skipping comments
        let mut pos = self.pos;
        while pos > 0 {
            pos -= 1;
            if !matches!(self.tokens[pos].token, Token::Comment) {
                return Span::new(self.tokens[pos].start, self.tokens[pos].end);
            }
        }
        Span::new(0, 0)
    }

    /// Parse the global header (pass 1).
    ///
    /// Scans the token stream for header directives (`@ppq`, `@bpm`, `@ts`,
    /// `@title`, `@seed`, `@scale`). Stops at the first non-header block
    /// (`@harmony`, `@pattern`, `@track`, `@drummap`). Applies defaults for
    /// missing directives.
    ///
    /// `@bpm` and `@ts` now accept an optional timeline form:
    /// - scalar:  `@bpm 120`
    /// - inline:  `@bpm 120 * 8 | 140 * 4 ramp=ease_in | 120`
    /// - block:   `@bpm` alone on a line, then entries on subsequent lines
    ///
    /// `@scale` now also accepts a timeline form:
    /// - scalar:  `@scale root=C mode=major`  (unchanged)
    /// - inline:  `@scale root=C mode=major * 16 | root=A mode=minor`
    /// - block:   `@scale` alone on a line, then entries on subsequent lines
    ///
    /// `@tempo` is deprecated in v0.5 and returns `CompileError::DeprecatedTempo`.
    pub fn parse_header(&mut self) -> CompileResult<GlobalHeader> {
        let mut header = GlobalHeader::default();
        let header_start = self.peek_spanned().map(|t| t.start).unwrap_or(0);

        self.skip_newlines();
        self.parse_header_directives(&mut header)?;

        let header_end = self.current_span().end;
        header.span = Some(Span::new(header_start, header_end));
        Ok(header)
    }

    /// Parse header directives into `header` until a non-header token is
    /// reached.
    ///
    /// This is the body of pass 1, split out so that header parsing can
    /// RESUME after a scalar `@scale` block: `@scale root=C mode=major`
    /// followed by `@bpm 140` is valid — header directives and `@scale`
    /// may appear in any order in the header region. `parse_only` calls
    /// this again after each scalar `@scale` block it parses. Duplicate
    /// detection is tracked on the parser (`saw_*` fields) so duplicates
    /// are caught across the resumption.
    pub fn parse_header_directives(&mut self, header: &mut GlobalHeader) -> CompileResult<()> {
        loop {
            self.skip_newlines();
            match self.peek() {
                Some(Token::AtPpq) => {
                    let span_start = self.peek_spanned().map(|t| t.start).unwrap_or(0);
                    self.advance(); // consume @ppq
                    if self.saw_ppq {
                        return Err(CompileError::ParseError {
                            message: "duplicate @ppq directive".to_string(),
                            span: Span::new(span_start, self.current_span().end),
                        });
                    }
                    header.ppq = self.expect_positive_u32("@ppq")?;
                    self.saw_ppq = true;
                }
                Some(Token::AtBpm) => {
                    let span_start = self.peek_spanned().map(|t| t.start).unwrap_or(0);
                    self.advance(); // consume @bpm
                    if self.saw_bpm {
                        return Err(CompileError::ParseError {
                            message: "duplicate @bpm directive".to_string(),
                            span: Span::new(span_start, self.current_span().end),
                        });
                    }
                    // Detect form: if next token is a number, we may have scalar or inline.
                    // If next token is Newline/None, we have block form.
                    self.skip_comments();
                    match self.peek() {
                        Some(Token::Newline) | None => {
                            // Block form: entries follow on subsequent lines.
                            let bpm_block = self.parse_bpm_block_entries()?;
                            if let Some(first) = bpm_block.entries.first() {
                                header.bpm = first.bpm;
                            }
                            header.bpm_block = Some(bpm_block);
                        }
                        _ => {
                            // Scalar or inline form: parse first BPM value.
                            let first_bpm = self.expect_positive_number("@bpm")?;
                            self.skip_comments();
                            if matches!(self.peek(), Some(Token::Star) | Some(Token::Pipe)) {
                                // Inline timeline: parse remaining entries.
                                let bpm_block = self.parse_bpm_inline_entries(first_bpm)?;
                                header.bpm = first_bpm;
                                header.bpm_block = Some(bpm_block);
                            } else {
                                // Scalar form.
                                header.bpm = first_bpm;
                            }
                        }
                    }
                    self.saw_bpm = true;
                }
                Some(Token::AtTs) => {
                    let span_start = self.peek_spanned().map(|t| t.start).unwrap_or(0);
                    self.advance(); // consume @ts
                    if self.saw_ts {
                        return Err(CompileError::ParseError {
                            message: "duplicate @ts directive".to_string(),
                            span: Span::new(span_start, self.current_span().end),
                        });
                    }
                    // Detect form: block form if Newline/None follows @ts.
                    self.skip_comments();
                    match self.peek() {
                        Some(Token::Newline) | None => {
                            // Block form.
                            let ts_block = self.parse_ts_block_entries()?;
                            if let Some(first) = ts_block.entries.first() {
                                header.ts_numerator = first.numerator;
                                header.ts_denominator = first.denominator;
                            }
                            header.ts_block = Some(ts_block);
                        }
                        _ => {
                            // Scalar or inline form: parse first N/M.
                            let (num, denom, span_end) = self.expect_time_signature()?;
                            let (num, denom) =
                                self.validate_ts_pair(num, denom, Span::new(span_start, span_end))?;
                            self.skip_comments();
                            if matches!(self.peek(), Some(Token::Star) | Some(Token::Pipe)) {
                                // Inline timeline.
                                let ts_block = self.parse_ts_inline_entries(num, denom)?;
                                header.ts_numerator = num;
                                header.ts_denominator = denom;
                                header.ts_block = Some(ts_block);
                            } else {
                                // Scalar form.
                                header.ts_numerator = num;
                                header.ts_denominator = denom;
                            }
                        }
                    }
                    self.saw_ts = true;
                }
                Some(Token::AtTitle) => {
                    let span_start = self.peek_spanned().map(|t| t.start).unwrap_or(0);
                    self.advance(); // consume @title
                    if self.saw_title {
                        return Err(CompileError::ParseError {
                            message: "duplicate @title directive".to_string(),
                            span: Span::new(span_start, self.current_span().end),
                        });
                    }
                    self.skip_comments();
                    if self.pos < self.tokens.len() {
                        if let Token::StringLiteral(s) = &self.tokens[self.pos].token {
                            header.title = Some(s.clone());
                            self.pos += 1;
                        } else {
                            return Err(CompileError::ParseError {
                                message: "@title requires a quoted string".to_string(),
                                span: self.current_span(),
                            });
                        }
                    } else {
                        return Err(CompileError::ParseError {
                            message: "@title requires a quoted string".to_string(),
                            span: Span::new(span_start, span_start),
                        });
                    }
                    self.saw_title = true;
                }
                Some(Token::AtSeed) => {
                    let span_start = self.peek_spanned().map(|t| t.start).unwrap_or(0);
                    self.advance(); // consume @seed
                    if self.saw_seed {
                        return Err(CompileError::ParseError {
                            message: "duplicate @seed directive".to_string(),
                            span: Span::new(span_start, self.current_span().end),
                        });
                    }
                    let val = self.expect_non_negative_integer("@seed")?;
                    header.seed = Some(val as u64);
                    self.saw_seed = true;
                }
                Some(Token::AtBars) => {
                    let span_start = self.peek_spanned().map(|t| t.start).unwrap_or(0);
                    self.advance(); // consume @bars
                    if header.bars.is_some() {
                        return Err(CompileError::ParseError {
                            message: "duplicate @bars directive".to_string(),
                            span: Span::new(span_start, self.current_span().end),
                        });
                    }
                    self.skip_comments();
                    // @bars off | @bars N
                    match self.peek() {
                        Some(Token::Ident(s)) if s == "off" => {
                            self.advance();
                            header.bars = Some(crate::ast::BarsSetting::Off);
                        }
                        _ => {
                            let val = self.expect_bounded_count("@bars")?;
                            header.bars = Some(crate::ast::BarsSetting::Count(val));
                        }
                    }
                }
                Some(Token::AtScale) => {
                    // @scale can appear in the header area. If it's a
                    // timeline (inline `* N`/`|` form, or block form with
                    // entries on the following lines), parse it here and
                    // store it in header.scale_block. If scalar, stop the
                    // header loop and let pass 2 parse it as a Block::Scale;
                    // pass 2 then resumes header parsing afterwards so
                    // header directives may follow a scalar @scale.
                    let scale_timeline = self.is_scale_timeline_form();
                    if scale_timeline {
                        self.advance(); // consume @scale
                        let scale_block = self.parse_scale_timeline_entries()?;
                        // Set tonal context from first entry
                        if let Some(first) = scale_block.entries.first() {
                            self.tonal_context = TonalContext {
                                root: first.root,
                                mode: first.mode.clone().unwrap_or_else(|| "major".to_string()),
                                span: None,
                            };
                        }
                        header.scale_block = Some(scale_block);
                    } else {
                        // Scalar form — stop the header loop; let pass 2 handle it.
                        break;
                    }
                }
                // @tempo is deprecated in v0.5
                Some(Token::AtTempo) => {
                    let span = self.current_span();
                    return Err(CompileError::DeprecatedTempo { span });
                }
                // Stop at block declarations or end of input
                Some(Token::AtHarmony)
                | Some(Token::AtPattern)
                | Some(Token::AtTrack)
                | Some(Token::AtDrummap)
                | None => break,
                // Skip any other tokens (allow flexibility)
                _ => {
                    // Unknown token in header position — could be an error,
                    // but for robustness we stop and let pass 2 handle it.
                    break;
                }
            }
        }

        Ok(())
    }

    /// Detect if the next `@scale` declaration is a timeline form.
    ///
    /// Two timeline forms exist (spec §5.2):
    /// - inline: params on the `@scale` line with `* N` bar counts and/or
    ///   `|` entry separators;
    /// - block: bare `@scale` on its own line, with entries (`root=` /
    ///   `mode=` `[* N]`) on the following lines.
    ///
    /// A `@scale` with params but no `*` / `|` on its line is the scalar
    /// form, and a bare `@scale` NOT followed by an entry line stays scalar
    /// (defaults) for backwards compatibility.
    fn is_scale_timeline_form(&self) -> bool {
        let mut pos = self.pos;
        // Skip comments at start
        while pos < self.tokens.len() && matches!(self.tokens[pos].token, Token::Comment) {
            pos += 1;
        }
        // Expect @scale
        if pos >= self.tokens.len() || !matches!(self.tokens[pos].token, Token::AtScale) {
            return false;
        }
        pos += 1;
        // Scan the rest of the @scale line, looking for timeline markers.
        let mut saw_same_line_token = false;
        loop {
            if pos >= self.tokens.len() {
                break;
            }
            match &self.tokens[pos].token {
                Token::Newline => break,
                Token::Comment => {}
                // `* N` or `|` on the @scale line — inline timeline form.
                Token::Star | Token::Pipe => return true,
                Token::AtHarmony
                | Token::AtPattern
                | Token::AtTrack
                | Token::AtDrummap
                | Token::AtBpm
                | Token::AtTs
                | Token::AtPpq
                | Token::AtTempo
                | Token::AtScale => break,
                _ => saw_same_line_token = true,
            }
            pos += 1;
        }
        if saw_same_line_token {
            // Params but no `*` / `|` — scalar form.
            return false;
        }
        // Bare `@scale` on its own line: block form when the next
        // non-blank line starts a timeline entry (`root=` or `mode=`).
        while pos < self.tokens.len()
            && matches!(self.tokens[pos].token, Token::Newline | Token::Comment)
        {
            pos += 1;
        }
        matches!(
            self.tokens.get(pos).map(|t| &t.token),
            Some(Token::KwRoot) | Some(Token::KwMode)
        )
    }

    /// Parse `@bpm` block form entries (the `@bpm` token has already been consumed).
    ///
    /// Entries appear on subsequent lines in the form:
    /// `<bpm> [* <bars>] [ramp=<curve>]`
    /// until the next block boundary.
    fn parse_bpm_block_entries(&mut self) -> CompileResult<BpmBlock> {
        self.skip_newlines();
        let mut entries = Vec::new();

        while !self.at_block_boundary_or_end() {
            self.skip_comments();
            if matches!(self.peek(), Some(Token::Newline)) {
                self.advance();
                continue;
            }
            if self.at_block_boundary_or_end() {
                break;
            }

            let entry = self.parse_bpm_entry()?;
            entries.push(entry);

            // Separator: newline or end
            self.skip_comments();
            if matches!(self.peek(), Some(Token::Newline)) {
                self.advance();
            }
        }

        if entries.is_empty() {
            return Err(CompileError::ParseError {
                message: "@bpm block requires at least one entry".to_string(),
                span: self.current_span(),
            });
        }

        Ok(BpmBlock {
            entries,
            span: None,
        })
    }

    /// Parse `@bpm` inline timeline entries, given the first BPM value already consumed.
    ///
    /// The inline form: `<first_bpm> [* <bars>] [ramp=<curve>] [| <bpm> ...]*`
    /// When this is called, we have `first_bpm` and the next token is `*` or `|`.
    fn parse_bpm_inline_entries(&mut self, first_bpm: f64) -> CompileResult<BpmBlock> {
        let mut entries = Vec::new();

        // Parse the optional `* bars` and `ramp=` for the first entry.
        let first_entry = self.parse_bpm_entry_suffix(first_bpm)?;
        entries.push(first_entry);

        // Parse remaining entries separated by `|`.
        while matches!(self.peek(), Some(Token::Pipe)) {
            self.advance(); // consume |
            self.skip_comments();
            if matches!(self.peek(), Some(Token::Newline) | None) {
                break;
            }
            let bpm = self.expect_positive_number("@bpm entry")?;
            let entry = self.parse_bpm_entry_suffix(bpm)?;
            entries.push(entry);
        }

        Ok(BpmBlock {
            entries,
            span: None,
        })
    }

    /// Parse a single `@bpm` entry: `<bpm> [* <bars>] [ramp=<curve>]`
    fn parse_bpm_entry(&mut self) -> CompileResult<BpmEntry> {
        let bpm = self.expect_positive_number("@bpm entry")?;
        self.parse_bpm_entry_suffix(bpm)
    }

    /// Parse the suffix of a `@bpm` entry after the BPM value: `[* <bars>] [ramp=<curve>]`
    fn parse_bpm_entry_suffix(&mut self, bpm: f64) -> CompileResult<BpmEntry> {
        let mut bars: Option<u32> = None;
        let mut ramp: Option<String> = None;

        self.skip_comments();
        if matches!(self.peek(), Some(Token::Star)) {
            self.advance(); // consume *
            bars = Some(self.expect_bounded_count("@bpm bar count")?);
        }

        self.skip_comments();
        // Check for ramp=<curve>
        if matches!(self.peek(), Some(Token::KwRamp)) {
            self.advance(); // consume ramp
            self.expect(&Token::Equals)?;
            let curve = self.parse_bpm_curve_name()?;
            ramp = Some(curve);
        }

        Ok(BpmEntry {
            bpm,
            bars,
            ramp,
            span: None,
        })
    }

    /// Parse a curve name for `@bpm` ramp= parameter.
    fn parse_bpm_curve_name(&mut self) -> CompileResult<String> {
        self.skip_comments();
        match self.peek().cloned() {
            Some(Token::KwEaseIn) => { self.advance(); Ok("ease_in".to_string()) }
            Some(Token::KwEaseOut) => { self.advance(); Ok("ease_out".to_string()) }
            Some(Token::KwEaseInOut) => { self.advance(); Ok("ease_in_out".to_string()) }
            Some(Token::KwArch) => { self.advance(); Ok("arch".to_string()) }
            Some(Token::KwLinear) => { self.advance(); Ok("linear".to_string()) }
            Some(Token::Ident(s)) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            other => Err(CompileError::ParseError {
                message: format!("expected curve name (ease_in, ease_out, ease_in_out, arch, linear), found {other:?}"),
                span: self.current_span(),
            }),
        }
    }

    /// Parse `@ts` block form entries (the `@ts` token has already been consumed).
    fn parse_ts_block_entries(&mut self) -> CompileResult<TsBlock> {
        self.skip_newlines();
        let mut entries = Vec::new();

        while !self.at_block_boundary_or_end() {
            self.skip_comments();
            if matches!(self.peek(), Some(Token::Newline)) {
                self.advance();
                continue;
            }
            if self.at_block_boundary_or_end() {
                break;
            }

            let entry = self.parse_ts_entry()?;
            entries.push(entry);

            self.skip_comments();
            if matches!(self.peek(), Some(Token::Newline)) {
                self.advance();
            }
        }

        if entries.is_empty() {
            return Err(CompileError::ParseError {
                message: "@ts block requires at least one entry".to_string(),
                span: self.current_span(),
            });
        }

        Ok(TsBlock {
            entries,
            span: None,
        })
    }

    /// Parse `@ts` inline timeline entries, given the first N/M already consumed.
    fn parse_ts_inline_entries(
        &mut self,
        first_num: u8,
        first_denom: u8,
    ) -> CompileResult<TsBlock> {
        let mut entries = Vec::new();

        let first_entry = self.parse_ts_entry_suffix(first_num, first_denom)?;
        entries.push(first_entry);

        while matches!(self.peek(), Some(Token::Pipe)) {
            self.advance(); // consume |
            self.skip_comments();
            if matches!(self.peek(), Some(Token::Newline) | None) {
                break;
            }
            let (num, denom, span_end) = self.expect_time_signature()?;
            let (num, denom) = self.validate_ts_pair(num, denom, Span::new(span_end, span_end))?;
            let entry = self.parse_ts_entry_suffix(num, denom)?;
            entries.push(entry);
        }

        Ok(TsBlock {
            entries,
            span: None,
        })
    }

    /// Parse a single `@ts` entry: `<num>/<denom> [* <bars>]`
    fn parse_ts_entry(&mut self) -> CompileResult<TsEntry> {
        let span_start = self.current_span().start;
        let (num, denom, span_end) = self.expect_time_signature()?;
        let (num, denom) = self.validate_ts_pair(num, denom, Span::new(span_start, span_end))?;
        self.parse_ts_entry_suffix(num, denom)
    }

    /// Parse the suffix of a `@ts` entry: `[* <bars>]`
    fn parse_ts_entry_suffix(&mut self, numerator: u8, denominator: u8) -> CompileResult<TsEntry> {
        let mut bars: Option<u32> = None;

        self.skip_comments();
        if matches!(self.peek(), Some(Token::Star)) {
            self.advance(); // consume *
            bars = Some(self.expect_bounded_count("@ts bar count")?);
        }

        Ok(TsEntry {
            numerator,
            denominator,
            bars,
            span: None,
        })
    }

    /// Parse `@scale` timeline entries (the `@scale` token has already been consumed).
    ///
    /// Each entry: `root=<note> mode=<mode> [* <bars>]`
    /// Entries are separated by `|` (inline) or newlines (block).
    fn parse_scale_timeline_entries(&mut self) -> CompileResult<ScaleBlock> {
        let mut entries = Vec::new();

        // Check if block form (next token on same line is nothing, or Newline)
        self.skip_comments();
        if matches!(self.peek(), Some(Token::Newline) | None) {
            // Block form
            self.skip_newlines();
            while !self.at_block_boundary_or_end() {
                self.skip_comments();
                if matches!(self.peek(), Some(Token::Newline)) {
                    self.advance();
                    continue;
                }
                if self.at_block_boundary_or_end() {
                    break;
                }
                let entry = self.parse_scale_entry()?;
                entries.push(entry);
                self.skip_comments();
                if matches!(self.peek(), Some(Token::Newline)) {
                    self.advance();
                }
            }
        } else {
            // Inline form: params then optional `* bars`, separated by `|`
            let first = self.parse_scale_entry()?;
            entries.push(first);

            while matches!(self.peek(), Some(Token::Pipe)) {
                self.advance(); // consume |
                self.skip_comments();
                if matches!(self.peek(), Some(Token::Newline) | None) {
                    break;
                }
                let entry = self.parse_scale_entry()?;
                entries.push(entry);
            }
        }

        if entries.is_empty() {
            return Err(CompileError::ParseError {
                message: "@scale timeline requires at least one entry".to_string(),
                span: self.current_span(),
            });
        }

        Ok(ScaleBlock {
            entries,
            span: None,
        })
    }

    /// Parse a single `@scale` timeline entry: `[root=<note>] [mode=<mode>] [* <bars>]`
    ///
    /// Both `root=` and `mode=` are optional; `None` means "not specified, inherit from previous
    /// entry" (resolved in `ScaleTimeline::from_scale_block`).
    fn parse_scale_entry(&mut self) -> CompileResult<ScaleEntry> {
        let entry_span = self.current_span();
        let mut root: Option<u8> = None;
        let mut mode: Option<String> = None;

        // Parse root= and mode= params (in any order)
        loop {
            self.skip_comments();
            match self.peek() {
                Some(Token::KwRoot) => {
                    self.advance();
                    self.expect(&Token::Equals)?;
                    let root_name = self.parse_note_root()?;
                    root = Some(
                        crate::harmony::parse_note_name(&root_name)
                            .ok_or_else(|| CompileError::ParseError {
                                message: format!("invalid root note '{root_name}'"),
                                span: self.current_span(),
                            })?
                            .0,
                    );
                }
                Some(Token::KwMode) => {
                    self.advance();
                    self.expect(&Token::Equals)?;
                    let m = self.expect_ident("mode value")?;
                    if crate::harmony::lookup_mode(&m).is_none() {
                        return Err(CompileError::ParseError {
                            message: format!("unknown mode '{m}'"),
                            span: self.current_span(),
                        });
                    }
                    mode = Some(m);
                }
                _ => break,
            }
        }

        // Parse optional `* bars`
        let mut bars: Option<u32> = None;
        self.skip_comments();
        if matches!(self.peek(), Some(Token::Star)) {
            self.advance(); // consume *
            bars = Some(self.expect_bounded_count("@scale bar count")?);
        }

        // The entry must consume at least one token — otherwise the block
        // form's entry loop would spin forever on an unrecognized token.
        if root.is_none() && mode.is_none() && bars.is_none() {
            return Err(CompileError::ParseError {
                message: format!(
                    "expected root= or mode= in @scale timeline entry, found {:?}",
                    self.peek()
                ),
                span: entry_span,
            });
        }

        Ok(ScaleEntry {
            root,
            mode,
            bars,
            span: None,
        })
    }

    /// Expect and consume a positive integer.
    fn expect_positive_integer(&mut self, context: &str) -> CompileResult<i64> {
        self.skip_comments();
        if self.pos >= self.tokens.len() {
            return Err(CompileError::ParseError {
                message: format!("{context} requires a positive integer value"),
                span: self.eof_span(),
            });
        }
        let tok = &self.tokens[self.pos];
        if let Token::Integer(val) = &tok.token {
            let val = *val;
            let span = Span::new(tok.start, tok.end);
            self.pos += 1;
            if val <= 0 {
                return Err(CompileError::ParseError {
                    message: format!("{context} must be a positive integer, got {val}"),
                    span,
                });
            }
            Ok(val)
        } else {
            Err(CompileError::ParseError {
                message: format!("{context} requires a positive integer value"),
                span: Span::new(tok.start, tok.end),
            })
        }
    }

    /// Expect and consume a non-negative integer (>= 0).
    fn expect_non_negative_integer(&mut self, context: &str) -> CompileResult<i64> {
        self.skip_comments();
        if self.pos >= self.tokens.len() {
            return Err(CompileError::ParseError {
                message: format!("{context} requires a non-negative integer value"),
                span: self.eof_span(),
            });
        }
        let tok = &self.tokens[self.pos];
        if let Token::Integer(val) = &tok.token {
            let val = *val;
            let span = Span::new(tok.start, tok.end);
            self.pos += 1;
            if val < 0 {
                return Err(CompileError::ParseError {
                    message: format!("{context} must be non-negative, got {val}"),
                    span,
                });
            }
            Ok(val)
        } else {
            Err(CompileError::ParseError {
                message: format!("{context} requires a non-negative integer value"),
                span: Span::new(tok.start, tok.end),
            })
        }
    }

    /// Expect and consume a positive number (integer or float).
    fn expect_positive_number(&mut self, context: &str) -> CompileResult<f64> {
        self.skip_comments();
        if self.pos >= self.tokens.len() {
            return Err(CompileError::ParseError {
                message: format!("{context} requires a positive number"),
                span: self.eof_span(),
            });
        }
        let tok = &self.tokens[self.pos];
        let (val, span) = match &tok.token {
            Token::Float(f) => (*f, Span::new(tok.start, tok.end)),
            Token::Integer(i) => (*i as f64, Span::new(tok.start, tok.end)),
            _ => {
                return Err(CompileError::ParseError {
                    message: format!("{context} requires a positive number"),
                    span: Span::new(tok.start, tok.end),
                });
            }
        };
        self.pos += 1;
        if val <= 0.0 {
            return Err(CompileError::ParseError {
                message: format!("{context} must be positive, got {val}"),
                span,
            });
        }
        Ok(val)
    }

    /// Expect and consume a non-negative number (integer or float, >= 0).
    fn expect_non_negative_number(&mut self, context: &str) -> CompileResult<f64> {
        self.skip_comments();
        if self.pos >= self.tokens.len() {
            return Err(CompileError::ParseError {
                message: format!("{context} requires a number"),
                span: self.eof_span(),
            });
        }
        let tok = &self.tokens[self.pos];
        let (val, span) = match &tok.token {
            Token::Float(f) => (*f, Span::new(tok.start, tok.end)),
            Token::Integer(i) => (*i as f64, Span::new(tok.start, tok.end)),
            _ => {
                return Err(CompileError::ParseError {
                    message: format!("{context} requires a number"),
                    span: Span::new(tok.start, tok.end),
                });
            }
        };
        self.pos += 1;
        if val < 0.0 {
            return Err(CompileError::ParseError {
                message: format!("{context} must be non-negative, got {val}"),
                span,
            });
        }
        Ok(val)
    }

    /// Expect and consume a positive integer that fits in `u32`.
    ///
    /// Range-checks on the wide (`i64`) type BEFORE narrowing so that
    /// values like `4294967296` produce an error instead of silently
    /// truncating to 0.
    fn expect_positive_u32(&mut self, context: &str) -> CompileResult<u32> {
        let span = self.current_span();
        let val = self.expect_positive_integer(context)?;
        if val > u32::MAX as i64 {
            return Err(CompileError::ParseError {
                message: format!("{context} value {val} is too large (max {})", u32::MAX),
                span,
            });
        }
        Ok(val as u32)
    }

    /// Expect and consume a positive repetition / bar count, capped at
    /// `MAX_REPEAT_COUNT` to prevent out-of-memory from eagerly
    /// materialized repeats.
    fn expect_bounded_count(&mut self, context: &str) -> CompileResult<u32> {
        let span = self.current_span();
        let val = self.expect_positive_integer(context)?;
        if val > MAX_REPEAT_COUNT {
            return Err(CompileError::ParseError {
                message: format!("{context} {val} exceeds maximum of {MAX_REPEAT_COUNT}"),
                span,
            });
        }
        Ok(val as u32)
    }

    /// Expect and consume an integer in the MIDI 7-bit range (0-127).
    ///
    /// Range-checks on the wide type BEFORE narrowing to `u8`.
    fn expect_midi_u7(&mut self, context: &str) -> CompileResult<u8> {
        let span = self.current_span();
        let val = self.expect_integer(context)?;
        if !(0..=127).contains(&val) {
            return Err(CompileError::ParseError {
                message: format!("{context} must be 0-127, got {val}"),
                span,
            });
        }
        Ok(val as u8)
    }

    /// Expect and consume a non-negative octave number (0-10).
    ///
    /// Range-checks on the wide type BEFORE narrowing to `u8`.
    fn expect_octave(&mut self, context: &str) -> CompileResult<u8> {
        let span = self.current_span();
        let val = self.expect_non_negative_integer(context)?;
        if val > 10 {
            return Err(CompileError::ParseError {
                message: format!("{context} must be 0-10, got {val}"),
                span,
            });
        }
        Ok(val as u8)
    }

    /// Expect and consume an integer (possibly negative) that fits in `i32`.
    fn expect_i32(&mut self, context: &str) -> CompileResult<i32> {
        let span = self.current_span();
        let val = self.expect_integer(context)?;
        i32::try_from(val).map_err(|_| CompileError::ParseError {
            message: format!("{context} {val} is out of range"),
            span,
        })
    }

    /// Validate a time signature pair on the wide type, then narrow to `u8`.
    ///
    /// The numerator must fit in `u8` (1-255); the denominator must be a
    /// power of 2 no larger than 128 (so it also fits in `u8`). Checking
    /// AFTER narrowing would let `@ts 4/256` truncate the denominator to 0
    /// and divide by zero downstream.
    fn validate_ts_pair(&self, num: i64, denom: i64, span: Span) -> CompileResult<(u8, u8)> {
        if !(1..=255).contains(&num) {
            return Err(CompileError::ParseError {
                message: format!("@ts numerator must be 1-255, got {num}"),
                span,
            });
        }
        if denom <= 0 || (denom & (denom - 1)) != 0 || denom > 128 {
            return Err(CompileError::ParseError {
                message: format!("@ts denominator must be a power of 2 (1-128), got {denom}"),
                span,
            });
        }
        Ok((num as u8, denom as u8))
    }

    /// Expect and consume a time signature (numerator/denominator).
    fn expect_time_signature(&mut self) -> CompileResult<(i64, i64, usize)> {
        let num = self.expect_positive_integer("@ts numerator")?;
        self.expect(&Token::Slash)?;
        let denom_tok = self.peek_spanned().map(|t| t.end).unwrap_or(0);
        let denom = self.expect_positive_integer("@ts denominator")?;
        Ok((num, denom, denom_tok))
    }

    /// Reset parser position to the beginning for pass 2.
    pub fn reset(&mut self) {
        self.pos = 0;
    }

    /// Get the current parser position.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Public wrapper for skip_newlines (used by test harness and external callers).
    pub fn skip_newlines_pub(&mut self) {
        self.skip_newlines();
    }

    /// Returns true if the parser still has unconsumed tokens.
    pub fn has_tokens(&self) -> bool {
        self.pos < self.tokens.len()
    }

    /// Returns true if the next token starts a `@scale` block.
    pub fn peek_is_scale(&self) -> bool {
        matches!(self.peek(), Some(Token::AtScale))
    }

    /// Returns true if the next token starts a `@harmony` block.
    pub fn peek_is_harmony(&self) -> bool {
        matches!(self.peek(), Some(Token::AtHarmony))
    }

    /// Returns true if the next token starts a `@pattern` block.
    pub fn peek_is_pattern(&self) -> bool {
        matches!(self.peek(), Some(Token::AtPattern))
    }

    /// Returns true if the next token starts a `@track` block.
    pub fn peek_is_track(&self) -> bool {
        matches!(self.peek(), Some(Token::AtTrack))
    }

    /// Returns true if the next token starts a `@drummap` block.
    pub fn peek_is_drummap(&self) -> bool {
        matches!(self.peek(), Some(Token::AtDrummap))
    }

    /// Returns true if the next token starts a `@tempo` block.
    pub fn peek_is_tempo(&self) -> bool {
        matches!(self.peek(), Some(Token::AtTempo))
    }

    /// Create a parse error for an unexpected token at the top level.
    ///
    /// Used by callers (CLI, golden tests) when the block parse loop encounters
    /// a token that doesn't match any known block type.
    pub fn error_unexpected_block(&self) -> CompileError {
        let span = self.current_span();
        let token_desc = match self.peek() {
            Some(tok) => format!("{tok:?}"),
            None => "end of input".to_string(),
        };
        CompileError::ParseError {
            message: format!("unexpected token at top level: {token_desc}"),
            span,
        }
    }

    /// Check if the current token is a block start.
    fn is_block_start(&self) -> bool {
        matches!(
            self.peek(),
            Some(
                Token::AtScale
                    | Token::AtHarmony
                    | Token::AtPattern
                    | Token::AtTrack
                    | Token::AtDrummap
                    | Token::AtTempo
                    | Token::AtBpm
                    | Token::AtTs
                    | Token::AtPpq
            )
        )
    }

    /// Check if at end of input or a block boundary.
    fn at_block_boundary_or_end(&self) -> bool {
        self.peek().is_none() || self.is_block_start()
    }

    // ── Scale Block Parsing ─────────────────────────────────────────

    /// Parse a `@scale` block.
    ///
    /// Expects the parser to be positioned at `@scale`. Accepts `root=` and
    /// `mode=` parameters only.
    pub fn parse_scale_block(&mut self) -> CompileResult<TonalContext> {
        let block_span_start = self.current_span().start;
        self.expect(&Token::AtScale)?;

        let mut root: Option<u8> = None;
        let mut mode = "major".to_string();

        // Parse params until newline
        while !matches!(self.peek(), Some(Token::Newline) | None) {
            match self.peek() {
                Some(Token::KwRoot) => {
                    self.advance();
                    self.expect(&Token::Equals)?;
                    let root_name = self.parse_note_root()?;
                    root = Some(
                        crate::harmony::parse_note_name(&root_name)
                            .ok_or_else(|| CompileError::ParseError {
                                message: format!("invalid root note '{root_name}'"),
                                span: self.current_span(),
                            })?
                            .0,
                    );
                }
                Some(Token::KwMode) => {
                    self.advance();
                    self.expect(&Token::Equals)?;
                    mode = self.expect_ident("mode value")?;
                    // Validate mode exists
                    if crate::harmony::lookup_mode(&mode).is_none() {
                        return Err(CompileError::ParseError {
                            message: format!("unknown mode '{mode}'"),
                            span: self.current_span(),
                        });
                    }
                }
                Some(Token::Comment) => {
                    self.advance();
                }
                _ => {
                    return Err(CompileError::ParseError {
                        message: format!(
                            "unexpected token in @scale declaration: {:?} — @scale accepts root= and mode= only",
                            self.peek()
                        ),
                        span: self.current_span(),
                    });
                }
            }
        }

        self.skip_newlines();

        let block_span_end = self.prev_span().end;
        let tc = TonalContext {
            root,
            mode,
            span: Some(Span::new(block_span_start, block_span_end)),
        };
        self.tonal_context = tc.clone();
        Ok(tc)
    }

    // ── Harmony Block Parsing ────────────────────────────────────────

    /// Parse a `@harmony` block.
    ///
    /// Expects the parser to be positioned at `@harmony`. Parses the declaration
    /// line, then bar grid lines, `steps:` blocks, and `section:` directives
    /// until the next block or end of input.
    pub fn parse_harmony_block(&mut self) -> CompileResult<HarmonyBlock> {
        let span_start = self.current_span().start;
        self.expect(&Token::AtHarmony)?;

        // Parse optional name (v0.5: name is optional; unnamed allowed when single block).
        // Keyword tokens (KwPlay, KwCh, etc.) are distinct from Ident in logos, so a
        // bare Ident is unambiguously the block name.
        let name: Option<String> = if matches!(self.peek(), Some(Token::Ident(_))) {
            Some(self.expect_ident("harmony block name")?)
        } else {
            None
        };

        // Parse optional parameters on declaration line
        let mut play = false;
        let mut channel = None;
        let mut program = None;
        let mut voice = VoicingStrategy::Close;
        let mut octave = 4u8;
        let mut velocity = 72u8;
        let mut inv = Inversion::Fixed(0);
        let mut seen_params = std::collections::HashSet::new();

        // Parse params until newline
        while !matches!(self.peek(), Some(Token::Newline) | None) {
            match self.peek() {
                Some(Token::KwMode) => {
                    return Err(CompileError::ParseError {
                        message: "mode= is not valid on @harmony — declare @scale root=<r> mode=<m> instead".to_string(),
                        span: self.current_span(),
                    });
                }
                Some(Token::KwPlay) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "play",
                        "@harmony",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    play = self.expect_bool()?;
                }
                Some(Token::KwCh) => {
                    check_duplicate_param(&mut seen_params, "ch", "@harmony", self.current_span())?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    let ch_val = self.expect_positive_integer("ch")?;
                    if !(1..=16).contains(&ch_val) {
                        return Err(CompileError::ChannelOutOfRange {
                            name: name.clone().unwrap_or_else(|| "<unnamed>".to_string()),
                            span: self.current_span(),
                        });
                    }
                    channel = Some(ch_val as u8);
                }
                Some(Token::KwProg) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "prog",
                        "@harmony",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    program = Some(self.expect_midi_u7("prog")?);
                }
                Some(Token::KwVoice) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "voice",
                        "@harmony",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    voice = self.expect_voicing()?;
                }
                Some(Token::KwOct) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "oct",
                        "@harmony",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    octave = self.expect_octave("oct")?;
                }
                Some(Token::KwVel) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "vel",
                        "@harmony",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    let vel_span = self.current_span();
                    let vel_val = self.expect_positive_integer("vel")?;
                    if vel_val > 127 {
                        return Err(CompileError::VelocityOutOfRange {
                            context: format!(
                                "harmony '{}'",
                                name.clone().unwrap_or_else(|| "<unnamed>".to_string())
                            ),
                            span: vel_span,
                        });
                    }
                    velocity = vel_val as u8;
                }
                Some(Token::KwInv) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "inv",
                        "@harmony",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    inv = self.expect_inversion()?;
                }
                Some(Token::Comment) => {
                    self.advance();
                }
                _ => {
                    return Err(CompileError::ParseError {
                        message: format!(
                            "unexpected token in @harmony declaration: {:?}",
                            self.peek()
                        ),
                        span: self.current_span(),
                    });
                }
            }
        }

        // Skip the newline after declaration
        self.skip_newlines();

        // Parse bar grid lines, steps: blocks, and section: directives
        let mut bars: Vec<Bar> = Vec::new();
        let mut sections: Vec<Section> = Vec::new();

        while !self.at_block_boundary_or_end() {
            self.skip_newlines();
            if self.at_block_boundary_or_end() {
                break;
            }

            // Check for section: directive
            if matches!(self.peek(), Some(Token::KwSection)) {
                let section = self.parse_section_directive()?;
                sections.push(section);
                self.skip_newlines();
                continue;
            }

            // Check for steps: block (follows a bar line)
            if matches!(self.peek(), Some(Token::KwSteps)) {
                // steps: block applies to the preceding bar
                if bars.is_empty() {
                    return Err(CompileError::ParseError {
                        message: "steps: block with no preceding bar".to_string(),
                        span: self.current_span(),
                    });
                }
                let step_chords = self.parse_harmony_steps_block()?;
                let last_bar = bars.last_mut().ok_or_else(|| CompileError::ParseError {
                    message: "steps: block with no preceding bar".to_string(),
                    span: Span::new(span_start, span_start),
                })?;
                last_bar.steps = Some(step_chords);
                self.skip_newlines();
                continue;
            }

            // Parse a bar grid line (one or more bars separated by |)
            let line_bars = self.parse_bar_grid_line()?;
            bars.extend(line_bars);
            self.skip_newlines();
        }

        let span_end = self.current_span().end;
        Ok(HarmonyBlock {
            name,
            play,
            channel,
            program,
            voice,
            octave,
            velocity,
            inv,
            bars,
            sections,
            span: Some(Span::new(span_start, span_end)),
        })
    }

    /// Parse a bar grid line: chord symbols separated by `|` for bar boundaries.
    fn parse_bar_grid_line(&mut self) -> CompileResult<Vec<Bar>> {
        let mut bars = Vec::new();
        let mut current_chords: Vec<BarChord> = Vec::new();

        loop {
            // Skip comments
            self.skip_comments();

            // Check for end of line
            if matches!(self.peek(), Some(Token::Newline) | None) {
                break;
            }

            // Check for bar separator
            if matches!(self.peek(), Some(Token::Pipe)) {
                self.advance(); // consume |
                                // Finish current bar
                if !current_chords.is_empty() {
                    bars.push(Bar {
                        chords: current_chords,
                        steps: None,
                        span: None,
                    });
                    current_chords = Vec::new();
                }
                continue;
            }

            // Parse a chord symbol (identifier)
            let chord = self.parse_inline_chord_symbol()?;

            // Check for beat assignment (:N)
            let beats = if matches!(self.peek(), Some(Token::Colon)) {
                self.advance(); // consume :
                let beats_span = self.current_span();
                let b = self.expect_positive_integer("beat count")?;
                if b > 255 {
                    return Err(CompileError::ParseError {
                        message: format!("beat count must be 1-255, got {b}"),
                        span: beats_span,
                    });
                }
                Some(b as u8)
            } else {
                None
            };

            current_chords.push(BarChord {
                chord,
                beats,
                span: None,
            });
        }

        // Don't forget the last bar on the line
        if !current_chords.is_empty() {
            bars.push(Bar {
                chords: current_chords,
                steps: None,
                span: None,
            });
        }

        Ok(bars)
    }

    /// Parse a chord symbol from the current token position.
    ///
    /// Chord symbols in the harmony block are identifiers (e.g., "Cmaj7", "Dm7b5").
    /// We consume the identifier and parse it as a chord symbol.
    /// Sharp roots like F#7 are handled by consuming Ident("F") + Sharp + trailing tokens.
    fn parse_inline_chord_symbol(&mut self) -> CompileResult<ChordSymbol> {
        self.skip_comments();
        let span = self.current_span();

        match self.peek().cloned() {
            Some(Token::Ident(s)) => {
                self.advance();
                // Reassemble sharp roots (`F#7`) and sharp alterations
                // (`G7#9`, `C7b9#11`) that the lexer split into multiple
                // tokens. See `reassemble_chord_symbol` for the
                // sharp-root vs sharp-alteration disambiguation rule.
                let chord_str = self.reassemble_chord_symbol(s)?;
                // Handle slash suffix: either quality continuation (C6/9) or slash bass (C/G).
                // After consuming the base chord ident (and optional sharp), check for Slash.
                // Slash + Integer → quality continuation (e.g., "6/9" suffix).
                // Slash + Ident  → slash bass note (e.g., "/G").
                let chord_str = if matches!(self.peek(), Some(Token::Slash)) {
                    // Consume the Slash, then determine context by what follows.
                    self.advance();
                    match self.peek().cloned() {
                        Some(Token::Integer(n)) => {
                            // Quality-slash suffix: e.g., C6/9
                            self.advance();
                            format!("{chord_str}/{n}")
                        }
                        Some(Token::Ident(bass)) => {
                            // Slash bass: e.g., C/G or Cmaj7/E
                            self.advance();
                            format!("{chord_str}/{bass}")
                        }
                        _ => chord_str, // Lone slash — pass through, let chord parser error
                    }
                } else {
                    chord_str
                };
                parse_chord_symbol_with_context(&chord_str, &self.tonal_context).map_err(|e| {
                    CompileError::ParseError {
                        message: format!("invalid chord symbol '{}': {}", chord_str, e),
                        span,
                    }
                })
            }
            other => Err(CompileError::ParseError {
                message: format!("expected chord symbol, found {other:?}"),
                span,
            }),
        }
    }

    /// Parse a `steps:` block within a harmony block.
    ///
    /// `steps:` is followed by chord symbols on the same line or indented on the next line.
    fn parse_harmony_steps_block(&mut self) -> CompileResult<Vec<ChordSymbol>> {
        self.expect(&Token::KwSteps)?;
        self.expect(&Token::Colon)?;

        let mut chords = Vec::new();

        // Parse chord symbols until newline or block boundary
        loop {
            self.skip_comments();
            if matches!(self.peek(), Some(Token::Newline) | None) {
                break;
            }
            if self.is_block_start() {
                break;
            }
            let chord = self.parse_inline_chord_symbol()?;
            chords.push(chord);
        }

        if chords.is_empty() {
            return Err(CompileError::ParseError {
                message: "steps: block must contain at least one chord symbol".to_string(),
                span: self.current_span(),
            });
        }

        Ok(chords)
    }

    /// Parse a `section:` modulation directive.
    fn parse_section_directive(&mut self) -> CompileResult<Section> {
        self.expect(&Token::KwSection)?;
        self.expect(&Token::Colon)?;

        let mut bar = None;
        let mut mode = None;
        let mut root = None;

        // Parse key=value pairs until newline
        while !matches!(self.peek(), Some(Token::Newline) | None) {
            self.skip_comments();
            if matches!(self.peek(), Some(Token::Newline) | None) {
                break;
            }

            match self.peek().cloned() {
                Some(Token::KwBar) => {
                    self.advance();
                    self.expect(&Token::Equals)?;
                    bar = Some(self.expect_positive_u32("bar")?);
                }
                Some(Token::KwMode) => {
                    self.advance();
                    self.expect(&Token::Equals)?;
                    mode = Some(self.expect_ident("mode")?);
                }
                Some(Token::KwRoot) => {
                    self.advance();
                    self.expect(&Token::Equals)?;
                    let root_name = self.parse_note_root()?;
                    let (pc, _) =
                        parse_note_name(&root_name).ok_or_else(|| CompileError::ParseError {
                            message: format!("invalid root note '{root_name}'"),
                            span: self.current_span(),
                        })?;
                    root = Some(pc);
                }
                _ => {
                    return Err(CompileError::ParseError {
                        message: format!(
                            "unexpected token in section: directive: {:?}",
                            self.peek()
                        ),
                        span: self.current_span(),
                    });
                }
            }
        }

        let bar = bar.ok_or_else(|| CompileError::ParseError {
            message: "section: directive requires bar=N".to_string(),
            span: self.current_span(),
        })?;

        Ok(Section {
            bar,
            mode,
            root,
            span: None,
        })
    }

    // ── Pattern Block Parsing ────────────────────────────────────────

    /// Parse a `@pattern` block.
    ///
    /// Handles two forms:
    /// 1. Pattern with step body: `@pattern name steps=N unit=F [params]\n body...`
    /// 2. Pattern assignment: `@pattern name = expression`
    pub fn parse_pattern_block(&mut self) -> CompileResult<PatternBlock> {
        let block_span_start = self.current_span().start;
        self.expect(&Token::AtPattern)?;

        let name = self.expect_ident("pattern name")?;

        // Check for assignment form: @pattern name = expr
        if matches!(self.peek(), Some(Token::Equals)) {
            self.advance(); // consume =
            let expr = self.parse_pattern_expr()?;
            // Skip to end of line
            self.skip_to_newline();
            let block_span_end = self.prev_span().end;
            return Ok(PatternBlock {
                name,
                steps: 0,
                unit: (0, 1),
                velocity: 84,
                gate: 0.9,
                octave: 4,
                transforms: Vec::new(),
                body: PatternBody::Expression(expr),
                span: Some(Span::new(block_span_start, block_span_end)),
            });
        }

        // Parse parameters
        let mut steps: Option<u32> = None;
        let mut unit: Option<(u32, u32)> = None;
        let mut velocity = 84u8;
        let mut gate = 0.9f64;
        let mut octave = 4u8;
        let mut seen_params = std::collections::HashSet::new();

        while !matches!(self.peek(), Some(Token::Newline) | None) {
            self.skip_comments();
            if matches!(self.peek(), Some(Token::Newline) | None) {
                break;
            }
            match self.peek() {
                Some(Token::KwSteps) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "steps",
                        "@pattern",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    steps = Some(self.expect_positive_u32("steps")?);
                }
                Some(Token::KwUnit) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "unit",
                        "@pattern",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    unit = Some(self.expect_fraction("unit")?);
                }
                Some(Token::KwVel) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "vel",
                        "@pattern",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    let vel_val = self.expect_positive_integer("vel")?;
                    if vel_val > 127 {
                        return Err(CompileError::VelocityOutOfRange {
                            context: format!("pattern '{name}'"),
                            span: self.current_span(),
                        });
                    }
                    velocity = vel_val as u8;
                }
                Some(Token::KwGate) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "gate",
                        "@pattern",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    let gate_val = self.expect_positive_number("gate")?;
                    if !(0.0..=1.0).contains(&gate_val) {
                        return Err(CompileError::GateOutOfRange {
                            context: format!("pattern '{name}'"),
                            span: self.current_span(),
                        });
                    }
                    gate = gate_val;
                }
                Some(Token::KwOct) => {
                    check_duplicate_param(
                        &mut seen_params,
                        "oct",
                        "@pattern",
                        self.current_span(),
                    )?;
                    self.advance();
                    self.expect(&Token::Equals)?;
                    octave = self.expect_octave("oct")?;
                }
                Some(Token::Comment) => {
                    self.advance();
                }
                Some(Token::Arrow) | Some(Token::Colon) => {
                    break;
                }
                Some(Token::Pipe) => {
                    // Old baked-in transform syntax used `|` before v0.5.
                    let span = self.current_span();
                    return Err(CompileError::DeprecatedPipeOperator { span });
                }
                _ => {
                    return Err(CompileError::ParseError {
                        message: format!(
                            "unexpected token in @pattern declaration: {:?}",
                            self.peek()
                        ),
                        span: self.current_span(),
                    });
                }
            }
        }

        // Parse baked-in transforms on the declaration line: -> transform -> transform ...
        let mut transforms = Vec::new();
        while matches!(self.peek(), Some(Token::Arrow)) {
            self.advance();
            transforms.push(self.parse_transform_call()?);
        }

        // ── Inline pattern body: `@pattern p unit=1/4: ^1 ^3 ^5 ^7` ───
        // A `:` after parameters means the step body follows on the same line,
        // with each whitespace-separated token group being one step.
        if matches!(self.peek(), Some(Token::Colon)) {
            self.advance(); // consume :

            let unit = unit.ok_or_else(|| CompileError::ParseError {
                message: format!("pattern '{name}': unit= is required"),
                span: self.current_span(),
            })?;

            let mut step_lines: Vec<StepLine> = Vec::new();

            while !matches!(self.peek(), Some(Token::Newline) | None) {
                self.skip_comments();
                if matches!(self.peek(), Some(Token::Newline) | None) {
                    break;
                }

                // Parse one step (token or +connected cluster)
                let first = self.parse_step_token()?;
                let mut tokens = vec![first];
                while matches!(self.peek(), Some(Token::Plus)) {
                    self.advance();
                    tokens.push(self.parse_step_token()?);
                }
                let token_spans = vec![None; tokens.len()];
                step_lines.push(StepLine {
                    tokens,
                    token_spans,
                    span: None,
                });
            }

            // Infer steps if not declared; validate if declared
            let actual_steps = step_lines.len() as u32;
            if let Some(declared) = steps {
                if actual_steps != declared {
                    return Err(CompileError::StepCountMismatch {
                        name: name.clone(),
                        declared,
                        actual: actual_steps,
                        span: self.current_span(),
                    });
                }
            }

            let block_span_end = self.prev_span().end;
            return Ok(PatternBlock {
                name,
                steps: actual_steps,
                unit,
                velocity,
                gate,
                octave,
                transforms,
                body: PatternBody::Steps(step_lines),
                span: Some(Span::new(block_span_start, block_span_end)),
            });
        }

        // ── Multi-line pattern body (existing path) ───────────────────
        let unit = unit.ok_or_else(|| CompileError::ParseError {
            message: format!("pattern '{name}': unit= is required"),
            span: self.current_span(),
        })?;

        self.skip_newlines();

        // Parse step body lines
        let mut step_lines: Vec<StepLine> = Vec::new();

        while !self.at_block_boundary_or_end() {
            self.skip_newlines();
            if self.at_block_boundary_or_end() {
                break;
            }

            // Check if this line is a step line or something else
            // Step lines contain step tokens; stop if we see a non-step token
            if self.is_step_line_start() {
                let line = self.parse_step_line()?;
                step_lines.push(line);
            } else {
                break;
            }
            // Consume the newline at end of step line
            if matches!(self.peek(), Some(Token::Newline)) {
                self.advance();
            }
        }

        // Infer steps if not declared; validate if declared
        let actual_steps = step_lines.len() as u32;
        if let Some(declared) = steps {
            if actual_steps != declared {
                return Err(CompileError::StepCountMismatch {
                    name: name.clone(),
                    declared,
                    actual: actual_steps,
                    span: self.current_span(),
                });
            }
        }

        let block_span_end = self.prev_span().end;
        Ok(PatternBlock {
            name,
            steps: actual_steps,
            unit,
            velocity,
            gate,
            octave,
            transforms,
            body: PatternBody::Steps(step_lines),
            span: Some(Span::new(block_span_start, block_span_end)),
        })
    }

    /// Check if the current token could start a step line.
    fn is_step_line_start(&self) -> bool {
        matches!(
            self.peek(),
            Some(
                Token::Caret        // ^degree
                | Token::PercentSign // %n chord ordinal
                | Token::Dot        // rest
                | Token::Tilde      // tie
                | Token::Dollar     // $chord
                | Token::LParen     // subdivision
                | Token::LBrace     // variant
                | Token::Ident(_)   // absolute pitch, MIDI number, drum hit
                | Token::Sharp // could be start of something
            )
        )
    }

    /// Parse a single step line (one line of step tokens).
    ///
    /// A step line contains tokens separated by `+` for simultaneous notes.
    /// Each token may have annotations `[...]` attached.
    fn parse_step_line(&mut self) -> CompileResult<StepLine> {
        let line_start = self.peek_spanned().map(|t| t.start).unwrap_or(0);
        let mut tokens = Vec::new();

        // Parse first token
        let first = self.parse_step_token()?;
        tokens.push(first);

        // Parse additional simultaneous tokens separated by +
        while matches!(self.peek(), Some(Token::Plus)) {
            self.advance(); // consume +
            let tok = self.parse_step_token()?;
            tokens.push(tok);
        }

        let line_end = self.prev_span().end;
        let token_spans = vec![None; tokens.len()];
        Ok(StepLine {
            tokens,
            token_spans,
            span: Some(Span::new(line_start, line_end)),
        })
    }

    /// Parse a single step token (the atomic unit within a step line).
    fn parse_step_token(&mut self) -> CompileResult<StepToken> {
        self.skip_comments();

        match self.peek().cloned() {
            // Rest
            Some(Token::Dot) => {
                self.advance();
                Ok(StepToken::Rest)
            }

            // Tie
            Some(Token::Tilde) => {
                self.advance();
                Ok(StepToken::Tie)
            }

            // Degree token: ^
            Some(Token::Caret) => self.parse_degree_token(),

            // Chord ordinal token: %<integer>[/<octave>]
            Some(Token::PercentSign) => self.parse_chord_ordinal_token(),

            // Chord symbol in step context: $ or current chord: $chord
            Some(Token::Dollar) => {
                self.advance(); // consume $
                                // Check for $chord (current chord token)
                if matches!(self.peek(), Some(Token::Ident(s)) if s == "chord") {
                    self.advance(); // consume 'chord'
                    let annotations = self.maybe_parse_annotations()?;
                    return Ok(StepToken::CurrentChord { annotations });
                }
                // Old syntax: $_ — produce deprecation error
                if matches!(self.peek(), Some(Token::Ident(s)) if s == "_") {
                    let span = self.current_span();
                    return Err(CompileError::DeprecatedCurrentChordToken { span });
                }
                let chord_name = self.expect_ident("chord symbol")?;
                // Reassemble sharp roots ($F#7) and sharp alterations
                // ($G7#9, $C7b9#11) split by the lexer. See
                // `reassemble_chord_symbol` for the sharp-root vs
                // sharp-alteration disambiguation rule.
                let chord_name = self.reassemble_chord_symbol(chord_name)?;
                let chord = parse_chord_symbol_with_context(&chord_name, &self.tonal_context)
                    .map_err(|e| CompileError::ParseError {
                        message: format!("invalid chord symbol '${chord_name}': {e}"),
                        span: self.current_span(),
                    })?;
                let annotations = self.maybe_parse_annotations()?;
                Ok(StepToken::ChordStep { chord, annotations })
            }

            // Subdivision bracket
            Some(Token::LParen) => self.parse_subdivision(),

            // Variant bracket
            Some(Token::LBrace) => self.parse_variant(),

            // Identifier: could be absolute pitch (C4), MIDI number (n60), or drum hit
            Some(Token::Ident(s)) => self.parse_ident_step_token(s),

            other => Err(CompileError::ParseError {
                message: format!("expected step token, found {other:?}"),
                span: self.current_span(),
            }),
        }
    }

    /// Parse a degree token: `^[b|#]<degree>[/<octave>]`
    fn parse_degree_token(&mut self) -> CompileResult<StepToken> {
        self.advance(); // consume ^

        // Check for accidental and degree number.
        // The lexer may combine `b` + digits into a single identifier like "b3",
        // so we handle that case along with the normal `b` / `#` + Integer path.
        let (accidental, degree) = match self.peek().cloned() {
            Some(Token::Ident(s)) if s == "b" => {
                self.advance();
                // flat accidental, degree follows as integer
                let n = self.expect_degree_number()?;
                (-1i8, n)
            }
            Some(Token::Ident(s))
                if s.starts_with('b') && s[1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                // "b3" etc. — flat + degree in one token
                let n: i64 = s[1..].parse().map_err(|_| CompileError::ParseError {
                    message: format!("invalid degree in ^{s}"),
                    span: self.current_span(),
                })?;
                self.advance();
                if !(1..=13).contains(&n) {
                    return Err(CompileError::ParseError {
                        message: format!("scale degree must be 1-13, got {n}"),
                        span: self.current_span(),
                    });
                }
                (-1i8, n as u8)
            }
            Some(Token::Sharp) => {
                self.advance();
                let n = self.expect_degree_number()?;
                (1i8, n)
            }
            Some(Token::Integer(n)) => {
                self.advance();
                if !(1..=13).contains(&n) {
                    return Err(CompileError::ParseError {
                        message: format!("scale degree must be 1-13, got {n}"),
                        span: self.current_span(),
                    });
                }
                (0i8, n as u8)
            }
            _ => {
                return Err(CompileError::ParseError {
                    message: "expected degree number after ^".to_string(),
                    span: self.current_span(),
                });
            }
        };

        // Check for octave displacement: /N
        let octave = if matches!(self.peek(), Some(Token::Slash)) {
            self.advance(); // consume /
            match self.peek() {
                Some(Token::Integer(n)) => {
                    let n = *n;
                    let span = self.current_span();
                    self.advance();
                    if !(0..=10).contains(&n) {
                        return Err(CompileError::ParseError {
                            message: format!("octave displacement must be 0-10, got {n}"),
                            span,
                        });
                    }
                    Some(n as u8)
                }
                _ => {
                    return Err(CompileError::ParseError {
                        message: "expected octave number after /".to_string(),
                        span: self.current_span(),
                    });
                }
            }
        } else {
            None
        };

        let annotations = self.maybe_parse_annotations()?;

        Ok(StepToken::Degree {
            degree,
            accidental,
            octave,
            annotations,
        })
    }

    /// Parse a chord ordinal token: `%<degree>[/<octave>]`
    ///
    /// `%1` = root (1st chord tone), `%2` = 2nd chord tone, etc.
    /// Optional `/<octave>` forces an explicit octave.
    fn parse_chord_ordinal_token(&mut self) -> CompileResult<StepToken> {
        self.advance(); // consume %

        // Expect an integer degree ≥ 1
        let degree = match self.peek().cloned() {
            Some(Token::Integer(n)) => {
                let span = self.current_span();
                self.advance();
                if n < 1 {
                    return Err(CompileError::ParseError {
                        message: format!("chord ordinal must be ≥ 1, got {n}"),
                        span: self.current_span(),
                    });
                }
                if n > 127 {
                    return Err(CompileError::ParseError {
                        message: format!("chord ordinal must be 1-127, got {n}"),
                        span,
                    });
                }
                n as u32
            }
            other => {
                return Err(CompileError::ParseError {
                    message: format!("expected ordinal number after %, found {other:?}"),
                    span: self.current_span(),
                });
            }
        };

        // Optional forced octave: /<integer>
        let octave = if matches!(self.peek(), Some(Token::Slash)) {
            self.advance(); // consume /
            match self.peek().cloned() {
                Some(Token::Integer(n)) => {
                    let span = self.current_span();
                    self.advance();
                    if !(0..=10).contains(&n) {
                        return Err(CompileError::ParseError {
                            message: format!("octave displacement must be 0-10, got {n}"),
                            span,
                        });
                    }
                    Some(n as u8)
                }
                _ => {
                    return Err(CompileError::ParseError {
                        message: "expected octave number after / in chord ordinal".to_string(),
                        span: self.current_span(),
                    });
                }
            }
        } else {
            None
        };

        let annotations = self.maybe_parse_annotations()?;

        Ok(StepToken::ChordOrdinal {
            degree,
            octave,
            annotations,
        })
    }

    /// Parse an identifier-based step token (absolute pitch, MIDI number, or drum hit).
    fn parse_ident_step_token(&mut self, s: String) -> CompileResult<StepToken> {
        self.advance(); // consume the identifier

        // Try MIDI note number: n<digits>. An `n` followed only by digits is
        // unambiguously a MIDI number token, so an out-of-range value is a
        // hard error rather than a silent fall-through to the drum-hit path
        // (which would compile a typo like `n200` into silence).
        if let Some(rest) = s.strip_prefix('n') {
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                let span = self.prev_span();
                let note = rest
                    .parse::<u32>()
                    .ok()
                    .filter(|&n| n <= 127)
                    .ok_or_else(|| CompileError::ParseError {
                        message: format!("MIDI note number must be 0-127, got n{rest}"),
                        span,
                    })?;
                let annotations = self.maybe_parse_annotations()?;
                return Ok(StepToken::MidiNumber {
                    note: note as u8,
                    annotations,
                });
            }
        }

        // Try absolute pitch: <letter>[#|b]<octave>
        if let Some((midi_note, _consumed)) = try_parse_absolute_pitch(&s) {
            let annotations = self.maybe_parse_annotations()?;
            return Ok(StepToken::AbsolutePitch {
                midi_note,
                annotations,
            });
        }

        // Otherwise treat as drum hit name
        let annotations = self.maybe_parse_annotations()?;
        Ok(StepToken::DrumHit {
            name: s,
            annotations,
        })
    }

    /// Parse a subdivision bracket: `(token token ...)`
    fn parse_subdivision(&mut self) -> CompileResult<StepToken> {
        self.enter_nested()?;
        let result = self.parse_subdivision_inner();
        self.exit_nested();
        result
    }

    /// Body of `parse_subdivision`, wrapped by the nesting-depth guard.
    fn parse_subdivision_inner(&mut self) -> CompileResult<StepToken> {
        self.advance(); // consume (

        let mut tokens = Vec::new();
        while !matches!(self.peek(), Some(Token::RParen) | None) {
            self.skip_comments();
            if matches!(self.peek(), Some(Token::RParen) | None) {
                break;
            }
            let first = self.parse_step_token()?;
            if matches!(self.peek(), Some(Token::Plus)) {
                // `+` inside a subdivision: the joined tokens are
                // simultaneous and share ONE subdivision slot (spec §7.4 /
                // §7.10) — `(^1+^3 ^5)` has two slots, the first a
                // two-note chord.
                //
                // The cluster is encoded as a single-alternative variant
                // pool: a one-alternative pool is semantically identical
                // to a plain simultaneous cluster (it has no variability,
                // so `vary()` ignores it), and the pool emission path
                // already emits all tokens of the chosen alternative at
                // the same slot. Sharing the slot is what makes a
                // following `~` extend the whole chord — the tie
                // machinery keys on slot identity.
                let mut cluster = vec![first];
                while matches!(self.peek(), Some(Token::Plus)) {
                    self.advance(); // consume +
                    cluster.push(self.parse_step_token()?);
                }
                tokens.push(StepToken::Variant {
                    alternatives: vec![cluster],
                });
            } else {
                tokens.push(first);
            }
        }

        if matches!(self.peek(), Some(Token::RParen)) {
            self.advance(); // consume )
        } else {
            return Err(CompileError::ParseError {
                message: "unclosed subdivision bracket".to_string(),
                span: self.current_span(),
            });
        }

        Ok(StepToken::Subdivision { tokens })
    }

    /// Parse a variant bracket: `{token, token, token}`
    fn parse_variant(&mut self) -> CompileResult<StepToken> {
        self.enter_nested()?;
        let result = self.parse_variant_inner();
        self.exit_nested();
        result
    }

    /// Body of `parse_variant`, wrapped by the nesting-depth guard.
    fn parse_variant_inner(&mut self) -> CompileResult<StepToken> {
        self.advance(); // consume {

        let mut alternatives: Vec<Vec<StepToken>> = Vec::new();
        let mut current: Vec<StepToken> = Vec::new();

        loop {
            self.skip_comments();
            match self.peek() {
                Some(Token::RBrace) => {
                    self.advance();
                    if !current.is_empty() {
                        alternatives.push(current);
                    }
                    break;
                }
                Some(Token::Comma) => {
                    self.advance();
                    alternatives.push(current);
                    current = Vec::new();
                }
                Some(Token::Pipe) => {
                    // Old variant separator before v0.5.
                    let span = self.current_span();
                    return Err(CompileError::DeprecatedVariantPipe { span });
                }
                None => {
                    return Err(CompileError::ParseError {
                        message: "unclosed variant bracket".to_string(),
                        span: self.current_span(),
                    });
                }
                _ => {
                    let tok = self.parse_step_token()?;
                    current.push(tok);
                }
            }
        }

        if alternatives.is_empty() {
            return Err(CompileError::ParseError {
                message: "variant pool must have at least one alternative".to_string(),
                span: self.current_span(),
            });
        }

        Ok(StepToken::Variant { alternatives })
    }

    /// Optionally parse step annotations: `[key:value ...]`
    fn maybe_parse_annotations(&mut self) -> CompileResult<Vec<Annotation>> {
        if !matches!(self.peek(), Some(Token::LBracket)) {
            return Ok(Vec::new());
        }
        self.advance(); // consume [

        let mut annotations = Vec::new();
        while !matches!(self.peek(), Some(Token::RBracket) | None) {
            self.skip_comments();
            if matches!(self.peek(), Some(Token::RBracket)) {
                break;
            }
            let ann = self.parse_single_annotation()?;
            annotations.push(ann);
        }

        if matches!(self.peek(), Some(Token::RBracket)) {
            self.advance(); // consume ]
        } else {
            return Err(CompileError::ParseError {
                message: "unclosed annotation bracket".to_string(),
                span: self.current_span(),
            });
        }

        Ok(annotations)
    }

    /// Parse a single annotation key:value pair.
    fn parse_single_annotation(&mut self) -> CompileResult<Annotation> {
        self.skip_comments();

        match self.peek().cloned() {
            Some(Token::KwVel) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let span = self.current_span();
                let val = self.expect_positive_integer("vel")?;
                if val > 127 {
                    return Err(CompileError::VelocityOutOfRange {
                        context: "step annotation [vel]".to_string(),
                        span,
                    });
                }
                Ok(Annotation::Vel(val as u8))
            }
            Some(Token::KwGate) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let gate_span = self.current_span();
                let val = self.expect_positive_number("gate")?;
                // Same range rule as gate= on @track / @pattern
                // (GateOutOfRange): a gate above 1.0 would overlap
                // following steps.
                if !(0.0..=1.0).contains(&val) {
                    return Err(CompileError::ParseError {
                        message: "step annotation [gate]: gate must be 0.0-1.0".to_string(),
                        span: gate_span,
                    });
                }
                Ok(Annotation::Gate(val))
            }
            Some(Token::KwDur) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let (num, denom) = self.expect_fraction("dur")?;
                Ok(Annotation::Dur(num, denom))
            }
            Some(Token::KwShift) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let tv = self.parse_timing_value()?;
                Ok(Annotation::Shift(tv))
            }
            Some(Token::KwLshift) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let tv = self.parse_timing_value()?;
                Ok(Annotation::LShift(tv))
            }
            Some(Token::KwOct) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let val = self.expect_octave("oct")?;
                Ok(Annotation::Oct(val))
            }
            Some(Token::KwExpr) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let cc = self.parse_cc_value()?;
                Ok(Annotation::Expr(cc))
            }
            Some(Token::KwDyn) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let cc = self.parse_cc_value()?;
                Ok(Annotation::Dyn(cc))
            }
            Some(Token::KwSus) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let val = self.expect_midi_u7("sus")?;
                Ok(Annotation::Sus(val))
            }
            Some(Token::KwPan) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let cc = self.parse_cc_value()?;
                Ok(Annotation::Pan(cc))
            }
            Some(Token::KwVol) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let cc = self.parse_cc_value()?;
                Ok(Annotation::Vol(cc))
            }
            Some(Token::KwPb) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let span = self.current_span();
                let val = self.expect_integer("pb")?;
                if !(-8192..=8191).contains(&val) {
                    return Err(CompileError::ParseError {
                        message: format!("pb must be -8192 to 8191, got {val}"),
                        span,
                    });
                }
                Ok(Annotation::PitchBend(val as i16))
            }
            Some(Token::KwAt) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let val = self.expect_midi_u7("at")?;
                Ok(Annotation::Aftertouch(val))
            }
            Some(Token::KwCc(n)) => {
                let cc_num = n;
                self.advance();
                self.expect(&Token::Colon)?;
                let cc = self.parse_cc_value()?;
                Ok(Annotation::Cc(cc_num, cc))
            }
            // Conditional annotations
            Some(Token::KwEvery) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let n = self.expect_positive_u32("every:N")?;
                Ok(Annotation::Condition(crate::ast::StepCondition::Every(n)))
            }
            Some(Token::KwCond) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let x = self.expect_positive_u32("cond:X")?;
                self.expect(&Token::Colon)?;
                let y = self.expect_positive_u32("cond:Y")?;
                Ok(Annotation::Condition(crate::ast::StepCondition::Cond(x, y)))
            }
            Some(Token::KwOnce) => {
                self.advance();
                Ok(Annotation::Condition(crate::ast::StepCondition::Once))
            }
            Some(Token::KwPre) => {
                self.advance();
                Ok(Annotation::Condition(crate::ast::StepCondition::Pre))
            }
            Some(Token::KwRatch) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let n = self.expect_positive_u32("ratch:N")?;
                Ok(Annotation::Ratch(n))
            }
            Some(Token::KwRatchDecay) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let val = self.expect_positive_number("ratch_decay value")?;
                Ok(Annotation::RatchDecay(val))
            }
            Some(Token::KwProb) => {
                self.advance();
                self.expect(&Token::Colon)?;
                let prob = match self.peek().cloned() {
                    Some(Token::Percent(p)) => {
                        self.advance();
                        p / 100.0
                    }
                    Some(Token::Float(f)) => {
                        self.advance();
                        f
                    }
                    Some(Token::Integer(n)) => {
                        self.advance();
                        n as f64
                    }
                    other => {
                        return Err(CompileError::ParseError {
                            message: format!(
                                "expected probability value (0.0–1.0 or percent), got {other:?}"
                            ),
                            span: self.current_span(),
                        })
                    }
                };
                Ok(Annotation::Prob(prob.clamp(0.0, 1.0)))
            }
            Some(Token::KwGlide) => {
                self.advance();
                if matches!(self.peek(), Some(Token::Colon)) {
                    self.advance();
                    let f = self.expect_positive_number("glide fraction")?;
                    Ok(Annotation::Glide(Some(f.clamp(0.0, 1.0))))
                } else {
                    Ok(Annotation::Glide(None))
                }
            }
            other => Err(CompileError::ParseError {
                message: format!("unknown annotation key: {other:?}"),
                span: self.current_span(),
            }),
        }
    }

    /// Parse a CC value: either static integer or ramp `start->end`.
    fn parse_cc_value(&mut self) -> CompileResult<CcValue> {
        let start = self.expect_midi_u7("CC value")?;

        if matches!(self.peek(), Some(Token::Arrow)) {
            self.advance(); // consume ->
            let end = self.expect_midi_u7("CC ramp end")?;
            Ok(CcValue::Ramp { start, end })
        } else {
            Ok(CcValue::Static(start))
        }
    }

    /// Parse a timing value: percent, fraction, or milliseconds.
    fn parse_timing_value(&mut self) -> CompileResult<TimingValue> {
        self.skip_comments();
        match self.peek().cloned() {
            Some(Token::Percent(p)) => {
                self.advance();
                Ok(TimingValue::Percent(p))
            }
            Some(Token::Milliseconds(ms)) => {
                self.advance();
                Ok(TimingValue::Milliseconds(ms))
            }
            Some(Token::Integer(_)) => {
                // Could be a fraction N/M or just an integer
                let num = self.expect_i32("timing value")?;
                if matches!(self.peek(), Some(Token::Slash)) {
                    self.advance(); // consume /
                    let denom = self.expect_positive_u32("timing denominator")?;
                    Ok(TimingValue::Fraction(num, denom))
                } else {
                    // Bare integer — treat as percent? No, as fraction of whole note
                    // Actually per spec, timing values must be %, fraction, or ms
                    // A bare integer isn't valid... but let's be lenient and treat as ticks
                    Ok(TimingValue::Fraction(num, 1))
                }
            }
            other => Err(CompileError::ParseError {
                message: format!("expected timing value (%, fraction, or ms), found {other:?}"),
                span: self.current_span(),
            }),
        }
    }

    /// Parse a pattern composition expression (for assignment and play: directives).
    ///
    /// Precedence (loosest to tightest):
    ///   1. `->` — transform pipe (loosest, left-associative)
    ///   2. `>>` / `~>>` — concatenation (right-associative)
    ///   3. `*` / `*~` — repeat (left-associative)
    ///   4. `()` / identifier — primary (tightest)
    pub fn parse_pattern_expr(&mut self) -> CompileResult<crate::ast::PatternExpr> {
        let mut expr = self.parse_concat_expr()?;

        // `->` is the loosest-binding operator (transform pipe)
        loop {
            self.skip_comments();
            if matches!(self.peek(), Some(Token::Arrow)) {
                self.advance();
                let transform = self.parse_transform_call()?;
                expr = crate::ast::PatternExpr::Transform {
                    pattern: Box::new(expr),
                    transform,
                };
            } else if matches!(self.peek(), Some(Token::Pipe)) {
                // Old syntax: `|` was the transform pipe before v0.5.
                let span = self.current_span();
                return Err(CompileError::DeprecatedPipeOperator { span });
            } else {
                break;
            }
        }

        Ok(expr)
    }

    /// Parse concatenation expressions (`>>`, `~>>`). Right-associative.
    ///
    /// Implemented as a loop (collect operands, then fold from the right)
    /// rather than right-recursion so that a pathological `a >> a >> ...`
    /// chain cannot overflow the stack.
    fn parse_concat_expr(&mut self) -> CompileResult<crate::ast::PatternExpr> {
        let mut items = vec![self.parse_repeat_expr()?];
        // `true` marks a soft concat (`~>>`), `false` a hard concat (`>>`).
        let mut soft_ops: Vec<bool> = Vec::new();

        loop {
            self.skip_comments();
            match self.peek() {
                Some(Token::RShift) => {
                    self.advance();
                    soft_ops.push(false);
                }
                Some(Token::TildeRShift) => {
                    self.advance();
                    soft_ops.push(true);
                }
                _ => break,
            }
            items.push(self.parse_repeat_expr()?);
        }

        // Fold right-associatively: a >> b >> c → Concat(a, Concat(b, c)).
        let mut iter = items.into_iter().rev();
        let mut expr = iter
            .next()
            .expect("items always holds at least one expression");
        for (left, soft) in iter.zip(soft_ops.into_iter().rev()) {
            expr = if soft {
                crate::ast::PatternExpr::ConcatSoft {
                    left: Box::new(left),
                    right: Box::new(expr),
                }
            } else {
                crate::ast::PatternExpr::Concat {
                    left: Box::new(left),
                    right: Box::new(expr),
                }
            };
        }
        Ok(expr)
    }

    /// Parse repeat expressions (`*`, `*~`). Left-associative.
    fn parse_repeat_expr(&mut self) -> CompileResult<crate::ast::PatternExpr> {
        let mut expr = self.parse_primary_expr()?;

        loop {
            self.skip_comments();
            match self.peek() {
                Some(Token::Star) => {
                    self.advance();
                    let count = self.expect_bounded_count("repetition count")?;
                    expr = crate::ast::PatternExpr::Repeat {
                        pattern: Box::new(expr),
                        count,
                    };
                }
                Some(Token::StarTilde) => {
                    self.advance();
                    let count = self.expect_bounded_count("repetition count")?;
                    expr = crate::ast::PatternExpr::RepeatSoft {
                        pattern: Box::new(expr),
                        count,
                    };
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    /// Parse a primary expression: identifier or parenthesized sub-expression.
    fn parse_primary_expr(&mut self) -> CompileResult<crate::ast::PatternExpr> {
        self.skip_comments();

        if matches!(self.peek(), Some(Token::LParen)) {
            self.advance();
            self.enter_nested()?;
            let expr_result = self.parse_pattern_expr();
            self.exit_nested();
            let expr = expr_result?;
            if !matches!(self.peek(), Some(Token::RParen)) {
                return Err(CompileError::ParseError {
                    message: "expected ')' to close grouped expression".to_string(),
                    span: self.current_span(),
                });
            }
            self.advance();
            return Ok(expr);
        }

        let name = self.expect_ident("pattern reference")?;
        let rate = if matches!(self.peek(), Some(Token::At)) {
            self.advance();
            Some(self.expect_positive_number("rate")?)
        } else {
            None
        };
        Ok(crate::ast::PatternExpr::Ref { name, rate })
    }

    /// Parse a transform function call.
    fn parse_transform_call(&mut self) -> CompileResult<crate::ast::TransformCall> {
        use crate::ast::TransformCall;

        match self.peek().cloned() {
            Some(Token::KwReverse) => {
                self.advance();
                Ok(TransformCall::Reverse)
            }
            Some(Token::KwInvert) => {
                self.advance();
                Ok(TransformCall::Invert)
            }
            Some(Token::KwRetrograde) => {
                self.advance();
                Ok(TransformCall::Retrograde)
            }
            Some(Token::KwMirror) => {
                self.advance();
                Ok(TransformCall::Mirror)
            }
            Some(Token::KwRotate) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let n = self.expect_i32("rotate amount")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Rotate(n))
            }
            Some(Token::KwStretch) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let (num, denom) = self.expect_fraction_or_int("stretch factor")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Stretch(num, denom))
            }
            Some(Token::KwCompress) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let (num, denom) = self.expect_fraction_or_int("compress factor")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Compress(num, denom))
            }
            Some(Token::KwTranspose) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let n = self.expect_i32("transpose semitones")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Transpose(n))
            }
            Some(Token::KwShiftOct) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let n = self.expect_i32("shift_oct amount")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::ShiftOct(n))
            }
            Some(Token::KwSubset) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let mut indices = Vec::new();
                indices.push(self.expect_positive_u32("subset index")?);
                while matches!(self.peek(), Some(Token::Comma)) {
                    self.advance();
                    indices.push(self.expect_positive_u32("subset index")?);
                }
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Subset(indices))
            }
            Some(Token::KwInterleave) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let name = self.expect_ident("interleave pattern")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Interleave(name))
            }
            Some(Token::KwHumanize) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let timing = self.parse_timing_value()?;
                self.expect(&Token::Comma)?;
                let intensity = self.expect_positive_number("humanize intensity")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Humanize(timing, intensity))
            }
            Some(Token::KwVary) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let prob = self.expect_positive_number("vary probability")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Vary(prob))
            }
            Some(Token::KwSwing) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let ratio = self.expect_positive_number("swing ratio")?;
                self.expect(&Token::Comma)?;
                let (num, denom) = self.expect_fraction("swing unit")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Swing(ratio, num, denom))
            }
            Some(Token::KwRubato) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let depth = self.expect_positive_number("rubato depth")?;
                self.expect(&Token::Comma)?;
                let curve = self.parse_curve()?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Rubato(depth, curve))
            }
            Some(Token::KwRitardando) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let depth = self.expect_positive_number("ritardando depth")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Ritardando(depth))
            }
            Some(Token::KwAccelerando) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let depth = self.expect_positive_number("accelerando depth")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Accelerando(depth))
            }
            Some(Token::KwAgogic) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let mut steps = Vec::new();
                steps.push(self.expect_positive_u32("agogic step")?);
                while matches!(self.peek(), Some(Token::Comma)) {
                    self.advance();
                    steps.push(self.expect_positive_u32("agogic step")?);
                }
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Agogic(steps))
            }
            Some(Token::KwBreathe) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let position = self.expect_positive_u32("breathe position")?;
                self.expect(&Token::Comma)?;
                let duration = self.parse_timing_value()?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Breathe(position, duration))
            }
            Some(Token::KwSwell) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let peak = self.expect_positive_number("swell peak")?;
                self.expect(&Token::Comma)?;
                let curve = self.parse_curve()?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Swell(peak, curve))
            }
            Some(Token::KwPhrase) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let tension = self.expect_positive_number("phrase tension")?;
                self.expect(&Token::Comma)?;
                let release = self.expect_positive_number("phrase release")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Phrase(tension, release))
            }
            Some(Token::KwEvolve) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let toggle = self.expect_non_negative_number("evolve toggle")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Evolve(toggle))
            }
            Some(Token::KwEuclidGate) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let pulses = self.expect_positive_u32("euclid_gate pulses")?;
                self.expect(&Token::Comma)?;
                let steps = self.expect_positive_u32("euclid_gate steps")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::EuclidGate(pulses, steps))
            }
            Some(Token::KwEcho) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let (rate_num, rate_den) = self.expect_fraction("echo rate")?;
                self.expect(&Token::Comma)?;
                let repeats = self.expect_bounded_count("echo repeats")?;
                self.expect(&Token::Comma)?;
                let decay = self.expect_positive_number("echo decay")?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::Echo(rate_num, rate_den, repeats, decay))
            }
            Some(Token::KwVelCurve) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let (wave, min_val, max_val, repeat) = self.parse_curve_params()?;
                if !(0.0..=127.0).contains(&min_val) || !(0.0..=127.0).contains(&max_val) {
                    return Err(CompileError::ParseError {
                        message: format!(
                            "vel_curve min/max must be 0-127, got min={min_val} max={max_val}"
                        ),
                        span: self.current_span(),
                    });
                }
                let min_u8 = min_val.round() as u8;
                let max_u8 = max_val.round() as u8;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::VelCurve(wave, min_u8, max_u8, repeat))
            }
            Some(Token::KwGateCurve) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let (wave, min_val, max_val, repeat) = self.parse_curve_params()?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::GateCurve(wave, min_val, max_val, repeat))
            }
            Some(Token::KwScaleLock) => {
                self.advance();
                self.expect(&Token::LParen)?;
                let (scale, root, snap_mode) = self.parse_scale_lock_params()?;
                self.expect(&Token::RParen)?;
                Ok(TransformCall::ScaleLock(scale, root, snap_mode))
            }
            Some(Token::KwArp) => {
                self.advance();
                let mut arp_pattern = crate::ast::ArpPattern::Up;
                let mut rate = (1u32, 8u32);
                let mut octaves = 1u32;
                if matches!(self.peek(), Some(Token::LParen)) {
                    self.advance(); // consume (
                    while !matches!(self.peek(), Some(Token::RParen) | None) {
                        // `rate` lexes as the KwRate keyword (it's also a
                        // track parameter), so accept it alongside idents.
                        let key = if matches!(self.peek(), Some(Token::KwRate)) {
                            self.advance();
                            "rate".to_string()
                        } else {
                            self.expect_ident("arp parameter name")?
                        };
                        self.expect(&Token::Equals)?;
                        match key.as_str() {
                            "pattern" => {
                                arp_pattern = match self.peek().cloned() {
                                    Some(Token::KwUp) => { self.advance(); crate::ast::ArpPattern::Up }
                                    Some(Token::KwDown) => { self.advance(); crate::ast::ArpPattern::Down }
                                    Some(Token::KwUpDown) => { self.advance(); crate::ast::ArpPattern::UpDown }
                                    Some(Token::KwRandom) => { self.advance(); crate::ast::ArpPattern::Random }
                                    other => return Err(CompileError::ParseError {
                                        message: format!("expected up/down/updown/random for arp pattern, got {other:?}"),
                                        span: self.current_span(),
                                    }),
                                };
                            }
                            "rate" => {
                                let (n, d) = self.expect_fraction("arp rate")?;
                                rate = (n, d);
                            }
                            "octaves" => {
                                octaves = self.expect_bounded_count("arp octaves")?;
                            }
                            other => {
                                return Err(CompileError::ParseError {
                                    message: format!("unknown arp parameter: {other}"),
                                    span: self.current_span(),
                                })
                            }
                        }
                        if matches!(self.peek(), Some(Token::Comma)) {
                            self.advance();
                        }
                    }
                    self.expect(&Token::RParen)?;
                }
                Ok(TransformCall::Arp {
                    pattern: arp_pattern,
                    rate,
                    octaves,
                })
            }
            other => Err(CompileError::ParseError {
                message: format!("expected transform name, found {other:?}"),
                span: self.current_span(),
            }),
        }
    }

    /// Parse an expressive curve name (ease_in, ease_out, ease_in_out, arch).
    fn parse_curve(&mut self) -> CompileResult<crate::ast::ExpressiveCurve> {
        use crate::ast::ExpressiveCurve;
        match self.peek() {
            Some(Token::KwEaseIn) => {
                self.advance();
                Ok(ExpressiveCurve::EaseIn)
            }
            Some(Token::KwEaseOut) => {
                self.advance();
                Ok(ExpressiveCurve::EaseOut)
            }
            Some(Token::KwEaseInOut) => {
                self.advance();
                Ok(ExpressiveCurve::EaseInOut)
            }
            Some(Token::KwArch) => {
                self.advance();
                Ok(ExpressiveCurve::Arch)
            }
            other => Err(CompileError::ParseError {
                message: format!(
                    "expected curve name (ease_in, ease_out, ease_in_out, arch), found {other:?}"
                ),
                span: self.current_span(),
            }),
        }
    }

    /// Parse a wave shape keyword (sine, tri, ramp, square, random).
    fn parse_wave_shape(&mut self) -> CompileResult<crate::ast::WaveShape> {
        use crate::ast::WaveShape;
        match self.peek() {
            Some(Token::KwSine) => {
                self.advance();
                Ok(WaveShape::Sine)
            }
            Some(Token::KwTri) => {
                self.advance();
                Ok(WaveShape::Tri)
            }
            Some(Token::KwRamp) => {
                self.advance();
                Ok(WaveShape::Ramp)
            }
            Some(Token::KwSquare) => {
                self.advance();
                Ok(WaveShape::Square)
            }
            Some(Token::KwRandom) => {
                self.advance();
                Ok(WaveShape::Random)
            }
            other => Err(CompileError::ParseError {
                message: format!(
                    "expected wave shape (sine, tri, ramp, square, random), found {other:?}"
                ),
                span: self.current_span(),
            }),
        }
    }

    /// Parse named curve parameters: `wave=<shape>, min=<num>, max=<num>[, repeat=<int>]`.
    /// Returns `(wave, min, max, repeat)`.
    fn parse_curve_params(&mut self) -> CompileResult<(crate::ast::WaveShape, f64, f64, u32)> {
        // wave=<shape>
        self.expect_ident_eq("wave")?;
        self.expect(&Token::Equals)?;
        let wave = self.parse_wave_shape()?;
        self.expect(&Token::Comma)?;

        // min=<num>
        self.expect_ident_eq("min")?;
        self.expect(&Token::Equals)?;
        let min_val = self.expect_non_negative_number("min")?;
        self.expect(&Token::Comma)?;

        // max=<num>
        self.expect_ident_eq("max")?;
        self.expect(&Token::Equals)?;
        let max_val = self.expect_non_negative_number("max")?;

        // optional: repeat=<int>
        let repeat = if self.peek() == Some(&Token::Comma) {
            self.advance();
            self.expect_ident_eq("repeat")?;
            self.expect(&Token::Equals)?;
            self.expect_positive_u32("repeat")?
        } else {
            1
        };

        Ok((wave, min_val, max_val, repeat))
    }

    /// Expect an `Ident` token with the exact given name.
    fn expect_ident_eq(&mut self, name: &str) -> CompileResult<()> {
        match self.peek() {
            Some(Token::Ident(s)) if s == name => {
                self.advance();
                Ok(())
            }
            other => Err(CompileError::ParseError {
                message: format!("expected '{name}', found {other:?}"),
                span: self.current_span(),
            }),
        }
    }

    /// Parse a snap mode keyword (down, up, filter).
    fn parse_snap_mode(&mut self) -> CompileResult<crate::ast::SnapMode> {
        use crate::ast::SnapMode;
        match self.peek() {
            Some(Token::KwDown) => {
                self.advance();
                Ok(SnapMode::Down)
            }
            Some(Token::KwUp) => {
                self.advance();
                Ok(SnapMode::Up)
            }
            Some(Token::KwFilter) => {
                self.advance();
                Ok(SnapMode::Filter)
            }
            other => Err(CompileError::ParseError {
                message: format!("expected snap mode (down, up, filter), found {other:?}"),
                span: self.current_span(),
            }),
        }
    }

    /// Parse scale_lock parameters: `[scale=<name>, root=<note>,] mode=<down|up|filter>`.
    /// Returns `(Option<scale_name>, Option<root_pc>, SnapMode)`.
    fn parse_scale_lock_params(
        &mut self,
    ) -> CompileResult<(Option<String>, Option<u8>, crate::ast::SnapMode)> {
        let mut scale: Option<String> = None;
        let mut root: Option<u8> = None;
        let mut snap_mode: Option<crate::ast::SnapMode> = None;

        // Parse named parameters in any order
        loop {
            match self.peek() {
                Some(Token::RParen) => break,
                Some(Token::Ident(s)) if s == "scale" => {
                    self.advance();
                    self.expect(&Token::Equals)?;
                    // Scale name is an identifier
                    let scale_name = self.expect_ident("scale name")?;
                    scale = Some(scale_name);
                }
                Some(Token::KwRoot) => {
                    self.advance();
                    self.expect(&Token::Equals)?;
                    // Root is a note name (A-G with optional # or b)
                    let root_pc = self.parse_root_pitch_class()?;
                    root = Some(root_pc);
                }
                Some(Token::KwMode) => {
                    self.advance();
                    self.expect(&Token::Equals)?;
                    snap_mode = Some(self.parse_snap_mode()?);
                }
                _ => {
                    return Err(CompileError::ParseError {
                        message: "expected 'scale=', 'root=', or 'mode=' parameter".to_string(),
                        span: self.current_span(),
                    });
                }
            }
            // Consume comma if present
            if self.peek() == Some(&Token::Comma) {
                self.advance();
            }
        }

        let snap_mode = snap_mode.ok_or_else(|| CompileError::ParseError {
            message: "scale_lock requires mode= parameter (down, up, filter)".to_string(),
            span: self.current_span(),
        })?;

        Ok((scale, root, snap_mode))
    }

    /// Reassemble a chord-symbol string from the token fragments the lexer
    /// produces after the base identifier.
    ///
    /// `#` is its own token, so a chord symbol containing a sharp arrives
    /// split: `F#9` as `Ident("F") + Sharp + Integer(9)`, `G7#9` as
    /// `Ident("G7") + Sharp + Integer(9)`, `C7b9#11` as
    /// `Ident("C7b9") + Sharp + Integer(11)`.
    ///
    /// Disambiguation rule: a SINGLE uppercase letter followed by `#` takes
    /// the `#` as a sharp ROOT — `F#9` is F-sharp dominant ninth. Any longer
    /// identifier followed by `#` + integer takes `#N` as a sharp
    /// ALTERATION — `F7#9` is F dominant seventh with a sharp ninth. An
    /// F-root sharp-nine chord is therefore written `F7#9`, never `F#9`.
    ///
    /// Alteration fragments are consumed iteratively so stacked alterations
    /// work: `G7#5#9` (`Sharp + Integer` twice) and `G7#5b9` (`Sharp +
    /// Integer`, then `Ident("b9")` — a flat continuation split off by the
    /// lexer). A flat continuation is only absorbed when it looks like an
    /// alteration (`b` followed by a digit), so a following chord such as
    /// `bVII7` in a bar line is never swallowed.
    fn reassemble_chord_symbol(&mut self, base: String) -> CompileResult<String> {
        let mut full = base;

        // Sharp ROOT: single uppercase letter + Sharp.
        if full.len() == 1
            && full.as_bytes()[0].is_ascii_uppercase()
            && matches!(self.peek(), Some(Token::Sharp))
        {
            self.advance(); // consume Sharp
            full.push('#');
            // Quality directly after the sharp root:
            // F#7 → Integer(7); F#m7 → Ident("m7").
            if let Some(Token::Integer(n)) = self.peek().cloned() {
                self.advance();
                full.push_str(&n.to_string());
            } else if let Some(Token::Ident(suffix)) = self.peek().cloned() {
                self.advance();
                full.push_str(&suffix);
            }
        }

        // Sharp alterations (`#N`) and flat continuations (`bN...`),
        // iteratively.
        loop {
            match self.peek().cloned() {
                Some(Token::Sharp) => {
                    self.advance(); // consume Sharp
                    match self.peek().cloned() {
                        Some(Token::Integer(n)) => {
                            self.advance();
                            full.push('#');
                            full.push_str(&n.to_string());
                        }
                        other => {
                            return Err(CompileError::ParseError {
                                message: format!(
                                    "expected alteration degree after '#' in chord symbol \
                                     '{full}#', found {other:?}"
                                ),
                                span: self.current_span(),
                            });
                        }
                    }
                }
                Some(Token::Ident(suffix))
                    if suffix.len() >= 2
                        && suffix.as_bytes()[0] == b'b'
                        && suffix.as_bytes()[1].is_ascii_digit() =>
                {
                    self.advance();
                    full.push_str(&suffix);
                }
                _ => break,
            }
        }

        Ok(full)
    }

    /// Parse a note root identifier, handling the sharp token reconstruction.
    /// The lexer tokenizes `F#` as `Ident("F") + Sharp`, so this helper reads
    /// `Ident` and optionally consumes a following `Sharp` token, returning
    /// the reconstructed name (e.g. "F#"). Flat roots like "Gb" are already
    /// a single `Ident` token.
    fn parse_note_root(&mut self) -> CompileResult<String> {
        let mut name = self.expect_ident("root note name")?;
        if name.len() == 1
            && name.as_bytes()[0].is_ascii_uppercase()
            && matches!(self.peek(), Some(Token::Sharp))
        {
            self.advance(); // consume Sharp
            name.push('#');
        }
        Ok(name)
    }

    /// Parse a root pitch class from a note name (A-G with optional # or b).
    /// Returns the pitch class (0=C, 1=C#, ... 11=B).
    fn parse_root_pitch_class(&mut self) -> CompileResult<u8> {
        let name = self.parse_note_root()?;
        crate::harmony::parse_note_name(&name)
            .map(|(pc, _)| pc)
            .ok_or_else(|| CompileError::ParseError {
                message: format!("invalid root note name: '{name}'"),
                span: self.current_span(),
            })
    }

    /// Skip tokens until the next newline or end of input.
    fn skip_to_newline(&mut self) {
        while self.pos < self.tokens.len() {
            if matches!(self.tokens[self.pos].token, Token::Newline) {
                self.pos += 1;
                break;
            }
            self.pos += 1;
        }
    }

    // ── Track Block Parsing ─────────────────────────────────────────

    /// Parse a `@track` block.
    ///
    /// Handles single-line and multi-line forms. Parses all parameters,
    /// then the `play:` or `steps:` content directive.
    pub fn parse_track_block(&mut self) -> CompileResult<TrackBlock> {
        let block_span_start = self.current_span().start;
        self.expect(&Token::AtTrack)?;

        let name = self.expect_ident("track name")?;

        // Parse parameters (may span multiple lines for multi-line form)
        let mut channel: Option<u8> = None;
        let mut program: Option<u8> = None;
        let mut unit: Option<(u32, u32)> = None;
        let mut octave = 4u8;
        let mut velocity = 84u8;
        let mut gate = 0.9f64;
        let mut shift: Option<TimingValue> = None;
        let mut lshift: Option<TimingValue> = None;
        let mut follow: Option<String> = None;
        let mut voice = VoicingStrategy::Close;
        let mut inv = Inversion::default();
        let mut seed: Option<u64> = None;
        let mut is_drum = false;
        let mut drummap: Option<String> = None;
        let mut mode: Option<String> = None;
        let mut rate: Option<f64> = None;
        let mut swing: Option<f64> = None;
        let mut swing_unit: Option<(u32, u32)> = None;
        let mut start: Option<u32> = None;
        let mut content: Option<TrackContent> = None;
        // Duplicate-parameter detection spans the whole block (params may
        // continue on subsequent lines in the multi-line form).
        let mut seen_params = std::collections::HashSet::new();

        // Parse the declaration line parameters
        while !matches!(self.peek(), Some(Token::Newline) | None) {
            self.skip_comments();
            if matches!(self.peek(), Some(Token::Newline) | None) {
                break;
            }
            if !self.parse_track_param(
                &name,
                &mut seen_params,
                &mut channel,
                &mut program,
                &mut unit,
                &mut octave,
                &mut velocity,
                &mut gate,
                &mut shift,
                &mut lshift,
                &mut follow,
                &mut voice,
                &mut inv,
                &mut seed,
                &mut is_drum,
                &mut drummap,
                &mut mode,
                &mut rate,
                &mut swing,
                &mut swing_unit,
                &mut start,
                &mut content,
            )? {
                break;
            }
        }

        // Skip to body
        self.skip_newlines();

        // Multi-line: parse additional parameter lines and content
        while !self.at_block_boundary_or_end() {
            self.skip_newlines();
            if self.at_block_boundary_or_end() {
                break;
            }

            // Check for play: directive
            if matches!(self.peek(), Some(Token::KwPlay)) {
                // Peek ahead to distinguish play=true/false from play: expression
                if self.peek_ahead_is_colon() {
                    if content.is_some() {
                        return Err(CompileError::PlayAndSteps {
                            name: name.clone(),
                            span: self.current_span(),
                        });
                    }
                    self.advance(); // consume play
                    self.expect(&Token::Colon)?;
                    let expr = self.parse_pattern_expr()?;
                    content = Some(TrackContent::Play(expr));
                    self.skip_to_newline();
                    continue;
                }
            }

            // Check for steps: directive
            if matches!(self.peek(), Some(Token::KwSteps)) && self.peek_ahead_is_colon() {
                if content.is_some() {
                    return Err(CompileError::PlayAndSteps {
                        name: name.clone(),
                        span: self.current_span(),
                    });
                }
                self.advance(); // consume steps
                self.expect(&Token::Colon)?;
                self.skip_newlines();
                let lines = self.parse_inline_steps()?;
                content = Some(TrackContent::Steps(lines));
                continue;
            }

            // Try to parse a parameter line
            if !self.parse_track_param(
                &name,
                &mut seen_params,
                &mut channel,
                &mut program,
                &mut unit,
                &mut octave,
                &mut velocity,
                &mut gate,
                &mut shift,
                &mut lshift,
                &mut follow,
                &mut voice,
                &mut inv,
                &mut seed,
                &mut is_drum,
                &mut drummap,
                &mut mode,
                &mut rate,
                &mut swing,
                &mut swing_unit,
                &mut start,
                &mut content,
            )? {
                break;
            }

            // Consume newline at end of parameter line
            if matches!(self.peek(), Some(Token::Newline)) {
                self.advance();
            }
        }

        // Validate required fields
        let channel = channel.ok_or_else(|| CompileError::ParseError {
            message: format!("track '{name}': ch= is required"),
            span: self.current_span(),
        })?;

        let content = content.ok_or_else(|| CompileError::NeitherPlayNorSteps {
            name: name.clone(),
            span: self.current_span(),
        })?;

        // Drum tracks cannot follow a harmony block
        if is_drum && follow.is_some() {
            return Err(CompileError::DrumTrackWithFollow {
                name: name.clone(),
                span: self.current_span(),
            });
        }

        let block_span_end = self.prev_span().end;
        Ok(TrackBlock {
            name,
            channel,
            program,
            unit,
            octave,
            velocity,
            gate,
            shift,
            lshift,
            follow,
            voice,
            inv,
            seed,
            mode,
            rate,
            swing,
            swing_unit,
            start,
            is_drum,
            drummap,
            content,
            span: Some(Span::new(block_span_start, block_span_end)),
        })
    }

    /// Parse a single track parameter. Returns Ok(true) if a parameter was consumed,
    /// Ok(false) if the current token isn't a track parameter (caller should stop).
    #[allow(clippy::too_many_arguments)]
    fn parse_track_param(
        &mut self,
        name: &str,
        seen_params: &mut std::collections::HashSet<&'static str>,
        channel: &mut Option<u8>,
        program: &mut Option<u8>,
        unit: &mut Option<(u32, u32)>,
        octave: &mut u8,
        velocity: &mut u8,
        gate: &mut f64,
        shift: &mut Option<TimingValue>,
        lshift: &mut Option<TimingValue>,
        follow: &mut Option<String>,
        voice: &mut VoicingStrategy,
        inv: &mut Inversion,
        seed: &mut Option<u64>,
        is_drum: &mut bool,
        drummap: &mut Option<String>,
        mode: &mut Option<String>,
        rate: &mut Option<f64>,
        swing: &mut Option<f64>,
        swing_unit: &mut Option<(u32, u32)>,
        start: &mut Option<u32>,
        _content: &mut Option<TrackContent>,
    ) -> CompileResult<bool> {
        self.skip_comments();
        match self.peek() {
            Some(Token::KwCh) => {
                check_duplicate_param(seen_params, "ch", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                let ch_val = self.expect_positive_integer("ch")?;
                if !(1..=16).contains(&ch_val) {
                    return Err(CompileError::ChannelOutOfRange {
                        name: name.to_string(),
                        span: self.current_span(),
                    });
                }
                *channel = Some(ch_val as u8);
                Ok(true)
            }
            Some(Token::KwProg) => {
                check_duplicate_param(seen_params, "prog", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                // prog can be integer or GM name string
                if matches!(self.peek(), Some(Token::Integer(_))) {
                    *program = Some(self.expect_midi_u7("prog")?);
                } else {
                    // GM name: consume identifier(s) joined by underscores
                    let prog_name = self.expect_ident("program name")?;
                    *program = Some(gm_program_by_name(&prog_name).ok_or_else(|| {
                        CompileError::ParseError {
                            message: format!("unknown GM program name: '{prog_name}'"),
                            span: self.current_span(),
                        }
                    })?);
                }
                Ok(true)
            }
            Some(Token::KwUnit) => {
                check_duplicate_param(seen_params, "unit", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *unit = Some(self.expect_fraction("unit")?);
                Ok(true)
            }
            Some(Token::KwOct) => {
                check_duplicate_param(seen_params, "oct", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *octave = self.expect_octave("oct")?;
                Ok(true)
            }
            Some(Token::KwVel) => {
                check_duplicate_param(seen_params, "vel", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                let vel_val = self.expect_positive_integer("vel")?;
                if vel_val > 127 {
                    return Err(CompileError::VelocityOutOfRange {
                        context: format!("track '{name}'"),
                        span: self.current_span(),
                    });
                }
                *velocity = vel_val as u8;
                Ok(true)
            }
            Some(Token::KwGate) => {
                check_duplicate_param(seen_params, "gate", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                let gate_val = self.expect_positive_number("gate")?;
                if !(0.0..=1.0).contains(&gate_val) {
                    return Err(CompileError::GateOutOfRange {
                        context: format!("track '{name}'"),
                        span: self.current_span(),
                    });
                }
                *gate = gate_val;
                Ok(true)
            }
            Some(Token::KwShift) => {
                check_duplicate_param(seen_params, "shift", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *shift = Some(self.parse_timing_value()?);
                Ok(true)
            }
            Some(Token::KwLshift) => {
                check_duplicate_param(seen_params, "lshift", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *lshift = Some(self.parse_timing_value()?);
                Ok(true)
            }
            Some(Token::KwFollow) => {
                check_duplicate_param(seen_params, "follow", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *follow = Some(self.expect_ident("follow harmony name")?);
                Ok(true)
            }
            Some(Token::KwVoice) => {
                check_duplicate_param(seen_params, "voice", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *voice = self.expect_voicing()?;
                Ok(true)
            }
            Some(Token::KwInv) => {
                check_duplicate_param(seen_params, "inv", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *inv = self.expect_inversion()?;
                Ok(true)
            }
            Some(Token::KwSeed) => {
                check_duplicate_param(seen_params, "seed", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *seed = Some(self.expect_non_negative_integer("seed")? as u64);
                Ok(true)
            }
            Some(Token::KwType) => {
                check_duplicate_param(seen_params, "type", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                // "drums" is a keyword token, so check for it directly
                if matches!(self.peek(), Some(Token::KwDrums)) {
                    self.advance();
                    *is_drum = true;
                } else {
                    let type_name = self.expect_ident("track type")?;
                    return Err(CompileError::ParseError {
                        message: format!("unknown track type '{type_name}', expected 'drums'"),
                        span: self.current_span(),
                    });
                }
                Ok(true)
            }
            Some(Token::KwDrummap) => {
                check_duplicate_param(seen_params, "drummap", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *drummap = Some(self.expect_ident("drummap name")?);
                Ok(true)
            }
            Some(Token::KwMode) => {
                check_duplicate_param(seen_params, "mode", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                let mode_name = self.expect_ident("mode value")?;
                if crate::harmony::lookup_mode(&mode_name).is_none() {
                    return Err(CompileError::ParseError {
                        message: format!("unknown mode '{mode_name}'"),
                        span: self.current_span(),
                    });
                }
                *mode = Some(mode_name);
                Ok(true)
            }
            Some(Token::KwRate) => {
                check_duplicate_param(seen_params, "rate", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *rate = Some(self.expect_positive_number("rate")?);
                Ok(true)
            }
            Some(Token::KwSwing) => {
                check_duplicate_param(seen_params, "swing", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *swing = Some(self.expect_positive_number("swing ratio")?);
                Ok(true)
            }
            Some(Token::KwSwingUnit) => {
                check_duplicate_param(seen_params, "swingunit", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                *swing_unit = Some(self.expect_fraction("swingunit")?);
                Ok(true)
            }
            Some(Token::KwStart) => {
                check_duplicate_param(seen_params, "start", "@track", self.current_span())?;
                self.advance();
                self.expect(&Token::Equals)?;
                let bar = self.expect_positive_u32("start bar")?;
                if bar == 0 {
                    return Err(CompileError::ParseError {
                        message: format!(
                            "track '{name}': start= must be >= 1 (1-indexed bar number)"
                        ),
                        span: self.current_span(),
                    });
                }
                *start = Some(bar);
                Ok(true)
            }
            Some(Token::Comment) => {
                self.advance();
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Check if the token after the current one is a colon (for play: / steps: detection).
    fn peek_ahead_is_colon(&self) -> bool {
        if self.pos + 1 < self.tokens.len() {
            matches!(self.tokens[self.pos + 1].token, Token::Colon)
        } else {
            false
        }
    }

    /// Parse inline step lines for a `steps:` block in a track.
    fn parse_inline_steps(&mut self) -> CompileResult<Vec<StepLine>> {
        let mut lines = Vec::new();
        while !self.at_block_boundary_or_end() {
            self.skip_newlines();
            if self.at_block_boundary_or_end() {
                break;
            }
            if self.is_step_line_start() {
                let line = self.parse_step_line()?;
                lines.push(line);
            } else {
                break;
            }
            if matches!(self.peek(), Some(Token::Newline)) {
                self.advance();
            }
        }
        Ok(lines)
    }

    /// Expect and consume an inversion value (0-3 or auto).
    fn expect_inversion(&mut self) -> CompileResult<Inversion> {
        self.skip_comments();
        match self.peek() {
            Some(Token::Integer(n)) => {
                let n = *n;
                self.advance();
                if !(0..=3).contains(&n) {
                    return Err(CompileError::ParseError {
                        message: format!("inv must be 0-3 or auto, got {n}"),
                        span: self.current_span(),
                    });
                }
                Ok(Inversion::Fixed(n as u8))
            }
            Some(Token::KwAuto) => {
                self.advance();
                Ok(Inversion::Auto)
            }
            _ => Err(CompileError::ParseError {
                message: "expected inversion value (0-3 or auto)".to_string(),
                span: self.current_span(),
            }),
        }
    }

    // ── Drummap Block Parsing ────────────────────────────────────────

    /// Parse a `@drummap` block.
    ///
    /// Expects the parser to be positioned at `@drummap`. Parses the optional
    /// name, then mapping lines `identifier = midi_note` until the next block
    /// or end of input.
    pub fn parse_drummap_block(&mut self) -> CompileResult<DrumMapBlock> {
        let block_span_start = self.current_span().start;
        self.expect(&Token::AtDrummap)?;

        // Optional name — if the next token is an identifier (not newline/EOF), consume it
        let name = if matches!(self.peek(), Some(Token::Ident(_))) {
            Some(self.expect_ident("drummap name")?)
        } else {
            None
        };

        self.skip_to_newline();
        self.skip_newlines();

        let mut mappings: Vec<(String, u8)> = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        while !self.at_block_boundary_or_end() {
            self.skip_newlines();
            if self.at_block_boundary_or_end() {
                break;
            }

            // Skip comments
            if matches!(self.peek(), Some(Token::Comment)) {
                self.advance();
                if matches!(self.peek(), Some(Token::Newline)) {
                    self.advance();
                }
                continue;
            }

            // Parse mapping: identifier = integer
            let hit_name = self.expect_ident("drum hit name")?;

            if !seen_names.insert(hit_name.clone()) {
                return Err(CompileError::ParseError {
                    message: format!("drummap: duplicate mapping '{hit_name}'"),
                    span: self.current_span(),
                });
            }

            self.expect(&Token::Equals)?;

            // Range-check on the wide type BEFORE narrowing: `kick=300`
            // must error, not truncate to 44.
            let note = self.expect_midi_u7("drummap MIDI note")?;

            mappings.push((hit_name, note));

            // Consume trailing comments and newline
            if matches!(self.peek(), Some(Token::Comment)) {
                self.advance();
            }
            if matches!(self.peek(), Some(Token::Newline)) {
                self.advance();
            }
        }

        let block_span_end = self.prev_span().end;
        Ok(DrumMapBlock {
            name,
            mappings,
            span: Some(Span::new(block_span_start, block_span_end)),
        })
    }

    // ── Helper methods ───────────────────────────────────────────────

    /// Expect and consume an identifier.
    fn expect_ident(&mut self, context: &str) -> CompileResult<String> {
        self.skip_comments();
        if self.pos >= self.tokens.len() {
            return Err(CompileError::ParseError {
                message: format!("{context}: expected identifier"),
                span: self.eof_span(),
            });
        }
        let tok = &self.tokens[self.pos];
        if let Token::Ident(s) = &tok.token {
            let s = s.clone();
            self.pos += 1;
            Ok(s)
        } else {
            Err(CompileError::ParseError {
                message: format!("{context}: expected identifier, found {:?}", tok.token),
                span: Span::new(tok.start, tok.end),
            })
        }
    }

    /// Expect and consume a boolean value (`true` or `false`).
    fn expect_bool(&mut self) -> CompileResult<bool> {
        self.skip_comments();
        match self.peek() {
            Some(Token::KwTrue) => {
                self.advance();
                Ok(true)
            }
            Some(Token::KwFalse) => {
                self.advance();
                Ok(false)
            }
            _ => Err(CompileError::ParseError {
                message: "expected true or false".to_string(),
                span: self.current_span(),
            }),
        }
    }

    /// Expect and consume a voicing strategy identifier.
    fn expect_voicing(&mut self) -> CompileResult<VoicingStrategy> {
        let name = self.expect_ident("voicing strategy")?;
        match name.as_str() {
            "close" => Ok(VoicingStrategy::Close),
            "open" => Ok(VoicingStrategy::Open),
            "drop2" => Ok(VoicingStrategy::Drop2),
            "shell" => Ok(VoicingStrategy::Shell),
            "triad" => Ok(VoicingStrategy::Triad),
            "drop3" => Ok(VoicingStrategy::Drop3),
            "rootless" => Ok(VoicingStrategy::Rootless),
            _ => Err(CompileError::ParseError {
                message: format!(
                    "unknown voicing strategy '{name}'. \
                     Valid options: close, open, drop2, drop3, shell, triad, rootless"
                ),
                span: self.current_span(),
            }),
        }
    }

    /// Expect and consume a fraction `numerator/denominator`.
    ///
    /// Both parts are range-checked on the wide type before narrowing so
    /// that e.g. `unit=1/4294967296` errors instead of truncating the
    /// denominator to 0 (divide-by-zero downstream).
    fn expect_fraction(&mut self, context: &str) -> CompileResult<(u32, u32)> {
        let num = self.expect_positive_u32(context)?;
        self.expect(&Token::Slash)?;
        let denom = self.expect_positive_u32(context)?;
        Ok((num, denom))
    }

    /// Expect and consume a fraction or plain integer. Returns (num, denom).
    fn expect_fraction_or_int(&mut self, context: &str) -> CompileResult<(u32, u32)> {
        let num = self.expect_positive_u32(context)?;
        if matches!(self.peek(), Some(Token::Slash)) {
            self.advance();
            let denom = self.expect_positive_u32(context)?;
            Ok((num, denom))
        } else {
            Ok((num, 1))
        }
    }

    /// Expect a degree number (1-13).
    fn expect_degree_number(&mut self) -> CompileResult<u8> {
        match self.peek() {
            Some(Token::Integer(n)) => {
                let n = *n;
                self.advance();
                if !(1..=13).contains(&n) {
                    return Err(CompileError::ParseError {
                        message: format!("scale degree must be 1-13, got {n}"),
                        span: self.current_span(),
                    });
                }
                Ok(n as u8)
            }
            _ => Err(CompileError::ParseError {
                message: "expected degree number after ^".to_string(),
                span: self.current_span(),
            }),
        }
    }

    /// Expect and consume an integer (may be negative).
    fn expect_integer(&mut self, context: &str) -> CompileResult<i64> {
        self.skip_comments();
        if self.pos >= self.tokens.len() {
            return Err(CompileError::ParseError {
                message: format!("{context}: expected integer"),
                span: self.eof_span(),
            });
        }
        let tok = &self.tokens[self.pos];
        if let Token::Integer(val) = &tok.token {
            let val = *val;
            self.pos += 1;
            Ok(val)
        } else {
            Err(CompileError::ParseError {
                message: format!("{context}: expected integer, found {:?}", tok.token),
                span: Span::new(tok.start, tok.end),
            })
        }
    }
}

/// Record a block-declaration parameter in `seen`, erroring on a duplicate.
///
/// The global header already rejects duplicate directives; block headers
/// (`@track`, `@pattern`, `@harmony`) apply the same rule via this helper
/// instead of silently keeping the last value.
fn check_duplicate_param(
    seen: &mut std::collections::HashSet<&'static str>,
    key: &'static str,
    block: &str,
    span: Span,
) -> CompileResult<()> {
    if !seen.insert(key) {
        return Err(CompileError::ParseError {
            message: format!("duplicate parameter '{key}' in {block} declaration"),
            span,
        });
    }
    Ok(())
}

/// Try to parse an absolute pitch string like "C4", "D#5", "Bb3" into a MIDI note number.
/// Returns `Some((midi_note, consumed_len))` on success, `None` if the string
/// doesn't match the pattern `<letter>[#|b]<octave>`.
fn try_parse_absolute_pitch(s: &str) -> Option<(u8, usize)> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    // First character must be a note letter A-G (uppercase)
    let base_semitone = match bytes[0] {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };

    let mut pos = 1;

    // Optional accidental: # or b
    let accidental: i8 = if pos < bytes.len() {
        match bytes[pos] {
            b'#' => {
                pos += 1;
                1
            }
            b'b' => {
                pos += 1;
                -1
            }
            _ => 0,
        }
    } else {
        return None; // need at least an octave digit
    };

    // Octave: one or two digits (we accept -1 through 9, but ident won't have negative)
    if pos >= bytes.len() || !bytes[pos].is_ascii_digit() {
        return None;
    }

    let octave_start = pos;
    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
        pos += 1;
    }

    let octave_str = &s[octave_start..pos];
    let octave: i8 = octave_str.parse().ok()?;

    // MIDI note = (octave + 1) * 12 + base_semitone + accidental
    // Using octave convention where C4 = 60
    let midi: i16 = (octave as i16 + 1) * 12 + base_semitone as i16 + accidental as i16;

    if !(0..=127).contains(&midi) {
        return None;
    }

    // Ensure we consumed the entire string (no trailing characters)
    if pos != bytes.len() {
        return None;
    }

    Some((midi as u8, pos))
}

/// Map a GM instrument name (snake_case) to its MIDI program number (0-127).
/// Returns `None` if the name is not recognized.
fn gm_program_by_name(name: &str) -> Option<u8> {
    match name {
        "acoustic_grand_piano" | "piano" => Some(0),
        "bright_acoustic_piano" => Some(1),
        "electric_grand_piano" => Some(2),
        "honky_tonk_piano" => Some(3),
        "electric_piano_1" | "rhodes" => Some(4),
        "electric_piano_2" => Some(5),
        "harpsichord" => Some(6),
        "clavinet" => Some(7),
        "celesta" => Some(8),
        "glockenspiel" => Some(9),
        "music_box" => Some(10),
        "vibraphone" => Some(11),
        "marimba" => Some(12),
        "xylophone" => Some(13),
        "tubular_bells" => Some(14),
        "dulcimer" => Some(15),
        "drawbar_organ" => Some(16),
        "percussive_organ" => Some(17),
        "rock_organ" => Some(18),
        "church_organ" => Some(19),
        "reed_organ" => Some(20),
        "accordion" => Some(21),
        "harmonica" => Some(22),
        "tango_accordion" => Some(23),
        "acoustic_guitar_nylon" | "nylon_guitar" => Some(24),
        "acoustic_guitar_steel" | "steel_guitar" => Some(25),
        "electric_guitar_jazz" => Some(26),
        "electric_guitar_clean" => Some(27),
        "electric_guitar_muted" => Some(28),
        "overdriven_guitar" => Some(29),
        "distortion_guitar" => Some(30),
        "guitar_harmonics" => Some(31),
        "acoustic_bass" => Some(32),
        "electric_bass_finger" | "finger_bass" => Some(33),
        "electric_bass_pick" | "pick_bass" => Some(34),
        "fretless_bass" => Some(35),
        "slap_bass_1" | "slap_bass" => Some(36),
        "slap_bass_2" => Some(37),
        "synth_bass_1" | "synth_bass" => Some(38),
        "synth_bass_2" => Some(39),
        "violin" => Some(40),
        "viola" => Some(41),
        "cello" => Some(42),
        "contrabass" => Some(43),
        "tremolo_strings" => Some(44),
        "pizzicato_strings" => Some(45),
        "orchestral_harp" | "harp" => Some(46),
        "timpani" => Some(47),
        "string_ensemble_1" | "strings" => Some(48),
        "string_ensemble_2" => Some(49),
        "synth_strings_1" | "synth_strings" => Some(50),
        "synth_strings_2" => Some(51),
        "choir_aahs" | "choir" => Some(52),
        "voice_oohs" => Some(53),
        "synth_voice" => Some(54),
        "orchestra_hit" => Some(55),
        "trumpet" => Some(56),
        "trombone" => Some(57),
        "tuba" => Some(58),
        "muted_trumpet" => Some(59),
        "french_horn" => Some(60),
        "brass_section" | "brass" => Some(61),
        "synth_brass_1" | "synth_brass" => Some(62),
        "synth_brass_2" => Some(63),
        "soprano_sax" => Some(64),
        "alto_sax" => Some(65),
        "tenor_sax" => Some(66),
        "baritone_sax" => Some(67),
        "oboe" => Some(68),
        "english_horn" => Some(69),
        "bassoon" => Some(70),
        "clarinet" => Some(71),
        "piccolo" => Some(72),
        "flute" => Some(73),
        "recorder" => Some(74),
        "pan_flute" => Some(75),
        "blown_bottle" => Some(76),
        "shakuhachi" => Some(77),
        "whistle" => Some(78),
        "ocarina" => Some(79),
        "lead_1_square" | "square_lead" => Some(80),
        "lead_2_sawtooth" | "saw_lead" => Some(81),
        "lead_3_calliope" => Some(82),
        "lead_4_chiff" => Some(83),
        "lead_5_charang" => Some(84),
        "lead_6_voice" => Some(85),
        "lead_7_fifths" => Some(86),
        "lead_8_bass_lead" => Some(87),
        "pad_1_new_age" | "new_age_pad" => Some(88),
        "pad_2_warm" | "warm_pad" => Some(89),
        "pad_3_polysynth" => Some(90),
        "pad_4_choir" => Some(91),
        "pad_5_bowed" => Some(92),
        "pad_6_metallic" => Some(93),
        "pad_7_halo" => Some(94),
        "pad_8_sweep" => Some(95),
        "fx_1_rain" => Some(96),
        "fx_2_soundtrack" => Some(97),
        "fx_3_crystal" => Some(98),
        "fx_4_atmosphere" => Some(99),
        "fx_5_brightness" => Some(100),
        "fx_6_goblins" => Some(101),
        "fx_7_echoes" => Some(102),
        "fx_8_sci_fi" => Some(103),
        "sitar" => Some(104),
        "banjo" => Some(105),
        "shamisen" => Some(106),
        "koto" => Some(107),
        "kalimba" => Some(108),
        "bag_pipe" => Some(109),
        "fiddle" => Some(110),
        "shanai" => Some(111),
        "tinkle_bell" => Some(112),
        "agogo" => Some(113),
        "steel_drums" => Some(114),
        "woodblock" => Some(115),
        "taiko_drum" => Some(116),
        "melodic_tom" => Some(117),
        "synth_drum" => Some(118),
        "reverse_cymbal" => Some(119),
        "guitar_fret_noise" => Some(120),
        "breath_noise" => Some(121),
        "seashore" => Some(122),
        "bird_tweet" => Some(123),
        "telephone_ring" => Some(124),
        "helicopter" => Some(125),
        "applause" => Some(126),
        "gunshot" => Some(127),
        _ => None,
    }
}

/// Return the GM default drum map mappings.
///
/// These 17 names are built-in and always available on channel 10
/// when no `@drummap` is declared.
pub fn gm_default_drummap() -> Vec<(String, u8)> {
    vec![
        ("kick".into(), 36),
        ("snare".into(), 38),
        ("clap".into(), 39),
        ("snare_rim".into(), 40),
        ("tom_lo".into(), 41),
        ("hh".into(), 42),
        ("tom_mid".into(), 43),
        ("ohh".into(), 46),
        ("tom_hi".into(), 48),
        ("crash".into(), 49),
        ("ride".into(), 51),
        ("ride_bell".into(), 53),
        ("cowbell".into(), 56),
        ("bongo_hi".into(), 60),
        ("bongo_lo".into(), 61),
        ("conga_hi".into(), 62),
        ("conga_lo".into(), 63),
    ]
}

/// Parse only the global header from a token stream.
///
/// This is the pass 1 entry point. Returns the header with defaults applied
/// for any missing directives.
pub fn parse_header(tokens: Vec<SpannedToken>) -> CompileResult<(GlobalHeader, Parser)> {
    let mut parser = Parser::new(tokens);
    let header = parser.parse_header()?;
    Ok((header, parser))
}

/// Parse a complete Interval source string into a `Program` AST.
///
/// Runs both passes (header extraction + block parsing) and returns the
/// typed AST without compiling. Useful for IDE integration, syntax
/// highlighting, and introspection APIs.
///
/// Note: `resolved_seed` is left as `None` since seed resolution requires
/// OS APIs not available in WASM. The caller can set it before compilation.
pub fn parse_only(source: &str) -> CompileResult<crate::ast::Program> {
    use crate::ast::Block;
    use crate::lexer::tokenize;

    let (tokens, lex_errors) = tokenize(source);
    if !lex_errors.is_empty() {
        return Err(CompileError::ParseError {
            message: format!("lexer errors: {lex_errors:?}"),
            span: Span::new(0, 0),
        });
    }

    let (mut header, mut p) = parse_header(tokens)?;

    let mut blocks = Vec::new();
    while p.has_tokens() {
        p.skip_newlines_pub();
        if !p.has_tokens() {
            break;
        }
        if p.peek_is_scale() {
            blocks.push(Block::Scale(p.parse_scale_block()?));
            // Header directives (@bpm, @ts, @ppq, …) may appear after a
            // scalar @scale — the header region is order-free. Resume
            // header parsing; it stops at the next non-header token.
            p.parse_header_directives(&mut header)?;
        } else if p.peek_is_harmony() {
            blocks.push(Block::Harmony(p.parse_harmony_block()?));
        } else if p.peek_is_pattern() {
            blocks.push(Block::Pattern(p.parse_pattern_block()?));
        } else if p.peek_is_track() {
            blocks.push(Block::Track(p.parse_track_block()?));
        } else if p.peek_is_drummap() {
            blocks.push(Block::DrumMap(p.parse_drummap_block()?));
        } else if p.peek_is_tempo() {
            // @tempo is a hard error everywhere in v0.5+, not just in the
            // header region — use @bpm block or inline form instead.
            return Err(CompileError::DeprecatedTempo {
                span: p.current_span(),
            });
        } else {
            return Err(p.error_unexpected_block());
        }
        p.skip_newlines_pub();
    }

    let source_len = source.len();
    Ok(crate::ast::Program {
        header,
        blocks,
        span: Some(Span::new(0, source_len)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Annotation, CcValue, PatternBody, PatternExpr, TransformCall};
    use crate::lexer::tokenize;

    fn parse_header_from_source(source: &str) -> CompileResult<GlobalHeader> {
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "lexer errors: {errors:?}");
        let (header, _parser) = parse_header(tokens)?;
        Ok(header)
    }

    #[test]
    fn test_empty_header_defaults() {
        let header = parse_header_from_source("").unwrap();
        assert_eq!(header.ppq, 480);
        assert_eq!(header.bpm, 120.0);
        assert_eq!(header.ts_numerator, 4);
        assert_eq!(header.ts_denominator, 4);
        assert!(header.title.is_none());
        assert_eq!(header.seed, None);
    }

    #[test]
    fn test_full_header() {
        let source = r#"@ppq 960
@bpm 138.0
@ts 3/4
@title "My Song"
@seed 42"#;
        let header = parse_header_from_source(source).unwrap();
        assert_eq!(header.ppq, 960);
        assert_eq!(header.bpm, 138.0);
        assert_eq!(header.ts_numerator, 3);
        assert_eq!(header.ts_denominator, 4);
        assert_eq!(header.title.as_deref(), Some("My Song"));
        assert_eq!(header.seed, Some(42));
    }

    #[test]
    fn test_partial_header() {
        let source = "@ppq 240\n@bpm 92.5";
        let header = parse_header_from_source(source).unwrap();
        assert_eq!(header.ppq, 240);
        assert_eq!(header.bpm, 92.5);
        // defaults
        assert_eq!(header.ts_numerator, 4);
        assert_eq!(header.ts_denominator, 4);
        assert!(header.title.is_none());
        assert_eq!(header.seed, None);
    }

    #[test]
    fn test_header_stops_at_block() {
        let source = "@ppq 480\n@bpm 120\n@harmony main mode=major\nCmaj7 | Am7";
        let header = parse_header_from_source(source).unwrap();
        assert_eq!(header.ppq, 480);
        assert_eq!(header.bpm, 120.0);
    }

    #[test]
    fn test_header_with_comments() {
        let source = "@ppq 480 // pulses per quarter note\n@bpm 138 // tempo";
        let header = parse_header_from_source(source).unwrap();
        assert_eq!(header.ppq, 480);
        assert_eq!(header.bpm, 138.0);
    }

    #[test]
    fn test_ppq_zero_error() {
        let source = "@ppq 0";
        let result = parse_header_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("positive integer"));
    }

    #[test]
    fn test_ppq_negative_error() {
        let source = "@ppq -10";
        let result = parse_header_from_source(source);
        assert!(result.is_err());
    }

    #[test]
    fn test_bpm_zero_error() {
        let source = "@bpm 0";
        let result = parse_header_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("positive"));
    }

    #[test]
    fn test_ts_bad_denominator() {
        let source = "@ts 4/3"; // 3 is not a power of 2
        let result = parse_header_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("power of 2"));
    }

    #[test]
    fn test_title_not_quoted() {
        let source = "@title hello";
        let result = parse_header_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("quoted string"));
    }

    #[test]
    fn test_duplicate_ppq() {
        let source = "@ppq 480\n@ppq 960";
        let result = parse_header_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn test_seed_zero_valid() {
        let source = "@seed 0";
        let header = parse_header_from_source(source).unwrap();
        assert_eq!(header.seed, Some(0));
    }

    #[test]
    fn test_bpm_integer_accepted() {
        // @bpm with integer (not float) should work
        let source = "@bpm 120";
        let header = parse_header_from_source(source).unwrap();
        assert_eq!(header.bpm, 120.0);
    }

    #[test]
    fn test_header_order_independent() {
        let source = "@seed 7\n@title \"Test\"\n@bpm 100\n@ts 6/8\n@ppq 96";
        let header = parse_header_from_source(source).unwrap();
        assert_eq!(header.seed, Some(7));
        assert_eq!(header.title.as_deref(), Some("Test"));
        assert_eq!(header.bpm, 100.0);
        assert_eq!(header.ts_numerator, 6);
        assert_eq!(header.ts_denominator, 8);
        assert_eq!(header.ppq, 96);
    }

    #[test]
    fn test_valid_time_signatures() {
        for (source, expected_num, expected_denom) in [
            ("@ts 4/4", 4, 4),
            ("@ts 3/4", 3, 4),
            ("@ts 6/8", 6, 8),
            ("@ts 2/2", 2, 2),
            ("@ts 5/4", 5, 4),
            ("@ts 7/8", 7, 8),
            ("@ts 12/16", 12, 16),
        ] {
            let header = parse_header_from_source(source).unwrap();
            assert_eq!(header.ts_numerator, expected_num, "failed for {source}");
            assert_eq!(header.ts_denominator, expected_denom, "failed for {source}");
        }
    }

    // ── Harmony block parsing tests ──────────────────────────────────

    fn parse_harmony_from_source(source: &str) -> CompileResult<HarmonyBlock> {
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "lexer errors: {errors:?}");
        let mut parser = Parser::new(tokens);
        // Skip header (if any)
        let _header = parser.parse_header()?;
        parser.skip_newlines();
        parser.parse_harmony_block()
    }

    #[test]
    fn test_harmony_simple() {
        let source = "@harmony main\nCmaj7 | Am7 | Dm7 | G7";
        let block = parse_harmony_from_source(source).unwrap();
        assert_eq!(block.name, Some("main".to_string()));
        assert_eq!(block.bars.len(), 4);
        assert_eq!(block.bars[0].chords.len(), 1);
        assert_eq!(block.bars[0].chords[0].chord.root, 0); // C
        assert_eq!(block.bars[1].chords[0].chord.root, 9); // A
        assert_eq!(block.bars[2].chords[0].chord.root, 2); // D
        assert_eq!(block.bars[3].chords[0].chord.root, 7); // G
    }

    #[test]
    fn test_harmony_multiline() {
        let source = "@harmony main\nCm7 | Fm7\nBb7 | Ebmaj7";
        let block = parse_harmony_from_source(source).unwrap();
        assert_eq!(block.bars.len(), 4);
        assert_eq!(block.bars[0].chords[0].chord.root, 0); // C
        assert_eq!(block.bars[1].chords[0].chord.root, 5); // F
        assert_eq!(block.bars[2].chords[0].chord.root, 10); // Bb
        assert_eq!(block.bars[3].chords[0].chord.root, 3); // Eb
    }

    #[test]
    fn test_harmony_multiple_chords_per_bar() {
        let source = "@harmony main\nDm7 G7 | Cmaj7";
        let block = parse_harmony_from_source(source).unwrap();
        assert_eq!(block.bars.len(), 2);
        assert_eq!(block.bars[0].chords.len(), 2); // Dm7 and G7
        assert_eq!(block.bars[1].chords.len(), 1); // Cmaj7
    }

    #[test]
    fn test_harmony_beat_assignment() {
        let source = "@harmony main\nDm7:3 G7:1 | Cmaj7";
        let block = parse_harmony_from_source(source).unwrap();
        assert_eq!(block.bars[0].chords[0].beats, Some(3));
        assert_eq!(block.bars[0].chords[1].beats, Some(1));
        assert_eq!(block.bars[1].chords[0].beats, None);
    }

    #[test]
    fn test_harmony_with_params() {
        let source = "@harmony comp play=true ch=3 voice=drop2 oct=5 vel=80\nCmaj7";
        let block = parse_harmony_from_source(source).unwrap();
        assert_eq!(block.name, Some("comp".to_string()));
        assert!(block.play);
        assert_eq!(block.channel, Some(3));
        assert_eq!(block.voice, VoicingStrategy::Drop2);
        assert_eq!(block.octave, 5);
        assert_eq!(block.velocity, 80);
    }

    #[test]
    fn test_harmony_steps_block() {
        let source = "@harmony main\nCmaj7\n  steps: Bbmaj7 Amaj7 Abmaj7 Gmaj7\nFmaj7";
        let block = parse_harmony_from_source(source).unwrap();
        assert_eq!(block.bars.len(), 2);
        // First bar should have steps: override
        assert!(block.bars[0].steps.is_some());
        let steps = block.bars[0].steps.as_ref().unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0].root, 10); // Bb
        assert_eq!(steps[1].root, 9); // A
        assert_eq!(steps[2].root, 8); // Ab
        assert_eq!(steps[3].root, 7); // G
                                      // Second bar has no steps
        assert!(block.bars[1].steps.is_none());
    }

    #[test]
    fn test_harmony_section_directive() {
        let source = "@harmony main\nCmaj7 | Am7\nsection: bar=3 mode=dorian root=D\nDm7 | Gm7";
        let block = parse_harmony_from_source(source).unwrap();
        assert_eq!(block.bars.len(), 4);
        assert_eq!(block.sections.len(), 1);
        assert_eq!(block.sections[0].bar, 3);
        assert_eq!(block.sections[0].mode, Some("dorian".to_string()));
        assert_eq!(block.sections[0].root, Some(2)); // D
    }

    #[test]
    fn test_harmony_with_comments() {
        let source = "@harmony main // primary timeline\nCmaj7 | Am7 // ii chord";
        let block = parse_harmony_from_source(source).unwrap();
        assert_eq!(block.bars.len(), 2);
    }

    #[test]
    fn test_harmony_stops_at_next_block() {
        let source = "@harmony main\nCmaj7\n@pattern test steps=4 unit=1/4";
        let block = parse_harmony_from_source(source).unwrap();
        assert_eq!(block.bars.len(), 1);
    }

    // ── Pattern Parser Tests ────────────────────────────────────────

    fn parse_pattern_from_source(source: &str) -> CompileResult<PatternBlock> {
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "lexer errors: {errors:?}");
        let mut parser = Parser::new(tokens);
        parser.parse_pattern_block()
    }

    #[test]
    fn test_pattern_simple_degrees() {
        let source = "@pattern bass steps=4 unit=1/4\n^1\n^3\n^5\n^1";
        let pat = parse_pattern_from_source(source).unwrap();
        assert_eq!(pat.name, "bass");
        assert_eq!(pat.steps, 4);
        assert_eq!(pat.unit, (1, 4));
        assert_eq!(pat.velocity, 84); // default
        assert_eq!(pat.gate, 0.9); // default
        assert_eq!(pat.octave, 4); // default
        if let PatternBody::Steps(lines) = &pat.body {
            assert_eq!(lines.len(), 4);
            assert!(matches!(
                &lines[0].tokens[0],
                StepToken::Degree {
                    degree: 1,
                    accidental: 0,
                    octave: None,
                    ..
                }
            ));
            assert!(matches!(
                &lines[2].tokens[0],
                StepToken::Degree {
                    degree: 5,
                    accidental: 0,
                    octave: None,
                    ..
                }
            ));
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_with_params() {
        let source = "@pattern melody steps=2 unit=1/8 vel=100 gate=0.8 oct=5\n^1\n^5";
        let pat = parse_pattern_from_source(source).unwrap();
        assert_eq!(pat.steps, 2);
        assert_eq!(pat.unit, (1, 8));
        assert_eq!(pat.velocity, 100);
        assert_eq!(pat.gate, 0.8);
        assert_eq!(pat.octave, 5);
    }

    #[test]
    fn test_pattern_rest_and_tie() {
        let source = "@pattern rests steps=4 unit=1/4\n^1\n.\n~\n^5";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            assert!(matches!(&lines[1].tokens[0], StepToken::Rest));
            assert!(matches!(&lines[2].tokens[0], StepToken::Tie));
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_degree_accidentals() {
        let source = "@pattern chromatic steps=2 unit=1/4\n^#1\n^b3";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            assert!(matches!(
                &lines[0].tokens[0],
                StepToken::Degree {
                    degree: 1,
                    accidental: 1,
                    ..
                }
            ));
            assert!(matches!(
                &lines[1].tokens[0],
                StepToken::Degree {
                    degree: 3,
                    accidental: -1,
                    ..
                }
            ));
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_degree_octave() {
        let source = "@pattern leap steps=1 unit=1/4\n^5/6";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            assert!(matches!(
                &lines[0].tokens[0],
                StepToken::Degree {
                    degree: 5,
                    octave: Some(6),
                    ..
                }
            ));
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_midi_number() {
        let source = "@pattern fixed steps=1 unit=1/4\nn60";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            assert!(matches!(
                &lines[0].tokens[0],
                StepToken::MidiNumber { note: 60, .. }
            ));
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_absolute_pitch() {
        let source = "@pattern abs steps=1 unit=1/4\nC4";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            assert!(matches!(
                &lines[0].tokens[0],
                StepToken::AbsolutePitch { midi_note: 60, .. }
            ));
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_simultaneous_notes() {
        let source = "@pattern chord steps=1 unit=1/4\n^1+^3+^5";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            assert_eq!(lines[0].tokens.len(), 3);
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_subdivision() {
        let source = "@pattern fast steps=1 unit=1/4\n(^1 ^2 ^3)";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            assert!(
                matches!(&lines[0].tokens[0], StepToken::Subdivision { tokens } if tokens.len() == 3)
            );
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_variant() {
        let source = "@pattern var steps=1 unit=1/4\n{^1, ^3, ^5}";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            assert!(
                matches!(&lines[0].tokens[0], StepToken::Variant { alternatives } if alternatives.len() == 3)
            );
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_annotation_vel() {
        let source = "@pattern ann steps=1 unit=1/4\n^1[vel:100]";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            if let StepToken::Degree { annotations, .. } = &lines[0].tokens[0] {
                assert_eq!(annotations.len(), 1);
                assert!(matches!(annotations[0], Annotation::Vel(100)));
            } else {
                panic!("expected Degree token");
            }
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_annotation_multiple() {
        let source = "@pattern ann steps=1 unit=1/4\n^1[vel:80 gate:0.5]";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            if let StepToken::Degree { annotations, .. } = &lines[0].tokens[0] {
                assert_eq!(annotations.len(), 2);
                assert!(matches!(annotations[0], Annotation::Vel(80)));
                if let Annotation::Gate(g) = annotations[1] {
                    assert!((g - 0.5).abs() < 0.001);
                } else {
                    panic!("expected Gate annotation");
                }
            } else {
                panic!("expected Degree token");
            }
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_cc_ramp() {
        let source = "@pattern cc_ramp steps=1 unit=1/4\n^1[expr:64->127]";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            if let StepToken::Degree { annotations, .. } = &lines[0].tokens[0] {
                assert!(matches!(
                    annotations[0],
                    Annotation::Expr(CcValue::Ramp {
                        start: 64,
                        end: 127
                    })
                ));
            } else {
                panic!("expected Degree token");
            }
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_assignment() {
        let source = "@pattern combined = intro >> verse";
        let pat = parse_pattern_from_source(source).unwrap();
        assert_eq!(pat.name, "combined");
        assert!(matches!(pat.body, PatternBody::Expression(_)));
    }

    #[test]
    fn test_pattern_expr_repeat() {
        let source = "@pattern rep = intro *4";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Expression(PatternExpr::Repeat { count, .. }) = &pat.body {
            assert_eq!(*count, 4);
        } else {
            panic!("expected Repeat expression, got {:?}", pat.body);
        }
    }

    #[test]
    fn test_pattern_expr_transform() {
        let source = "@pattern inverted = melody -> reverse";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Expression(PatternExpr::Transform { transform, .. }) = &pat.body {
            assert!(matches!(transform, TransformCall::Reverse));
        } else {
            panic!("expected Transform expression, got {:?}", pat.body);
        }
    }

    #[test]
    fn test_pattern_step_count_mismatch() {
        let source = "@pattern bad steps=4 unit=1/4\n^1\n^3";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("declared steps"));
    }

    #[test]
    fn test_absolute_pitch_c4() {
        assert_eq!(try_parse_absolute_pitch("C4"), Some((60, 2)));
    }

    #[test]
    fn test_absolute_pitch_sharps_flats() {
        assert_eq!(try_parse_absolute_pitch("C#4"), Some((61, 3)));
        assert_eq!(try_parse_absolute_pitch("Db4"), Some((61, 3)));
        assert_eq!(try_parse_absolute_pitch("A4"), Some((69, 2)));
        assert_eq!(try_parse_absolute_pitch("Bb3"), Some((58, 3)));
    }

    #[test]
    fn test_absolute_pitch_edge_cases() {
        assert_eq!(try_parse_absolute_pitch("C0"), Some((12, 2)));
        assert!(try_parse_absolute_pitch("").is_none());
        assert!(try_parse_absolute_pitch("X4").is_none());
        assert!(try_parse_absolute_pitch("C").is_none());
    }

    #[test]
    fn test_pattern_drum_hit() {
        let source = "@pattern mydrums steps=2 unit=1/4\nkick\nsnare";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            assert!(
                matches!(&lines[0].tokens[0], StepToken::DrumHit { name, .. } if name == "kick")
            );
            assert!(
                matches!(&lines[1].tokens[0], StepToken::DrumHit { name, .. } if name == "snare")
            );
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_pattern_transform_rotate() {
        let source = "@pattern rot = melody -> rotate(2)";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Expression(PatternExpr::Transform { transform, .. }) = &pat.body {
            assert!(matches!(transform, TransformCall::Rotate(2)));
        } else {
            panic!("expected Transform expression");
        }
    }

    #[test]
    fn test_pattern_transform_transpose() {
        let source = "@pattern trans = melody -> transpose(3)";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Expression(PatternExpr::Transform { transform, .. }) = &pat.body {
            assert!(matches!(transform, TransformCall::Transpose(3)));
        } else {
            panic!("expected Transform expression");
        }
    }

    // ── Track Parser Tests ─────────────────────────────────────────

    fn parse_track_from_source(source: &str) -> CompileResult<TrackBlock> {
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "lexer errors: {errors:?}");
        let mut parser = Parser::new(tokens);
        let _header = parser.parse_header()?;
        parser.skip_newlines();
        parser.parse_track_block()
    }

    #[test]
    fn test_track_simple_play() {
        let source = "@track bass ch=1 prog=32\n  play: motif * 4";
        let track = parse_track_from_source(source).unwrap();
        assert_eq!(track.name, "bass");
        assert_eq!(track.channel, 1);
        assert_eq!(track.program, Some(32));
        assert!(matches!(track.content, TrackContent::Play(_)));
    }

    #[test]
    fn test_track_defaults() {
        let source = "@track lead ch=2\n  play: melody";
        let track = parse_track_from_source(source).unwrap();
        assert_eq!(track.octave, 4);
        assert_eq!(track.velocity, 84);
        assert!((track.gate - 0.9).abs() < 0.001);
        assert_eq!(track.voice, VoicingStrategy::Close);
        assert_eq!(track.inv, Inversion::Fixed(0));
        assert!(!track.is_drum);
        assert!(track.seed.is_none());
        assert!(track.drummap.is_none());
        assert!(track.follow.is_none());
        assert!(track.shift.is_none());
        assert!(track.lshift.is_none());
    }

    #[test]
    fn test_track_all_params() {
        let source = "@track melody ch=3 prog=73 oct=5 vel=100 gate=0.8 voice=drop2 inv=auto seed=42 follow=main\n  play: theme";
        let track = parse_track_from_source(source).unwrap();
        assert_eq!(track.name, "melody");
        assert_eq!(track.channel, 3);
        assert_eq!(track.program, Some(73));
        assert_eq!(track.octave, 5);
        assert_eq!(track.velocity, 100);
        assert!((track.gate - 0.8).abs() < 0.001);
        assert_eq!(track.voice, VoicingStrategy::Drop2);
        assert_eq!(track.inv, Inversion::Auto);
        assert_eq!(track.seed, Some(42));
        assert_eq!(track.follow.as_deref(), Some("main"));
    }

    #[test]
    fn test_track_prog_by_name() {
        let source = "@track lead ch=1 prog=piano\n  play: melody";
        let track = parse_track_from_source(source).unwrap();
        assert_eq!(track.program, Some(0));
    }

    #[test]
    fn test_track_prog_by_name_flute() {
        let source = "@track wind ch=4 prog=flute\n  play: melody";
        let track = parse_track_from_source(source).unwrap();
        assert_eq!(track.program, Some(73));
    }

    #[test]
    fn test_track_unknown_prog_name() {
        let source = "@track bad ch=1 prog=kazoo\n  play: melody";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unknown GM program"));
    }

    #[test]
    fn test_track_drum_type() {
        let source = "@track perc ch=10 type=drums\n  play: beat";
        let track = parse_track_from_source(source).unwrap();
        assert!(track.is_drum);
    }

    #[test]
    fn test_track_drummap() {
        let source = "@track perc ch=10 type=drums drummap=kit1\n  play: beat";
        let track = parse_track_from_source(source).unwrap();
        assert!(track.is_drum);
        assert_eq!(track.drummap.as_deref(), Some("kit1"));
    }

    #[test]
    fn test_track_inline_steps() {
        let source = "@track bass ch=1\n  steps:\n  ^1\n  ^5";
        let track = parse_track_from_source(source).unwrap();
        assert_eq!(track.channel, 1);
        if let TrackContent::Steps(lines) = &track.content {
            assert_eq!(lines.len(), 2);
        } else {
            panic!("expected Steps content");
        }
    }

    #[test]
    fn test_track_missing_channel() {
        let source = "@track bad prog=0\n  play: melody";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ch="));
    }

    #[test]
    fn test_track_missing_content() {
        let source = "@track empty ch=1";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
    }

    #[test]
    fn test_track_inversion_values() {
        for (inv_str, expected) in [
            ("0", Inversion::Fixed(0)),
            ("1", Inversion::Fixed(1)),
            ("2", Inversion::Fixed(2)),
            ("3", Inversion::Fixed(3)),
            ("auto", Inversion::Auto),
        ] {
            let source = format!("@track t ch=1 inv={inv_str}\n  play: pat");
            let track = parse_track_from_source(&source).unwrap();
            assert_eq!(track.inv, expected, "failed for inv={inv_str}");
        }
    }

    #[test]
    fn test_track_inversion_out_of_range() {
        let source = "@track t ch=1 inv=5\n  play: pat";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("inv"));
    }

    #[test]
    fn test_track_voicing_strategies() {
        for (voice_str, expected) in [
            ("close", VoicingStrategy::Close),
            ("open", VoicingStrategy::Open),
            ("drop2", VoicingStrategy::Drop2),
            ("shell", VoicingStrategy::Shell),
            ("triad", VoicingStrategy::Triad),
            ("drop3", VoicingStrategy::Drop3),
            ("rootless", VoicingStrategy::Rootless),
        ] {
            let source = format!("@track t ch=1 voice={voice_str}\n  play: pat");
            let track = parse_track_from_source(&source).unwrap();
            assert_eq!(track.voice, expected, "failed for voice={voice_str}");
        }
    }

    #[test]
    fn test_track_play_expression() {
        let source = "@track bass ch=1\n  play: intro >> verse * 2 >> outro";
        let track = parse_track_from_source(source).unwrap();
        assert!(matches!(track.content, TrackContent::Play(_)));
    }

    #[test]
    fn test_gm_program_names() {
        assert_eq!(gm_program_by_name("piano"), Some(0));
        assert_eq!(gm_program_by_name("acoustic_grand_piano"), Some(0));
        assert_eq!(gm_program_by_name("strings"), Some(48));
        assert_eq!(gm_program_by_name("trumpet"), Some(56));
        assert_eq!(gm_program_by_name("flute"), Some(73));
        assert_eq!(gm_program_by_name("nonexistent"), None);
    }

    // ── Drummap Parser Tests ───────────────────────────────────────

    fn parse_drummap_from_source(source: &str) -> CompileResult<DrumMapBlock> {
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "lexer errors: {errors:?}");
        let mut parser = Parser::new(tokens);
        let _header = parser.parse_header()?;
        parser.skip_newlines();
        parser.parse_drummap_block()
    }

    #[test]
    fn test_drummap_named() {
        let source = "@drummap kit\n  kick = 36\n  snare = 38\n  hh = 42";
        let dm = parse_drummap_from_source(source).unwrap();
        assert_eq!(dm.name.as_deref(), Some("kit"));
        assert_eq!(dm.mappings.len(), 3);
        assert_eq!(dm.mappings[0], ("kick".into(), 36));
        assert_eq!(dm.mappings[1], ("snare".into(), 38));
        assert_eq!(dm.mappings[2], ("hh".into(), 42));
    }

    #[test]
    fn test_drummap_unnamed() {
        let source = "@drummap\n  kick = 36\n  snare = 38";
        let dm = parse_drummap_from_source(source).unwrap();
        assert!(dm.name.is_none());
        assert_eq!(dm.mappings.len(), 2);
    }

    #[test]
    fn test_drummap_with_comments() {
        let source = "@drummap kit // my kit\n  kick = 36 // bass drum\n  snare = 38";
        let dm = parse_drummap_from_source(source).unwrap();
        assert_eq!(dm.name.as_deref(), Some("kit"));
        assert_eq!(dm.mappings.len(), 2);
    }

    #[test]
    fn test_drummap_duplicate_name_error() {
        let source = "@drummap kit\n  kick = 36\n  kick = 37";
        let result = parse_drummap_from_source(source);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("duplicate"));
    }

    #[test]
    fn test_drummap_stops_at_next_block() {
        let source = "@drummap kit\n  kick = 36\n@pattern test steps=1 unit=1/4";
        let dm = parse_drummap_from_source(source).unwrap();
        assert_eq!(dm.mappings.len(), 1);
    }

    #[test]
    fn test_gm_default_drummap() {
        let dm = gm_default_drummap();
        assert_eq!(dm.len(), 17);
        assert!(dm.iter().any(|(name, note)| name == "kick" && *note == 36));
        assert!(dm.iter().any(|(name, note)| name == "snare" && *note == 38));
        assert!(dm.iter().any(|(name, note)| name == "hh" && *note == 42));
        assert!(dm.iter().any(|(name, note)| name == "ride" && *note == 51));
    }

    #[test]
    fn test_drum_track_with_follow_error() {
        let source = "@track perc ch=10 type=drums follow=main\n  play: beat";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("drum tracks cannot follow"));
    }

    #[test]
    fn test_drummap_empty() {
        let source = "@drummap kit";
        let dm = parse_drummap_from_source(source).unwrap();
        assert_eq!(dm.name.as_deref(), Some("kit"));
        assert!(dm.mappings.is_empty());
    }

    // ── Validation error tests ──────────────────────────────────────────

    #[test]
    fn test_channel_out_of_range_track() {
        let source = "@track lead ch=17\n  play: bass";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("ch must be 1-16"),
            "expected ch range error, got: {err}"
        );
    }

    #[test]
    fn test_channel_zero_out_of_range_track() {
        // ch=0 is rejected by expect_positive_integer before range check
        let source = "@track lead ch=0\n  play: bass";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
    }

    #[test]
    fn test_channel_valid_range() {
        let source = "@track lead ch=16\n  play: bass";
        let result = parse_track_from_source(source);
        assert!(result.is_ok());
    }

    #[test]
    fn test_velocity_out_of_range_track() {
        let source = "@track lead vel=128\n  play: bass";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("vel must be 1-127"),
            "expected vel range error, got: {err}"
        );
    }

    #[test]
    fn test_velocity_out_of_range_pattern() {
        let source = "@pattern bass steps=1 unit=1/4 vel=200\n^1";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("vel must be 1-127"),
            "expected vel range error, got: {err}"
        );
    }

    #[test]
    fn test_gate_out_of_range_track() {
        let source = "@track lead gate=1.5\n  play: bass";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("gate must be 0.0-1.0"),
            "expected gate range error, got: {err}"
        );
    }

    #[test]
    fn test_gate_negative_out_of_range_pattern() {
        let source = "@pattern bass steps=1 unit=1/4 gate=-0.1\n^1";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
    }

    // ── Untrusted input hardening tests ─────────────────────────────

    #[test]
    fn test_ts_denominator_too_large_for_u8() {
        // 256 is a power of 2 but truncates to 0 as u8 → must error, not
        // divide by zero downstream.
        let source = "@ts 4/256";
        let result = parse_header_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("power of 2"), "got: {err}");
    }

    #[test]
    fn test_ts_numerator_too_large_for_u8() {
        // 256 as u8 truncates to 0 → must error.
        let source = "@ts 256/4";
        let result = parse_header_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("1-255"), "got: {err}");
    }

    #[test]
    fn test_ppq_overflow_u32() {
        // 4294967296 as u32 truncates to 0 → must error.
        let source = "@ppq 4294967296";
        let result = parse_header_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn test_unit_fraction_denominator_overflow_u32() {
        // 4294967296 as u32 truncates to 0 → divide-by-zero downstream.
        let source = "@pattern p steps=1 unit=1/4294967296\n^1";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn test_deeply_nested_subdivision_error() {
        // A wall of `(` must produce a depth error, not a stack overflow.
        let source = format!(
            "@pattern p steps=1 unit=1/4\n{}^1{}",
            "(".repeat(1000),
            ")".repeat(1000)
        );
        let result = parse_pattern_from_source(&source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nesting too deep"), "got: {err}");
    }

    #[test]
    fn test_deeply_nested_expr_parens_error() {
        let source = format!("@pattern q = {}a{}", "(".repeat(1000), ")".repeat(1000));
        let result = parse_pattern_from_source(&source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nesting too deep"), "got: {err}");
    }

    #[test]
    fn test_moderate_nesting_still_parses() {
        // Well under the limit — must keep working.
        let source = format!(
            "@pattern p steps=1 unit=1/4\n{}^1{}",
            "(".repeat(10),
            ")".repeat(10)
        );
        assert!(parse_pattern_from_source(&source).is_ok());
    }

    #[test]
    fn test_long_concat_chain_no_stack_overflow() {
        // Right-recursion converted to a loop: a long `>>` chain parses fine.
        let mut expr = String::from("a");
        for _ in 0..5000 {
            expr.push_str(" >> a");
        }
        let source = format!("@pattern chain = {expr}");
        assert!(parse_pattern_from_source(&source).is_ok());
    }

    #[test]
    fn test_concat_right_associativity_preserved() {
        let source = "@pattern c = a >> b ~>> d";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Expression(PatternExpr::Concat { left, right }) = &pat.body {
            assert!(matches!(**left, PatternExpr::Ref { .. }));
            assert!(matches!(**right, PatternExpr::ConcatSoft { .. }));
        } else {
            panic!("expected right-associative Concat, got {:?}", pat.body);
        }
    }

    #[test]
    fn test_huge_repeat_count_error() {
        let source = "@track t ch=1\n  play: p * 4294967295";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exceeds maximum"), "got: {err}");
    }

    #[test]
    fn test_huge_bars_count_error() {
        let source = "@bars 4000000000";
        let result = parse_header_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exceeds maximum"), "got: {err}");
    }

    #[test]
    fn test_drummap_note_out_of_range_errors() {
        // 300 as u8 truncates to 44, which made the old >127 check
        // unreachable — the range check must run on the wide type.
        let source = "@drummap kit\n  kick = 300";
        let result = parse_drummap_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("0-127"), "got: {err}");
    }

    #[test]
    fn test_vel_annotation_out_of_range() {
        let source = "@pattern p steps=1 unit=1/4\n^1[vel:300]";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("vel must be 1-127"), "got: {err}");
    }

    #[test]
    fn test_harmony_vel_out_of_range() {
        let source = "@harmony main vel=200\nCmaj7";
        let result = parse_harmony_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("vel must be 1-127"), "got: {err}");
    }

    #[test]
    fn test_midi_number_out_of_range_errors() {
        // `n200` is unambiguously a MIDI number token — it must error
        // instead of silently compiling to a nonexistent drum hit.
        let source = "@pattern p steps=1 unit=1/4\nn200";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("0-127"), "got: {err}");
    }

    #[test]
    fn test_midi_number_huge_digits_errors() {
        let source = "@pattern p steps=1 unit=1/4\nn99999999999";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
    }

    #[test]
    fn test_non_numeric_n_prefix_still_drum_hit() {
        // `n` followed by non-digits stays a drum hit name (user-defined).
        let source = "@pattern p steps=1 unit=1/4\nnoise";
        let pat = parse_pattern_from_source(source).unwrap();
        if let PatternBody::Steps(lines) = &pat.body {
            assert!(
                matches!(&lines[0].tokens[0], StepToken::DrumHit { name, .. } if name == "noise")
            );
        } else {
            panic!("expected Steps body");
        }
    }

    #[test]
    fn test_cc_annotation_out_of_range() {
        let source = "@pattern p steps=1 unit=1/4\n^1[cc74:300]";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("0-127"), "got: {err}");
    }

    #[test]
    fn test_expr_annotation_negative_errors() {
        let source = "@pattern p steps=1 unit=1/4\n^1[expr:-5]";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("0-127"), "got: {err}");
    }

    #[test]
    fn test_pb_annotation_out_of_range() {
        // 40000 as i16 wraps negative — must be rejected on the wide type.
        let source = "@pattern p steps=1 unit=1/4\n^1[pb:40000]";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("-8192"), "got: {err}");
    }

    #[test]
    fn test_oct_param_out_of_range() {
        let source = "@track t ch=1 oct=300\n  play: p";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("0-10"), "got: {err}");
    }

    #[test]
    fn test_prog_param_out_of_range() {
        let source = "@track t ch=1 prog=300\n  play: p";
        let result = parse_track_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("0-127"), "got: {err}");
    }

    #[test]
    fn test_beat_assignment_out_of_range() {
        let source = "@harmony main\nDm7:300 G7:1 | Cmaj7";
        let result = parse_harmony_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("1-255"), "got: {err}");
    }

    #[test]
    fn test_degree_octave_displacement_out_of_range() {
        let source = "@pattern p steps=1 unit=1/4\n^1/300";
        let result = parse_pattern_from_source(source);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("0-10"), "got: {err}");
    }

    // ── Sharp chord alterations (token reassembly) ───────────────────

    #[test]
    fn test_sharp_root_still_parses_as_sharp_root() {
        // Disambiguation rule: single uppercase letter + `#` = sharp ROOT.
        // F#9 is F-sharp dominant ninth, NOT F with a sharp nine.
        let source = "@harmony main\nF#9";
        let block = parse_harmony_from_source(source).unwrap();
        let chord = &block.bars[0].chords[0].chord;
        assert_eq!(chord.root, 6); // F# pitch class
        assert_eq!(chord.intervals, vec![0, 4, 7, 10, 14]); // dominant 9
    }

    #[test]
    fn test_sharp_nine_alteration() {
        // F7#9: longer ident + `#` + integer = sharp ALTERATION on F7.
        let source = "@harmony main\nF7#9";
        let block = parse_harmony_from_source(source).unwrap();
        let chord = &block.bars[0].chords[0].chord;
        assert_eq!(chord.root, 5); // F pitch class
        assert_eq!(chord.intervals, vec![0, 4, 7, 10, 15]); // dom7 + #9
    }

    #[test]
    fn test_stacked_sharp_alterations() {
        // G7#5#9: iterative reassembly of Sharp + Integer fragments.
        let source = "@harmony main\nG7#5#9";
        let block = parse_harmony_from_source(source).unwrap();
        let chord = &block.bars[0].chords[0].chord;
        assert_eq!(chord.root, 7); // G
        assert_eq!(chord.intervals, vec![0, 4, 8, 10, 15]); // #5 replaces 7, #9 added
    }

    #[test]
    fn test_flat_then_sharp_alterations() {
        // C7b9#11: Ident("C7b9") + Sharp + Integer(11).
        let source = "@harmony main\nC7b9#11";
        let block = parse_harmony_from_source(source).unwrap();
        let chord = &block.bars[0].chords[0].chord;
        assert_eq!(chord.root, 0); // C
        assert_eq!(chord.intervals, vec![0, 4, 7, 10, 13, 18]); // dom7 + b9 + #11
    }

    #[test]
    fn test_sharp_then_flat_alterations() {
        // G7#5b9: Sharp + Integer, then a flat continuation Ident("b9").
        let source = "@harmony main\nG7#5b9";
        let block = parse_harmony_from_source(source).unwrap();
        let chord = &block.bars[0].chords[0].chord;
        assert_eq!(chord.root, 7);
        assert_eq!(chord.intervals, vec![0, 4, 8, 10, 13]);
    }

    #[test]
    fn test_sharp_alteration_does_not_swallow_next_chord() {
        // The chord after G7#9 must still be parsed as its own bar chord —
        // including a flat Roman numeral, which starts with 'b'.
        let source = "@harmony main\nG7#9 Cmaj7 | bVII7";
        let block = parse_harmony_from_source(source).unwrap();
        assert_eq!(block.bars.len(), 2);
        assert_eq!(block.bars[0].chords.len(), 2);
        assert_eq!(
            block.bars[0].chords[0].chord.intervals,
            vec![0, 4, 7, 10, 15]
        );
        assert_eq!(block.bars[0].chords[1].chord.root, 0); // Cmaj7
        assert_eq!(block.bars[1].chords[0].chord.root, 10); // bVII in C major
    }

    #[test]
    fn test_dollar_chord_sharp_alteration() {
        // The $-prefixed step-token site uses the same reassembly.
        let source = "@pattern p steps=1 unit=1/4\n$G7#9";
        let block = parse_pattern_from_source(source).unwrap();
        let PatternBody::Steps(ref lines) = block.body else {
            panic!("expected step body");
        };
        match &lines[0].tokens[0] {
            StepToken::ChordStep { chord, .. } => {
                assert_eq!(chord.root, 7);
                assert_eq!(chord.intervals, vec![0, 4, 7, 10, 15]);
            }
            other => panic!("expected ChordStep, got {other:?}"),
        }
    }

    #[test]
    fn test_dollar_chord_sharp_root_still_works() {
        let source = "@pattern p steps=1 unit=1/4\n$F#m7";
        let block = parse_pattern_from_source(source).unwrap();
        let PatternBody::Steps(ref lines) = block.body else {
            panic!("expected step body");
        };
        match &lines[0].tokens[0] {
            StepToken::ChordStep { chord, .. } => {
                assert_eq!(chord.root, 6); // F#
                assert_eq!(chord.intervals, vec![0, 3, 7, 10]); // m7
            }
            other => panic!("expected ChordStep, got {other:?}"),
        }
    }

    // ── Header directives after @scale ───────────────────────────────

    #[test]
    fn test_scale_first_then_header_directives() {
        let source = "@scale root=D mode=dorian\n@bpm 140\n@ppq 960\n@ts 3/4\n\n\
                      @pattern p unit=1/4: C4\n\n@track t ch=1\nplay: p\n";
        let program = parse_only(source).unwrap();
        assert_eq!(program.header.bpm, 140.0);
        assert_eq!(program.header.ppq, 960);
        assert_eq!(program.header.ts_numerator, 3);
        // The scalar @scale is still a Block::Scale in source order.
        match &program.blocks[0] {
            crate::ast::Block::Scale(tc) => {
                assert_eq!(tc.root, Some(2)); // D
                assert_eq!(tc.mode, "dorian");
            }
            other => panic!("expected Block::Scale first, got {other:?}"),
        }
    }

    #[test]
    fn test_duplicate_header_directive_across_scale_detected() {
        // Duplicate detection must survive the header-parse resumption.
        let source = "@bpm 120\n@scale root=C mode=major\n@bpm 140\n\n\
                      @pattern p unit=1/4: C4\n\n@track t ch=1\nplay: p\n";
        let err = parse_only(source).unwrap_err().to_string();
        assert!(err.contains("duplicate @bpm"), "got: {err}");
    }

    // ── @scale block form ────────────────────────────────────────────

    #[test]
    fn test_scale_block_form() {
        let source = "@scale\nroot=C mode=major * 2\nroot=A mode=minor\n\n\
                      @pattern p unit=1/4: ^1\n\n@track t ch=1\nplay: p\n";
        let program = parse_only(source).unwrap();
        let sb = program
            .header
            .scale_block
            .as_ref()
            .expect("block form must populate header.scale_block");
        assert_eq!(sb.entries.len(), 2);
        assert_eq!(sb.entries[0].root, Some(0));
        assert_eq!(sb.entries[0].mode.as_deref(), Some("major"));
        assert_eq!(sb.entries[0].bars, Some(2));
        assert_eq!(sb.entries[1].root, Some(9));
        assert_eq!(sb.entries[1].bars, None);
    }

    #[test]
    fn test_scale_inline_timeline_still_works() {
        let source = "@scale root=C mode=major * 8 | root=A mode=minor\n\n\
                      @pattern p unit=1/4: ^1\n\n@track t ch=1\nplay: p\n";
        let program = parse_only(source).unwrap();
        let sb = program
            .header
            .scale_block
            .as_ref()
            .expect("inline timeline");
        assert_eq!(sb.entries.len(), 2);
        assert_eq!(sb.entries[0].bars, Some(8));
    }

    #[test]
    fn test_scale_block_form_bad_entry_errors() {
        // A garbage entry line must error, not loop forever.
        let source = "@scale\nroot=C mode=major * 2\nfoo\n";
        let err = parse_only(source).unwrap_err().to_string();
        assert!(
            err.contains("root= or mode="),
            "expected entry error, got: {err}"
        );
    }

    #[test]
    fn test_bare_scale_alone_stays_scalar() {
        // Bare @scale with no entry lines is still the scalar form
        // (defaults), not an empty timeline.
        let source = "@scale\n\n@pattern p unit=1/4: C4\n\n@track t ch=1\nplay: p\n";
        let program = parse_only(source).unwrap();
        assert!(program.header.scale_block.is_none());
        assert!(matches!(program.blocks[0], crate::ast::Block::Scale(_)));
    }

    // ── @tempo hard error after blocks ───────────────────────────────

    #[test]
    fn test_tempo_after_block_is_hard_error() {
        let source = "@pattern p unit=1/4: C4\n\n@tempo\n120 | 140\n\n\
                      @track t ch=1\nplay: p\n";
        let err = parse_only(source).unwrap_err();
        assert!(
            matches!(err, CompileError::DeprecatedTempo { .. }),
            "expected DeprecatedTempo, got: {err:?}"
        );
    }

    // ── Duplicate block parameters ───────────────────────────────────

    #[test]
    fn test_duplicate_track_channel_errors() {
        let source = "@track t ch=1 ch=2 ch=3\nplay: p\n";
        let err = parse_track_from_source(source).unwrap_err().to_string();
        assert!(
            err.contains("duplicate parameter 'ch' in @track"),
            "got: {err}"
        );
    }

    #[test]
    fn test_duplicate_track_param_across_lines_errors() {
        let source = "@track t ch=1\nvel=90\nvel=100\nplay: p\n";
        let err = parse_track_from_source(source).unwrap_err().to_string();
        assert!(
            err.contains("duplicate parameter 'vel' in @track"),
            "got: {err}"
        );
    }

    #[test]
    fn test_duplicate_pattern_unit_errors() {
        let source = "@pattern p unit=1/4 unit=1/8\nC4";
        let err = parse_pattern_from_source(source).unwrap_err().to_string();
        assert!(
            err.contains("duplicate parameter 'unit' in @pattern"),
            "got: {err}"
        );
    }

    #[test]
    fn test_duplicate_harmony_oct_errors() {
        let source = "@harmony main oct=4 oct=5\nCmaj7";
        let err = parse_harmony_from_source(source).unwrap_err().to_string();
        assert!(
            err.contains("duplicate parameter 'oct' in @harmony"),
            "got: {err}"
        );
    }

    // ── arp named parameters ─────────────────────────────────────────

    #[test]
    fn test_arp_rate_named_parameter_parses() {
        // `rate` lexes as KwRate (also a track parameter); the arp argument
        // list must accept it as a key.
        let source = "@ppq 480\n@scale root=C mode=major\n@harmony main\nC\n@pattern c steps=1 unit=1/1\n$chord\n@track t ch=1 follow=main\nplay: c -> arp(pattern=down, rate=1/16, octaves=2)";
        let program = crate::parse_only(source).expect("arp(rate=...) must parse");
        drop(program);
    }

    // ── [gate:N] annotation range ────────────────────────────────────

    #[test]
    fn test_gate_annotation_out_of_range_errors() {
        let source = "@pattern p steps=1 unit=1/4\n^1[gate:8.0]";
        let err = parse_pattern_from_source(source).unwrap_err().to_string();
        assert!(err.contains("gate must be 0.0-1.0"), "got: {err}");
    }

    #[test]
    fn test_gate_annotation_in_range_ok() {
        let source = "@pattern p steps=1 unit=1/4\n^1[gate:0.5]";
        let block = parse_pattern_from_source(source).unwrap();
        let PatternBody::Steps(ref lines) = block.body else {
            panic!("expected step body");
        };
        match &lines[0].tokens[0] {
            StepToken::Degree { annotations, .. } => {
                assert!(matches!(annotations[0], Annotation::Gate(g) if g == 0.5));
            }
            other => panic!("expected Degree, got {other:?}"),
        }
    }

    // ── `+` inside subdivision brackets ──────────────────────────────

    #[test]
    fn test_subdivision_plus_cluster_structure() {
        // (^1+^3 ^5) — two slots: a simultaneous two-note cluster, then ^5.
        // The cluster is encoded as a single-alternative variant pool (see
        // parse_subdivision_inner).
        let source = "@pattern p steps=2 unit=1/4\n(^1+^3 ^5)\n.";
        let block = parse_pattern_from_source(source).unwrap();
        let PatternBody::Steps(ref lines) = block.body else {
            panic!("expected step body");
        };
        match &lines[0].tokens[0] {
            StepToken::Subdivision { tokens } => {
                assert_eq!(tokens.len(), 2, "cluster must occupy ONE slot");
                match &tokens[0] {
                    StepToken::Variant { alternatives } => {
                        assert_eq!(alternatives.len(), 1);
                        assert_eq!(alternatives[0].len(), 2);
                        assert!(matches!(
                            alternatives[0][0],
                            StepToken::Degree { degree: 1, .. }
                        ));
                        assert!(matches!(
                            alternatives[0][1],
                            StepToken::Degree { degree: 3, .. }
                        ));
                    }
                    other => panic!("expected simultaneous cluster, got {other:?}"),
                }
                assert!(matches!(tokens[1], StepToken::Degree { degree: 5, .. }));
            }
            other => panic!("expected Subdivision, got {other:?}"),
        }
    }
}
