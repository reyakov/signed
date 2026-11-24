//! Highlights source text into role-styled lines with tree-sitter grammars.
//! Roles are resolved to theme colors at render time, so a theme change
//! re-colors code without re-parsing it.
//!
//! # Examples
//!
//! ```no_run
//! use ui::highlight::{Language, highlight};
//!
//! let code = highlight("let answer = 42;\n", Language::Rust);
//! for span in &code.lines[0].spans {
//!     println!("{} ({:?})", span.text, span.role);
//! }
//! ```

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::SharedString;
#[cfg(not(target_arch = "wasm32"))]
use tree_sitter::StreamingIterator as _;

use crate::components::common::ResultLogErr;

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static HIGHLIGHTERS: RefCell<HashMap<Language, Option<Highlighter>>> =
        RefCell::new(HashMap::new());
}

/// A language source can highlight as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Plain,
    Rust,
    JavaScript,
    TypeScript,
    Python,
}

/// The semantic role a span plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Text,
    Keyword,
    String,
    Comment,
    Function,
    Type,
    Number,
    Constant,
    Property,
    Operator,
    Punctuation,
    Tag,
    Escape,
}

/// A grammar's parser and compiled highlight query.
#[cfg(not(target_arch = "wasm32"))]
struct Highlighter {
    parser: tree_sitter::Parser,
    query: tree_sitter::Query,
    roles: Vec<Option<Role>>,
}

/// A run of text sharing one role.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    /// The run's text.
    pub text: SharedString,
    /// The semantic role applied to the text.
    pub role: Role,
}

/// One rendered line of code.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Line {
    /// The spans making up the line, in character order.
    pub spans: Vec<Span>,
}

impl Line {
    /// The line's flat text, in character order.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for span in &self.spans {
            text.push_str(&span.text);
        }
        text
    }
}

/// Source text split into role-styled lines.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct HighlightedCode {
    /// The source's lines, in order.
    pub lines: Vec<Line>,
}

/// Highlights `source` with `language`'s tree-sitter grammar, falling back
/// to plain text when the grammar is unavailable or parsing fails.
#[cfg(not(target_arch = "wasm32"))]
pub fn highlight(source: &str, language: Language) -> HighlightedCode {
    let Some((grammar, query_source)) = grammar_for(language) else {
        return plain(source);
    };

    HIGHLIGHTERS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let highlighter = cache
            .entry(language)
            .or_insert_with(|| {
                let mut parser = tree_sitter::Parser::new();
                parser.set_language(&grammar).log_err()?;
                let query = tree_sitter::Query::new(&grammar, query_source).log_err()?;
                let roles = query
                    .capture_names()
                    .iter()
                    .map(|name| role_for_capture(name))
                    .collect();
                Some(Highlighter {
                    parser,
                    query,
                    roles,
                })
            })
            .as_mut();

        let Some(highlighter) = highlighter else {
            return plain(source);
        };

        let Some(tree) = highlighter.parser.parse(source, None) else {
            log::warn!("tree-sitter failed to parse {language:?}; rendering plain text");
            return plain(source);
        };

        // One role per byte; matches arrive in query order
        let mut role_at = vec![None; source.len()];
        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(&highlighter.query, tree.root_node(), source.as_bytes());

        while let Some(matched) = matches.next() {
            for capture in matched.captures {
                let Some(role) = highlighter
                    .roles
                    .get(capture.index as usize)
                    .copied()
                    .flatten()
                else {
                    continue;
                };
                for byte in capture.node.byte_range() {
                    if let Some(slot) = role_at.get_mut(byte) {
                        *slot = Some(role);
                    }
                }
            }
        }

        build_lines(source, &role_at)
    })
}

/// Highlights `source` as plain text; the web target has no grammars.
#[cfg(target_arch = "wasm32")]
pub fn highlight(source: &str, _language: Language) -> HighlightedCode {
    plain(source)
}

/// Source rendered without any grammar: plain text roles.
pub(crate) fn plain(source: &str) -> HighlightedCode {
    build_lines(source, &[])
}

/// The grammar and its highlight query for a language, when compiled in.
#[cfg(not(target_arch = "wasm32"))]
fn grammar_for(language: Language) -> Option<(tree_sitter::Language, &'static str)> {
    match language {
        Language::Rust => rust_grammar(),
        Language::TypeScript => typescript_grammar(),
        Language::JavaScript => javascript_grammar(),
        Language::Python => python_grammar(),
        Language::Plain => None,
    }
}

/// The Rust grammar.
#[cfg(not(target_arch = "wasm32"))]
fn rust_grammar() -> Option<(tree_sitter::Language, &'static str)> {
    Some((
        tree_sitter_rust::LANGUAGE.into(),
        tree_sitter_rust::HIGHLIGHTS_QUERY,
    ))
}

/// The TypeScript grammar.
#[cfg(not(target_arch = "wasm32"))]
fn typescript_grammar() -> Option<(tree_sitter::Language, &'static str)> {
    Some((
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
    ))
}

/// The JavaScript grammar.
#[cfg(not(target_arch = "wasm32"))]
fn javascript_grammar() -> Option<(tree_sitter::Language, &'static str)> {
    Some((
        tree_sitter_javascript::LANGUAGE.into(),
        tree_sitter_javascript::HIGHLIGHT_QUERY,
    ))
}

/// The Python grammar.
#[cfg(not(target_arch = "wasm32"))]
fn python_grammar() -> Option<(tree_sitter::Language, &'static str)> {
    Some((
        tree_sitter_python::LANGUAGE.into(),
        tree_sitter_python::HIGHLIGHTS_QUERY,
    ))
}

/// Maps a query capture name to a role; unmapped captures render as text.
fn role_for_capture(name: &str) -> Option<Role> {
    match name.split('.').next()? {
        "keyword" => Some(Role::Keyword),
        "string" => Some(Role::String),
        "escape" => Some(Role::Escape),
        "comment" => Some(Role::Comment),
        "function" | "method" => Some(Role::Function),
        "type" | "attribute" => Some(Role::Type),
        "number" => Some(Role::Number),
        "constant" => Some(Role::Constant),
        "property" => Some(Role::Property),
        "operator" => Some(Role::Operator),
        "punctuation" => Some(Role::Punctuation),
        "tag" => Some(Role::Tag),
        _ => None,
    }
}

/// Splits `source` into lines, grouping consecutive bytes that share a role into one span.
fn build_lines(source: &str, role_at: &[Option<Role>]) -> HighlightedCode {
    let role_of =
        |byte: usize| -> Role { role_at.get(byte).copied().flatten().unwrap_or(Role::Text) };

    let mut lines = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut text = String::new();
    let mut role = Role::Text;

    for (byte, character) in source.char_indices() {
        let byte_role = role_of(byte);
        if character == '\n' {
            flush_run(&mut text, role, &mut spans);
            lines.push(Line {
                spans: std::mem::take(&mut spans),
            });
            continue;
        }
        if byte_role != role && !text.is_empty() {
            flush_run(&mut text, role, &mut spans);
        }
        role = byte_role;
        text.push(character);
    }

    flush_run(&mut text, role, &mut spans);
    lines.push(Line {
        spans: std::mem::take(&mut spans),
    });

    HighlightedCode { lines }
}

fn flush_run(text: &mut String, role: Role, spans: &mut Vec<Span>) {
    if !text.is_empty() {
        spans.push(Span {
            text: SharedString::from(std::mem::take(text)),
            role,
        });
    }
}
