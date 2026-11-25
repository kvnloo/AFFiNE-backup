//! Pure-Rust line-state-machine syntax highlighter.
//!
//! Replaces regex/Wasm highlighters with a deterministic, linear-time per-line lexer
//! that propagates multi-line state (block comments, multi-line strings, template
//! literals) across lines. Single-line edits re-lex only until the end-of-line state
//! converges with the cached state of the next line.

mod languages;
mod lexer;

use std::ops::Range;

use ui::Hsla;

use crate::theme::ThemeMode;

pub use languages::{LanguageId, LanguageSpec, language_spec, resolve_language};
pub use lexer::{LineState, lex_line};

/// Semantic token slots mapped to light and dark theme colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Plain,
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Operator,
    Punctuation,
    Macro,
    Property,
    Constant,
    Tag,
    DiffAdded,
    DiffRemoved,
}

/// One byte-offset highlight span within a single line of source code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub bytes: Range<usize>,
    pub kind: TokenKind,
}

/// Highlighted spans and outgoing lexer state for a single source line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightedLine {
    pub spans: Vec<HighlightSpan>,
    pub end_state: LineState,
}

/// Highlight a single line starting from `start_state`.
pub fn highlight_line(lang: LanguageId, line: &str, start_state: LineState) -> HighlightedLine {
    lex_line(lang, line, start_state)
}

/// Highlight a full multi-line document, propagating lexer state across lines.
pub fn highlight_document(language: &str, text: &str) -> Vec<HighlightedLine> {
    let lang = resolve_language(language);
    let mut state = LineState::Normal;
    let mut out = Vec::new();
    for line in text.split('\n') {
        let highlighted = lex_line(lang, line, state);
        state = highlighted.end_state;
        out.push(highlighted);
    }
    out
}

/// Incrementally re-highlight `lines` starting at `changed_line`, stopping as soon as
/// a line's outgoing [`LineState`] matches the previously cached state.
///
/// Returns the number of lines re-lexed.
pub fn highlight_incremental(
    language: &str,
    lines: &[&str],
    cache: &mut Vec<HighlightedLine>,
    changed_line: usize,
) -> usize {
    let lang = resolve_language(language);
    if cache.len() != lines.len() || changed_line >= lines.len() {
        cache.clear();
        let mut state = LineState::Normal;
        for line in lines {
            let hl = lex_line(lang, line, state);
            state = hl.end_state;
            cache.push(hl);
        }
        return lines.len();
    }

    let mut state = if changed_line == 0 {
        LineState::Normal
    } else {
        cache[changed_line - 1].end_state
    };
    let mut relexed = 0;
    for idx in changed_line..lines.len() {
        let prev_end = cache[idx].end_state;
        let next = lex_line(lang, lines[idx], state);
        state = next.end_state;
        cache[idx] = next;
        relexed += 1;
        if idx > changed_line && state == prev_end {
            break;
        }
    }
    relexed
}

