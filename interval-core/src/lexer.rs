//! Lexer for the Interval language.
//!
//! Uses `logos` for fast, declarative tokenization. Handles all token types
//! defined in the spec: header directives, block declarations, step tokens,
//! operators, brackets, annotations, and comments.
//!
//! The lexer produces a flat token stream. Context-sensitive disambiguation
//! (e.g., `|` as bar separator vs. transform pipe) happens in the parser.

use logos::Logos;

/// All token types produced by the Interval lexer.
///
/// The lexer is context-free. `|` is now exclusively the bar separator for
/// harmony blocks and timeline directives. The transform pipe is `->` and the
/// variant separator is `,`. The parser enforces these rules.
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t]+")]
pub enum Token {
    // ── Newlines ──────────────────────────────────────────────────────
    /// Newline (significant: each step occupies exactly one line).
    #[regex(r"\n|\r\n")]
    Newline,

    // ── Comments ──────────────────────────────────────────────────────
    /// Line comment (stripped by lexer, content preserved for diagnostics).
    #[regex(r"//[^\n]*")]
    Comment,

    // ── Header directives ─────────────────────────────────────────────
    /// `@ppq` — pulses per quarter note.
    #[token("@ppq")]
    AtPpq,

    /// `@bpm` — tempo in beats per minute.
    #[token("@bpm")]
    AtBpm,

    /// `@ts` — time signature.
    #[token("@ts")]
    AtTs,

    /// `@title` — optional title string.
    #[token("@title")]
    AtTitle,

    /// `@seed` — global seed for seeded operations.
    #[token("@seed")]
    AtSeed,

    /// `@bars` — global bar count for automatic pattern fill.
    #[token("@bars")]
    AtBars,

    // ── Block declarations ────────────────────────────────────────────
    /// `@scale` — global tonal context block.
    #[token("@scale")]
    AtScale,

    /// `@harmony` — harmony timeline block.
    #[token("@harmony")]
    AtHarmony,

    /// `@pattern` — pattern block.
    #[token("@pattern")]
    AtPattern,

    /// `@track` — track block.
    #[token("@track")]
    AtTrack,

    /// `@drummap` — drum map block.
    #[token("@drummap")]
    AtDrummap,

    /// `@tempo` — tempo timeline block.
    #[token("@tempo")]
    AtTempo,

    // ── Keywords / parameter names ────────────────────────────────────
    /// `mode` keyword (in `mode=`).
    #[token("mode")]
    KwMode,

    /// `play` keyword (in `play=` or `play:`).
    #[token("play")]
    KwPlay,

    /// `steps` keyword (in `steps=` or `steps:`).
    #[token("steps")]
    KwSteps,

    /// `unit` keyword (in `unit=`).
    #[token("unit")]
    KwUnit,

    /// `vel` keyword.
    #[token("vel")]
    KwVel,

    /// `gate` keyword.
    #[token("gate")]
    KwGate,

    /// `oct` keyword.
    #[token("oct")]
    KwOct,

    /// `ch` keyword.
    #[token("ch")]
    KwCh,

    /// `prog` keyword.
    #[token("prog")]
    KwProg,

    /// `voice` keyword.
    #[token("voice")]
    KwVoice,

    /// `inv` keyword.
    #[token("inv")]
    KwInv,

    /// `seed` keyword (track-level).
    #[token("seed")]
    KwSeed,

    /// `follow` keyword.
    #[token("follow")]
    KwFollow,

    /// `shift` keyword.
    #[token("shift")]
    KwShift,

    /// `lshift` keyword.
    #[token("lshift")]
    KwLshift,

    /// `type` keyword (in `type=drums`).
    #[token("type")]
    KwType,

    /// `drummap` keyword (in `drummap=name`).
    #[token("drummap")]
    KwDrummap,

    /// `section` keyword (in `section:` modulation directive).
    #[token("section")]
    KwSection,

    /// `bar` keyword (in `bar=N` within section directive).
    #[token("bar")]
    KwBar,

    /// `root` keyword (in `root=X` within section directive).
    #[token("root")]
    KwRoot,

    /// `dur` annotation keyword.
    #[token("dur")]
    KwDur,

    /// `expr` annotation keyword.
    #[token("expr")]
    KwExpr,

    /// `dyn` annotation keyword.
    #[token("dyn")]
    KwDyn,

    /// `sus` annotation keyword.
    #[token("sus")]
    KwSus,

    /// `pan` annotation keyword.
    #[token("pan")]
    KwPan,

    /// `vol` annotation keyword.
    #[token("vol")]
    KwVol,

    /// `pb` annotation keyword (pitch bend).
    #[token("pb")]
    KwPb,

    /// `at` annotation keyword (aftertouch).
    #[token("at")]
    KwAt,

    /// `auto` keyword (for `inv=auto`).
    #[token("auto")]
    KwAuto,

    /// `every` conditional annotation keyword.
    #[token("every")]
    KwEvery,

    /// `cond` conditional annotation keyword.
    #[token("cond")]
    KwCond,

    /// `once` conditional annotation keyword.
    #[token("once")]
    KwOnce,

    /// `pre` conditional annotation keyword.
    #[token("pre")]
    KwPre,

    /// `rate` keyword.
    #[token("rate")]
    KwRate,

    /// `evolve` keyword.
    #[token("evolve")]
    KwEvolve,

    /// `euclid_gate` keyword.
    #[token("euclid_gate")]
    KwEuclidGate,

    /// `echo` keyword.
    #[token("echo")]
    KwEcho,

    /// `vel_curve` keyword.
    #[token("vel_curve")]
    KwVelCurve,

    /// `gate_curve` keyword.
    #[token("gate_curve")]
    KwGateCurve,

    /// `sine` keyword (wave shape).
    #[token("sine")]
    KwSine,

    /// `tri` keyword (wave shape).
    #[token("tri")]
    KwTri,

    /// `ramp` keyword (wave shape).
    #[token("ramp")]
    KwRamp,

    /// `square` keyword (wave shape).
    #[token("square")]
    KwSquare,

    /// `random` keyword (wave shape).
    #[token("random")]
    KwRandom,

    /// `arp` transform name.
    #[token("arp")]
    KwArp,

    /// `updown` arp pattern (up then down, no repeat at top).
    #[token("updown")]
    KwUpDown,

    /// `scale_lock` keyword.
    #[token("scale_lock")]
    KwScaleLock,

    /// `down` keyword (snap mode).
    #[token("down")]
    KwDown,

    /// `up` keyword (snap mode).
    #[token("up")]
    KwUp,

    /// `filter` keyword (snap mode).
    #[token("filter")]
    KwFilter,

    /// `swing` keyword.
    #[token("swing")]
    KwSwing,

    /// `swingunit` keyword.
    #[token("swingunit")]
    KwSwingUnit,

    /// `ratch` keyword (ratchet count annotation).
    #[token("ratch")]
    KwRatch,

    /// `ratch_decay` keyword (ratchet velocity decay annotation).
    #[token("ratch_decay")]
    KwRatchDecay,

    /// `true` literal.
    #[token("true")]
    KwTrue,

    /// `false` literal.
    #[token("false")]
    KwFalse,

    /// `drums` keyword (for `type=drums`).
    #[token("drums")]
    KwDrums,

    /// `start` keyword (in `start=<bar>`).
    #[token("start")]
    KwStart,

    /// `prob` annotation keyword (step probability).
    #[token("prob")]
    KwProb,

    /// `glide` annotation keyword (portamento).
    #[token("glide")]
    KwGlide,

    // ── Transform names ───────────────────────────────────────────────
    /// `reverse` transform.
    #[token("reverse")]
    KwReverse,

    /// `invert` transform.
    #[token("invert")]
    KwInvert,

    /// `retrograde` transform.
    #[token("retrograde")]
    KwRetrograde,

    /// `rotate` transform.
    #[token("rotate")]
    KwRotate,

    /// `stretch` transform.
    #[token("stretch")]
    KwStretch,

    /// `compress` transform.
    #[token("compress")]
    KwCompress,

    /// `transpose` transform.
    #[token("transpose")]
    KwTranspose,

    /// `shift_oct` transform.
    #[token("shift_oct")]
    KwShiftOct,

    /// `subset` transform.
    #[token("subset")]
    KwSubset,