/// Resolve a semantic [`TokenKind`] into a crisp light or dark theme color.
pub fn syntax_color(kind: TokenKind, mode: ThemeMode) -> Hsla {
    let rgb = match mode {
        ThemeMode::Light => match kind {
            TokenKind::Plain => 0x1E1E1E,
            TokenKind::Keyword => 0xCF222E,
            TokenKind::Type => 0x953800,
            TokenKind::Function => 0x8250DF,
            TokenKind::String => 0x0A3069,
            TokenKind::Number => 0x0550AE,
            TokenKind::Comment => 0x6E7781,
            TokenKind::Operator => 0x0550AE,
            TokenKind::Punctuation => 0x57606A,
            TokenKind::Macro => 0x953800,
            TokenKind::Property => 0x116329,
            TokenKind::Constant => 0x0550AE,
            TokenKind::Tag => 0x116329,
            TokenKind::DiffAdded => 0x116329,
            TokenKind::DiffRemoved => 0xCF222E,
        },
        ThemeMode::Dark => match kind {
            TokenKind::Plain => 0xE6EDF3,
            TokenKind::Keyword => 0xFF7B72,
            TokenKind::Type => 0xFFA657,
            TokenKind::Function => 0xD2A8FF,
            TokenKind::String => 0xA5D6FF,
            TokenKind::Number => 0x79C0FF,
            TokenKind::Comment => 0x8B949E,
            TokenKind::Operator => 0x79C0FF,
            TokenKind::Punctuation => 0x9CA3AF,
            TokenKind::Macro => 0xFFA657,
            TokenKind::Property => 0x7EE787,
            TokenKind::Constant => 0x79C0FF,
            TokenKind::Tag => 0x7EE787,
            TokenKind::DiffAdded => 0x7EE787,
            TokenKind::DiffRemoved => 0xFF7B72,
        },
    };
    ui::rgb(rgb).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_language_aliases_across_thirty_families() {
        let cases = [
            ("rs", LanguageId::Rust),
            ("rust", LanguageId::Rust),
            ("ts", LanguageId::TypeScript),
            ("typescript", LanguageId::TypeScript),
            ("tsx", LanguageId::Tsx),
            ("js", LanguageId::JavaScript),
            ("py", LanguageId::Python),
            ("go", LanguageId::Go),
            ("c", LanguageId::C),
            ("cpp", LanguageId::Cpp),
            ("java", LanguageId::Java),
            ("kt", LanguageId::Kotlin),
            ("swift", LanguageId::Swift),
            ("bash", LanguageId::Shell),
            ("json", LanguageId::Json),
            ("yml", LanguageId::Yaml),
            ("toml", LanguageId::Toml),
            ("sql", LanguageId::Sql),
            ("html", LanguageId::Html),
            ("css", LanguageId::Css),
            ("md", LanguageId::Markdown),
            ("tex", LanguageId::Latex),
            ("typ", LanguageId::Typst),
            ("mermaid", LanguageId::Mermaid),
            ("gql", LanguageId::Graphql),
            ("dockerfile", LanguageId::Dockerfile),
            ("diff", LanguageId::Diff),
            ("rb", LanguageId::Ruby),
            ("php", LanguageId::Php),
            ("cs", LanguageId::CSharp),
            ("lua", LanguageId::Lua),
            ("zig", LanguageId::Zig),
            ("unknown-lang", LanguageId::Plain),
        ];
        for (alias, expected) in cases {
            assert_eq!(resolve_language(alias), expected, "alias={alias}");
        }
    }

    #[test]
    fn multiline_state_propagates_and_incremental_relex_converges() {
        let lines = [
            "/* start comment",
            "still comment",
            "end comment */",
            "fn alpha() -> usize { 42 }",
            "let beta = \"hello\";",
        ];
        let mut cache = highlight_document("rust", &lines.join("\n"));
        assert_eq!(cache.len(), 5);
        assert!(cache[1].spans.iter().all(|s| s.kind == TokenKind::Comment));
        assert_eq!(cache[2].end_state, LineState::Normal);
        assert!(cache[3].spans.iter().any(|s| s.kind == TokenKind::Keyword));
        assert!(cache[3].spans.iter().any(|s| s.kind == TokenKind::Number));
        assert!(cache[4].spans.iter().any(|s| s.kind == TokenKind::String));

        // Editing line 3 without changing its end state re-lexes line 3 and converges on line 4.
        let edited = [
            "/* start comment",
            "still comment",
            "end comment */",
            "fn alpha() -> usize { 99 }",
            "let beta = \"hello\";",
        ];
        let count = highlight_incremental("rust", &edited, &mut cache, 3);
        assert!(count <= 2, "expected early convergence, relexed {count}");
    }

    #[test]
    fn latex_and_typescript_tokens_classify_distinct_semantic_slots() {
        let tex = highlight_document("latex", r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}");
        assert!(tex[0].spans.iter().any(|s| s.kind == TokenKind::Keyword));
        assert!(tex[0].spans.iter().any(|s| s.kind == TokenKind::Number));
        assert!(
            tex[0]
                .spans
                .iter()
                .any(|s| s.kind == TokenKind::Punctuation)
        );

        let ts = highlight_document("ts", "export const count: number = 3; // total");
        let kinds: Vec<TokenKind> = ts[0].spans.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::Type));
        assert!(kinds.contains(&TokenKind::Number));
        assert!(kinds.contains(&TokenKind::Comment));
    }
}