    /// `interleave` transform.
    #[token("interleave")]
    KwInterleave,

    /// `mirror` transform.
    #[token("mirror")]
    KwMirror,

    /// `humanize` transform.
    #[token("humanize")]
    KwHumanize,

    /// `vary` transform.
    #[token("vary")]
    KwVary,

    /// `rubato` expressive transform.
    #[token("rubato")]
    KwRubato,

    /// `ritardando` expressive transform.
    #[token("ritardando")]
    KwRitardando,

    /// `accelerando` expressive transform.
    #[token("accelerando")]
    KwAccelerando,

    /// `agogic` expressive transform.
    #[token("agogic")]
    KwAgogic,

    /// `breathe` expressive transform.
    #[token("breathe")]
    KwBreathe,

    /// `swell` expressive transform.
    #[token("swell")]
    KwSwell,

    /// `phrase` expressive transform.
    #[token("phrase")]
    KwPhrase,

    /// `ease_in` curve name.
    #[token("ease_in")]
    KwEaseIn,

    /// `ease_out` curve name.
    #[token("ease_out")]
    KwEaseOut,

    /// `ease_in_out` curve name.
    #[token("ease_in_out")]
    KwEaseInOut,

    /// `arch` curve name.
    #[token("arch")]
    KwArch,

    /// `linear` curve name (for `@bpm` timeline ramp).
    #[token("linear")]
    KwLinear,

    // ── CC annotation with number ─────────────────────────────────────
    /// `cc<N>` arbitrary CC annotation (e.g., `cc74`).
    #[regex(r"cc[0-9]+", |lex| lex.slice()[2..].parse::<u8>().ok())]
    KwCc(u8),

    // ── Degree tokens ─────────────────────────────────────────────────
    /// `^` degree prefix.
    #[token("^")]
    Caret,

    // ── Step tokens ───────────────────────────────────────────────────
    /// `.` rest.
    #[token(".")]
    Dot,

    /// `~` tie / hold.
    #[token("~")]
    Tilde,

    /// `+` simultaneous notes separator.
    #[token("+")]
    Plus,

    /// `$` chord symbol prefix (in step lines).
    #[token("$")]
    Dollar,

    /// `%` bare percent sign (chord ordinal prefix: `%1`, `%2`, etc.).
    /// Distinct from `Percent(f64)` which requires preceding digits (e.g., `5%`).
    #[token("%")]
    PercentSign,

    /// `@` bare at sign (per-reference rate modifier: `pattern@2.0`).
    #[token("@")]
    At,

    // Note: MIDI note numbers (e.g., `n60`) are lexed as identifiers
    // and parsed contextually by the parser.

    // ── Brackets ──────────────────────────────────────────────────────
    /// `(` open parenthesis (subdivision or transform args).
    #[token("(")]
    LParen,

    /// `)` close parenthesis.
    #[token(")")]
    RParen,

    /// `[` open square bracket (step annotation).
    #[token("[")]
    LBracket,

    /// `]` close square bracket.
    #[token("]")]
    RBracket,

    /// `{` open curly brace (variant pool).
    #[token("{")]
    LBrace,

    /// `}` close curly brace.
    #[token("}")]
    RBrace,

    // ── Operators and separators ──────────────────────────────────────
    /// `|` bar separator (harmony blocks and timeline directives).
    /// NOTE: `|` is no longer the transform pipe (use `->`) or variant separator (use `,`).
    #[token("|")]
    Pipe,

    /// `*~` soft boundary repetition.
    #[token("*~")]
    StarTilde,

    /// `*` hard boundary repetition.
    #[token("*")]
    Star,

    /// `~>>` soft boundary concatenation.
    #[token("~>>")]
    TildeRShift,

    /// `>>` hard boundary concatenation.
    #[token(">>")]
    RShift,

    /// `->` ramp arrow in annotations (e.g., `expr:40->88`).
    #[token("->")]
    Arrow,

    /// `=` assignment / key-value separator in directives.
    #[token("=")]
    Equals,

    /// `:` key-value separator in annotations and beat assignment.
    #[token(":")]
    Colon,

    /// `/` fraction separator (e.g., `1/4`, `4/4`).
    #[token("/")]
    Slash,

    /// `,` comma (transform argument separator).
    #[token(",")]
    Comma,

    // ── Note names (for chord roots and absolute pitch) ───────────────
    // These are matched as identifiers and resolved in the parser.

    // ── Accidentals ───────────────────────────────────────────────────
    /// `#` sharp accidental.
    #[token("#")]
    Sharp,

    /// `b` flat accidental — handled contextually in the parser since
    /// 'b' is also a valid note name. The parser distinguishes based on
    /// position (after `^` degree number = accidental, standalone = note B).

    // ── Literals ──────────────────────────────────────────────────────
    /// Integer literal.
    #[regex(r"-?[0-9]+", |lex| lex.slice().parse::<i64>().ok(), priority = 2)]
    Integer(i64),

    /// Float literal (must contain a decimal point).
    #[regex(r"-?[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().ok())]
    Float(f64),

    /// Percent literal (e.g., `5%`, `-3%`).
    #[regex(r"-?[0-9]+(\.[0-9]+)?%", |lex| {
        let s = lex.slice();
        s[..s.len()-1].parse::<f64>().ok()
    })]
    Percent(f64),

    /// Millisecond literal (e.g., `8ms`, `-5ms`).
    #[regex(r"-?[0-9]+(\.[0-9]+)?ms", |lex| {
        let s = lex.slice();
        s[..s.len()-2].parse::<f64>().ok()
    })]
    Milliseconds(f64),

    /// Quoted string literal (double quotes).
    #[regex(r#""[^"]*""#, |lex| {
        let s = lex.slice();
        s[1..s.len()-1].to_string()
    })]
    StringLiteral(String),

    // ── Identifiers ───────────────────────────────────────────────────
    /// General identifier (pattern names, harmony names, track names,
    /// drum hit names, mode names, voicing names, note names, etc.).
    /// The parser resolves the meaning based on context.
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*", |lex| lex.slice().to_string(), priority = 1)]
    Ident(String),

    // ── Unicode chord symbols ─────────────────────────────────────────
    /// `Δ` (delta) — major seventh quality symbol.
    #[token("Δ")]
    Delta,

    /// `°` (degree sign) — diminished quality symbol.
    #[token("°")]
    Degree,

    /// `ø` (slashed o) — half-diminished quality symbol.
    #[token("ø")]
    HalfDim,
}

/// A token with its source span.
#[derive(Debug, Clone)]
pub struct SpannedToken {
    /// The token type.
    pub token: Token,
    /// Start byte offset in source (inclusive).
    pub start: usize,
    /// End byte offset in source (exclusive).
    pub end: usize,
}

/// Tokenize a Interval source string into a sequence of spanned tokens.
///
/// Comments are included in the token stream (the parser can skip them).
/// Lexer errors are collected with their spans for error reporting.
pub fn tokenize(source: &str) -> (Vec<SpannedToken>, Vec<(usize, usize)>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    let lex = Token::lexer(source);
    for (result, span) in lex.spanned() {
        match result {
            Ok(token) => {
                tokens.push(SpannedToken {
                    token,
                    start: span.start,
                    end: span.end,
                });
            }
            Err(()) => {
                errors.push((span.start, span.end));
            }
        }
    }

    (tokens, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_tokens() {
        let source = "@ppq 480\n@bpm 120.0\n@ts 4/4\n@title \"Hello\"\n@seed 42";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        // Filter out newlines for easier assertion
        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::AtPpq);
        assert_eq!(toks[1], &Token::Integer(480));
        assert_eq!(toks[2], &Token::Newline);
        assert_eq!(toks[3], &Token::AtBpm);
        assert_eq!(toks[4], &Token::Float(120.0));
        assert_eq!(toks[5], &Token::Newline);
        assert_eq!(toks[6], &Token::AtTs);
        assert_eq!(toks[7], &Token::Integer(4));
        assert_eq!(toks[8], &Token::Slash);
        assert_eq!(toks[9], &Token::Integer(4));
        assert_eq!(toks[10], &Token::Newline);
        assert_eq!(toks[11], &Token::AtTitle);
        assert_eq!(toks[12], &Token::StringLiteral("Hello".to_string()));
        assert_eq!(toks[13], &Token::Newline);
        assert_eq!(toks[14], &Token::AtSeed);
        assert_eq!(toks[15], &Token::Integer(42));
    }

    #[test]
    fn test_step_tokens() {
        let source = "^1+^3+^5 . ~ (^1 ^3) {^1, ^b7}";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::Caret);
        assert_eq!(toks[1], &Token::Integer(1));
        assert_eq!(toks[2], &Token::Plus);
        assert_eq!(toks[3], &Token::Caret);
        assert_eq!(toks[4], &Token::Integer(3));
        assert_eq!(toks[5], &Token::Plus);
        assert_eq!(toks[6], &Token::Caret);
        assert_eq!(toks[7], &Token::Integer(5));
        assert_eq!(toks[8], &Token::Dot);
        assert_eq!(toks[9], &Token::Tilde);
        assert_eq!(toks[10], &Token::LParen);
        assert_eq!(toks[11], &Token::Caret);
        assert_eq!(toks[12], &Token::Integer(1));
        assert_eq!(toks[13], &Token::Caret);
        assert_eq!(toks[14], &Token::Integer(3));
        assert_eq!(toks[15], &Token::RParen);
        assert_eq!(toks[16], &Token::LBrace);
        assert_eq!(toks[17], &Token::Caret);
        assert_eq!(toks[18], &Token::Integer(1));
        assert_eq!(toks[19], &Token::Comma);
        assert_eq!(toks[20], &Token::Caret);
    }

    #[test]
    fn test_annotation_tokens() {
        let source = "^1[vel:110 shift:-3%]";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::Caret);
        assert_eq!(toks[1], &Token::Integer(1));
        assert_eq!(toks[2], &Token::LBracket);
        assert_eq!(toks[3], &Token::KwVel);
        assert_eq!(toks[4], &Token::Colon);
        assert_eq!(toks[5], &Token::Integer(110));
        assert_eq!(toks[6], &Token::KwShift);
        assert_eq!(toks[7], &Token::Colon);
        assert_eq!(toks[8], &Token::Percent(-3.0));
        assert_eq!(toks[9], &Token::RBracket);
    }

    #[test]
    fn test_pattern_composition_tokens() {
        let source = "play: verse >> chorus * 4 ~>> outro";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::KwPlay);
        assert_eq!(toks[1], &Token::Colon);
        assert_eq!(toks[2], &Token::Ident("verse".to_string()));
        assert_eq!(toks[3], &Token::RShift);
        assert_eq!(toks[4], &Token::Ident("chorus".to_string()));
        assert_eq!(toks[5], &Token::Star);
        assert_eq!(toks[6], &Token::Integer(4));
        assert_eq!(toks[7], &Token::TildeRShift);
        assert_eq!(toks[8], &Token::Ident("outro".to_string()));
    }

    #[test]
    fn test_ramp_annotation() {
        let source = "[expr:40->88]";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::LBracket);
        assert_eq!(toks[1], &Token::KwExpr);
        assert_eq!(toks[2], &Token::Colon);
        assert_eq!(toks[3], &Token::Integer(40));
        assert_eq!(toks[4], &Token::Arrow);
        assert_eq!(toks[5], &Token::Integer(88));
        assert_eq!(toks[6], &Token::RBracket);
    }

    #[test]
    fn test_comment_stripping() {
        let source = "@ppq 480 // pulses per quarter note\n@bpm 120.0";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::AtPpq);
        assert_eq!(toks[1], &Token::Integer(480));
        assert_eq!(toks[2], &Token::Comment);
        assert_eq!(toks[3], &Token::Newline);
        assert_eq!(toks[4], &Token::AtBpm);
        assert_eq!(toks[5], &Token::Float(120.0));
    }

    #[test]
    fn test_harmony_block_tokens() {
        let source = "@harmony main mode=dorian\nCm7 | Fm7";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::AtHarmony);
        assert_eq!(toks[1], &Token::Ident("main".to_string()));
        assert_eq!(toks[2], &Token::KwMode);
        assert_eq!(toks[3], &Token::Equals);
        assert_eq!(toks[4], &Token::Ident("dorian".to_string()));
        assert_eq!(toks[5], &Token::Newline);
        // "Cm7" — C is an ident, m is part of it... actually "Cm7" is a single ident
        assert_eq!(toks[6], &Token::Ident("Cm7".to_string()));
        assert_eq!(toks[7], &Token::Pipe);
        assert_eq!(toks[8], &Token::Ident("Fm7".to_string()));
    }

    #[test]
    fn test_soft_repetition() {
        let source = "pattern *~ 4";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::Ident("pattern".to_string()));
        assert_eq!(toks[1], &Token::StarTilde);
        assert_eq!(toks[2], &Token::Integer(4));
    }

    #[test]
    fn test_cc_annotation() {
        let source = "[cc74:30->80]";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::LBracket);
        assert_eq!(toks[1], &Token::KwCc(74));
        assert_eq!(toks[2], &Token::Colon);
        assert_eq!(toks[3], &Token::Integer(30));
        assert_eq!(toks[4], &Token::Arrow);
        assert_eq!(toks[5], &Token::Integer(80));
        assert_eq!(toks[6], &Token::RBracket);
    }

    #[test]
    fn test_millisecond_timing() {
        let source = "shift=8ms";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::KwShift);
        assert_eq!(toks[1], &Token::Equals);
        assert_eq!(toks[2], &Token::Milliseconds(8.0));
    }

    #[test]
    fn test_track_declaration() {
        let source = "@track bass ch=2 prog=32 oct=2 vel=88 follow=main";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::AtTrack);
        assert_eq!(toks[1], &Token::Ident("bass".to_string()));
        assert_eq!(toks[2], &Token::KwCh);
        assert_eq!(toks[3], &Token::Equals);
        assert_eq!(toks[4], &Token::Integer(2));
        assert_eq!(toks[5], &Token::KwProg);
        assert_eq!(toks[6], &Token::Equals);
        assert_eq!(toks[7], &Token::Integer(32));
    }

    #[test]
    fn test_unicode_chord_symbols() {
        let source = "Δ7 °7 ø7";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::Delta);
        assert_eq!(toks[1], &Token::Integer(7));
        assert_eq!(toks[2], &Token::Degree);
        assert_eq!(toks[3], &Token::Integer(7));
        assert_eq!(toks[4], &Token::HalfDim);
        assert_eq!(toks[5], &Token::Integer(7));
    }

    #[test]
    fn test_drummap_tokens() {
        let source = "@drummap kit\n  kick = 36\n  snare = 38";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::AtDrummap);
        assert_eq!(toks[1], &Token::Ident("kit".to_string()));
        assert_eq!(toks[2], &Token::Newline);
        assert_eq!(toks[3], &Token::Ident("kick".to_string()));
        assert_eq!(toks[4], &Token::Equals);
        assert_eq!(toks[5], &Token::Integer(36));
    }

    #[test]
    fn test_pattern_with_transforms() {
        let source = "@pattern arp_down = arp_up -> reverse";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::AtPattern);
        assert_eq!(toks[1], &Token::Ident("arp_down".to_string()));
        assert_eq!(toks[2], &Token::Equals);
        assert_eq!(toks[3], &Token::Ident("arp_up".to_string()));
        assert_eq!(toks[4], &Token::Arrow);
        assert_eq!(toks[5], &Token::KwReverse);
    }

    #[test]
    fn test_section_directive() {
        let source = "section: bar=5 mode=dorian root=D";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "unexpected lexer errors: {errors:?}");

        let toks: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert_eq!(toks[0], &Token::KwSection);
        assert_eq!(toks[1], &Token::Colon);
        assert_eq!(toks[2], &Token::KwBar);
        assert_eq!(toks[3], &Token::Equals);
        assert_eq!(toks[4], &Token::Integer(5));
        assert_eq!(toks[5], &Token::KwMode);
        assert_eq!(toks[6], &Token::Equals);
        assert_eq!(toks[7], &Token::Ident("dorian".to_string()));
        assert_eq!(toks[8], &Token::KwRoot);
        assert_eq!(toks[9], &Token::Equals);
        // D will be parsed as an identifier
        assert_eq!(toks[10], &Token::Ident("D".to_string()));
    }
}
