//! Lightweight, dependency-free syntax highlighting.
//!
//! This is deliberately *not* a parser. It is a per-line lexer that carries a
//! tiny state across lines (block comment, triple-quoted string) so that
//! multi-line constructs stay coloured. It is:
//!
//! * **cheap** — only visible lines are lexed, ~50 lines per frame;
//! * **incremental** — start states are cached per line and invalidated from
//!   the first edited line onwards;
//! * **honest** — approximate by design. Anything that needs real grammar
//!   awareness belongs in the planned tree-sitter/LSP integration, not here.
//!
//! Tokens are expressed in character offsets relative to the line, matching the
//! rest of the editor.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

/// A syntax category. The theme maps these to colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Type,
    Constant,
    Function,
    String,
    Number,
    Comment,
    Operator,
    Punctuation,
    Attribute,
    Heading,
}

/// A coloured span inside one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub len: usize,
    pub kind: TokenKind,
}

impl Token {
    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

/// Supported languages (extension based).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    Plain,
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Json,
    Toml,
    Yaml,
    Markdown,
    Shell,
    C,
    Cpp,
    Go,
    Java,
    Html,
    Css,
    Sql,
}

impl Language {
    /// Guess from a file name.
    pub fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "rs" => Language::Rust,
            "py" | "pyi" => Language::Python,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "tsx" | "mts" => Language::TypeScript,
            "json" | "jsonc" => Language::Json,
            "toml" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "md" | "markdown" => Language::Markdown,
            "sh" | "bash" | "zsh" | "fish" => Language::Shell,
            "c" | "h" => Language::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" => Language::Cpp,
            "go" => Language::Go,
            "java" => Language::Java,
            "html" | "htm" => Language::Html,
            "css" | "scss" => Language::Css,
            "sql" => Language::Sql,
            _ => match name.as_str() {
                "makefile" | "dockerfile" => Language::Shell,
                ".gitignore" | ".env" => Language::Plain,
                _ => Language::Plain,
            },
        }
    }

    /// Human readable name for the status bar.
    pub fn name(self) -> &'static str {
        match self {
            Language::Plain => "Plain Text",
            Language::Rust => "Rust",
            Language::Python => "Python",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Json => "JSON",
            Language::Toml => "TOML",
            Language::Yaml => "YAML",
            Language::Markdown => "Markdown",
            Language::Shell => "Shell",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Go => "Go",
            Language::Java => "Java",
            Language::Html => "HTML",
            Language::Css => "CSS",
            Language::Sql => "SQL",
        }
    }

    /// True when highlighting will produce anything useful.
    pub fn has_highlighting(self) -> bool {
        self != Language::Plain
    }
}

/// Cross-line lexer state.
pub type LexState = u8;

const ST_NORMAL: LexState = 0;
const ST_BLOCK_COMMENT: LexState = 1;
const ST_TRIPLE_SINGLE: LexState = 2;
const ST_TRIPLE_DOUBLE: LexState = 3;

struct Spec {
    line_comments: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    strings: &'static [char],
    triple_quotes: bool,
}

fn set(items: &'static [&'static str]) -> HashSet<&'static str> {
    items.iter().copied().collect()
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "type", "unsafe", "use",
    "where", "while", "yield",
];
const RUST_TYPES: &[&str] = &[
    "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8", "u16",
    "u32", "u64", "u128", "usize", "String", "Vec", "Option", "Result", "Box", "Arc", "Rc",
    "HashMap", "BTreeMap", "HashSet", "Cow", "Path", "PathBuf", "Duration", "Instant",
];
const RUST_CONSTANTS: &[&str] = &["true", "false", "None", "Some", "Ok", "Err"];

const PY_KEYWORDS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
    "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "match",
    "case",
];
const PY_TYPES: &[&str] = &[
    "int",
    "float",
    "str",
    "bool",
    "bytes",
    "list",
    "dict",
    "set",
    "tuple",
    "object",
    "type",
    "Exception",
    "ValueError",
    "NoneType",
    "Optional",
    "List",
    "Dict",
    "Any",
];
const PY_CONSTANTS: &[&str] = &["True", "False", "None", "self", "cls", "NotImplemented"];

const JS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "interface",
    "type",
    "enum",
    "implements",
    "readonly",
    "namespace",
    "declare",
    "abstract",
    "satisfies",
    "as",
];
const JS_TYPES: &[&str] = &[
    "string", "number", "boolean", "any", "unknown", "never", "object", "symbol", "bigint",
    "Promise", "Array", "Record", "Partial", "Map", "Set", "Date", "Error",
];
const JS_CONSTANTS: &[&str] = &["true", "false", "null", "undefined", "NaN", "Infinity"];

const C_KEYWORDS: &[&str] = &[
    "auto",
    "break",
    "case",
    "const",
    "continue",
    "default",
    "do",
    "else",
    "enum",
    "extern",
    "for",
    "goto",
    "if",
    "inline",
    "register",
    "restrict",
    "return",
    "sizeof",
    "static",
    "struct",
    "switch",
    "typedef",
    "union",
    "volatile",
    "while",
    "class",
    "namespace",
    "template",
    "public",
    "private",
    "protected",
    "virtual",
    "override",
    "using",
    "new",
    "delete",
    "try",
    "catch",
    "throw",
    "constexpr",
    "noexcept",
    "nullptr_t",
    "auto",
];
const C_TYPES: &[&str] = &[
    "bool", "char", "double", "float", "int", "long", "short", "signed", "unsigned", "void",
    "size_t", "ssize_t", "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t",
    "uint32_t", "uint64_t", "string", "vector", "std", "FILE",
];
const C_CONSTANTS: &[&str] = &["NULL", "true", "false", "nullptr", "EOF"];

const GO_KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];
const GO_TYPES: &[&str] = &[
    "bool",
    "byte",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
    "any",
];
const GO_CONSTANTS: &[&str] = &["true", "false", "nil", "iota"];

const JAVA_KEYWORDS: &[&str] = &[
    "abstract",
    "assert",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "for",
    "if",
    "implements",
    "import",
    "instanceof",
    "interface",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "volatile",
    "while",
    "record",
    "var",
];
const JAVA_TYPES: &[&str] = &[
    "boolean", "byte", "char", "double", "float", "int", "long", "short", "void", "String",
    "Object", "List", "Map", "Set", "Integer", "Double", "Boolean",
];
const JAVA_CONSTANTS: &[&str] = &["true", "false", "null"];

const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "function", "in", "return", "local", "export", "readonly", "shift", "source", "eval", "exec",
    "set", "unset", "trap", "wait",
];

const SQL_KEYWORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "table",
    "drop",
    "alter",
    "add",
    "column",
    "index",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "on",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "offset",
    "as",
    "and",
    "or",
    "not",
    "null",
    "primary",
    "key",
    "foreign",
    "references",
    "distinct",
    "union",
    "case",
    "when",
    "then",
    "end",
];
const SQL_TYPES: &[&str] = &[
    "int",
    "integer",
    "text",
    "varchar",
    "char",
    "boolean",
    "date",
    "timestamp",
    "numeric",
    "decimal",
    "real",
    "blob",
];
const SQL_CONSTANTS: &[&str] = &["true", "false", "null"];

fn spec(lang: Language) -> Option<&'static Spec> {
    macro_rules! leak {
        ($name:ident, $spec:expr) => {{
            static CELL: OnceLock<Spec> = OnceLock::new();
            Some(CELL.get_or_init(|| $spec))
        }};
    }
    match lang {
        Language::Rust => leak!(
            RUST,
            Spec {
                line_comments: &["//"],
                block: Some(("/*", "*/")),
                strings: &['"'],
                triple_quotes: false,
            }
        ),
        Language::Python => leak!(
            PY,
            Spec {
                line_comments: &["#"],
                block: None,
                strings: &['"', '\''],
                triple_quotes: true,
            }
        ),
        Language::JavaScript | Language::TypeScript => leak!(
            JS,
            Spec {
                line_comments: &["//"],
                block: Some(("/*", "*/")),
                strings: &['"', '\'', '`'],
                triple_quotes: false,
            }
        ),
        Language::Json => leak!(
            JSON,
            Spec {
                line_comments: &["//"],
                block: Some(("/*", "*/")),
                strings: &['"'],
                triple_quotes: false,
            }
        ),
        Language::Toml | Language::Yaml => leak!(
            CFG,
            Spec {
                line_comments: &["#"],
                block: None,
                strings: &['"', '\''],
                triple_quotes: false,
            }
        ),
        Language::Shell => leak!(
            SH,
            Spec {
                line_comments: &["#"],
                block: None,
                strings: &['"', '\''],
                triple_quotes: false,
            }
        ),
        Language::C | Language::Cpp => leak!(
            C,
            Spec {
                line_comments: &["//"],
                block: Some(("/*", "*/")),
                strings: &['"', '\''],
                triple_quotes: false,
            }
        ),
        Language::Go => leak!(
            GO,
            Spec {
                line_comments: &["//"],
                block: Some(("/*", "*/")),
                strings: &['"', '\'', '`'],
                triple_quotes: false,
            }
        ),
        Language::Java => leak!(
            JAVA,
            Spec {
                line_comments: &["//"],
                block: Some(("/*", "*/")),
                strings: &['"', '\''],
                triple_quotes: false,
            }
        ),
        Language::Sql => leak!(
            SQL,
            Spec {
                line_comments: &["--"],
                block: Some(("/*", "*/")),
                strings: &['\'', '"'],
                triple_quotes: false,
            }
        ),
        // CSS and HTML share the C family shape closely enough for a
        // lightweight highlighter.
        Language::Css | Language::Html => leak!(
            MARKUP,
            Spec {
                line_comments: &[],
                block: Some(("/*", "*/")),
                strings: &['"', '\''],
                triple_quotes: false,
            }
        ),
        Language::Markdown | Language::Plain => None,
    }
}

fn keyword_set(lang: Language) -> Option<&'static HashSet<&'static str>> {
    macro_rules! cache {
        ($cell:ident, $items:expr) => {{
            static $cell: OnceLock<HashSet<&'static str>> = OnceLock::new();
            Some($cell.get_or_init(|| set($items)))
        }};
    }
    match lang {
        Language::Rust => cache!(K, RUST_KEYWORDS),
        Language::Python => cache!(P, PY_KEYWORDS),
        Language::JavaScript | Language::TypeScript => cache!(J, JS_KEYWORDS),
        Language::Shell => cache!(S, SHELL_KEYWORDS),
        Language::C | Language::Cpp => cache!(C, C_KEYWORDS),
        Language::Go => cache!(G, GO_KEYWORDS),
        Language::Java => cache!(JV, JAVA_KEYWORDS),
        Language::Sql => cache!(Q, SQL_KEYWORDS),
        _ => None,
    }
}

fn type_set(lang: Language) -> Option<&'static HashSet<&'static str>> {
    macro_rules! cache {
        ($cell:ident, $items:expr) => {{
            static $cell: OnceLock<HashSet<&'static str>> = OnceLock::new();
            Some($cell.get_or_init(|| set($items)))
        }};
    }
    match lang {
        Language::Rust => cache!(K, RUST_TYPES),
        Language::Python => cache!(P, PY_TYPES),
        Language::JavaScript | Language::TypeScript => cache!(J, JS_TYPES),
        Language::C | Language::Cpp => cache!(C, C_TYPES),
        Language::Go => cache!(G, GO_TYPES),
        Language::Java => cache!(JV, JAVA_TYPES),
        Language::Sql => cache!(Q, SQL_TYPES),
        _ => None,
    }
}

fn constant_set(lang: Language) -> Option<&'static HashSet<&'static str>> {
    macro_rules! cache {
        ($cell:ident, $items:expr) => {{
            static $cell: OnceLock<HashSet<&'static str>> = OnceLock::new();
            Some($cell.get_or_init(|| set($items)))
        }};
    }
    match lang {
        Language::Rust => cache!(K, RUST_CONSTANTS),
        Language::Python => cache!(P, PY_CONSTANTS),
        Language::JavaScript | Language::TypeScript | Language::Json => cache!(J, JS_CONSTANTS),
        Language::C | Language::Cpp => cache!(C, C_CONSTANTS),
        Language::Go => cache!(G, GO_CONSTANTS),
        Language::Java => cache!(JV, JAVA_CONSTANTS),
        Language::Sql => cache!(Q, SQL_CONSTANTS),
        Language::Toml | Language::Yaml => cache!(T, &["true", "false"]),
        _ => None,
    }
}

fn starts_with_at(chars: &[char], at: usize, pat: &str) -> bool {
    let mut i = at;
    for c in pat.chars() {
        if i >= chars.len() || chars[i] != c {
            return false;
        }
        i += 1;
    }
    true
}

fn ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Lex one line, returning tokens (character offsets) and the state carried to
/// the next line.
pub fn lex_line(lang: Language, line: &str, state: LexState) -> (Vec<Token>, LexState) {
    if lang == Language::Plain {
        return (Vec::new(), ST_NORMAL);
    }
    if lang == Language::Markdown {
        return (lex_markdown(line), ST_NORMAL);
    }
    let Some(spec) = spec(lang) else {
        return (Vec::new(), ST_NORMAL);
    };
    let keywords = keyword_set(lang);
    let types = type_set(lang);
    let constants = constant_set(lang);

    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut tokens: Vec<Token> = Vec::new();
    let mut i = 0usize;
    let mut st = state;

    // Continue a multi-line construct from the previous line.
    if st == ST_BLOCK_COMMENT {
        if let Some((_, end)) = spec.block {
            let mut j = 0usize;
            while j < n && !starts_with_at(&chars, j, end) {
                j += 1;
            }
            if j < n {
                let k = j + end.chars().count();
                tokens.push(Token {
                    start: 0,
                    len: k,
                    kind: TokenKind::Comment,
                });
                i = k;
                st = ST_NORMAL;
            } else {
                tokens.push(Token {
                    start: 0,
                    len: n,
                    kind: TokenKind::Comment,
                });
                return (tokens, ST_BLOCK_COMMENT);
            }
        } else {
            st = ST_NORMAL;
        }
    } else if st == ST_TRIPLE_SINGLE || st == ST_TRIPLE_DOUBLE {
        let delim: Vec<char> = if st == ST_TRIPLE_SINGLE {
            vec!['\'', '\'', '\'']
        } else {
            vec!['"', '"', '"']
        };
        let mut j = 0usize;
        let mut found = None;
        while j + 2 < n + 1 {
            if j + 2 < n
                && chars[j] == delim[0]
                && chars[j + 1] == delim[1]
                && chars[j + 2] == delim[2]
            {
                found = Some(j + 3);
                break;
            }
            if j >= n {
                break;
            }
            j += 1;
        }
        match found {
            Some(k) => {
                tokens.push(Token {
                    start: 0,
                    len: k,
                    kind: TokenKind::String,
                });
                i = k;
                st = ST_NORMAL;
            }
            None => {
                tokens.push(Token {
                    start: 0,
                    len: n,
                    kind: TokenKind::String,
                });
                return (tokens, st);
            }
        }
    }

    while i < n {
        let c = chars[i];

        // Whitespace
        if c.is_whitespace() {
            i += 1;
            continue;
        }

        // Line comments
        if let Some(marker) = spec
            .line_comments
            .iter()
            .find(|m| starts_with_at(&chars, i, m))
        {
            let _ = marker;
            tokens.push(Token {
                start: i,
                len: n - i,
                kind: TokenKind::Comment,
            });
            return (tokens, ST_NORMAL);
        }

        // Block comments
        if let Some((start, end)) = spec.block {
            if starts_with_at(&chars, i, start) {
                let mut j = i + start.chars().count();
                let mut closed = false;
                while j < n {
                    if starts_with_at(&chars, j, end) {
                        j += end.chars().count();
                        closed = true;
                        break;
                    }
                    j += 1;
                }
                tokens.push(Token {
                    start: i,
                    len: j - i,
                    kind: TokenKind::Comment,
                });
                i = j;
                if !closed {
                    return (tokens, ST_BLOCK_COMMENT);
                }
                continue;
            }
        }

        // Triple quoted strings (Python)
        if spec.triple_quotes && (c == '"' || c == '\'') {
            let triple = [c, c, c];
            if i + 2 < n && chars[i + 1] == triple[1] && chars[i + 2] == triple[2] {
                let mut j = i + 3;
                let mut closed = false;
                while j + 2 < n {
                    if chars[j] == c && chars[j + 1] == c && chars[j + 2] == c {
                        j += 3;
                        closed = true;
                        break;
                    }
                    j += 1;
                }
                tokens.push(Token {
                    start: i,
                    len: j - i,
                    kind: TokenKind::String,
                });
                i = j;
                if !closed {
                    return (
                        tokens,
                        if c == '\'' {
                            ST_TRIPLE_SINGLE
                        } else {
                            ST_TRIPLE_DOUBLE
                        },
                    );
                }
                continue;
            }
        }

        // Strings
        if spec.strings.contains(&c) {
            let quote = c;
            let mut j = i + 1;
            while j < n {
                let ch = chars[j];
                if ch == '\\' {
                    j += 2;
                    continue;
                }
                if ch == quote {
                    j += 1;
                    break;
                }
                j += 1;
            }
            let end = j.min(n);
            tokens.push(Token {
                start: i,
                len: end - i,
                kind: TokenKind::String,
            });
            i = end;
            continue;
        }

        // Numbers
        if c.is_ascii_digit() {
            let mut j = i;
            while j < n
                && (chars[j].is_ascii_alphanumeric()
                    || chars[j] == '.'
                    || chars[j] == '_'
                    || ((chars[j] == '+' || chars[j] == '-')
                        && j > i
                        && matches!(chars[j - 1], 'e' | 'E')))
            {
                j += 1;
            }
            tokens.push(Token {
                start: i,
                len: j - i,
                kind: TokenKind::Number,
            });
            i = j;
            continue;
        }

        // Identifiers / keywords
        if ident_start(c) {
            let mut j = i + 1;
            while j < n && ident_continue(chars[j]) {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let kind = if constants
                .map(|s| s.contains(word.as_str()))
                .unwrap_or(false)
            {
                TokenKind::Constant
            } else if keywords.map(|s| s.contains(word.as_str())).unwrap_or(false) {
                TokenKind::Keyword
            } else if types.map(|s| s.contains(word.as_str())).unwrap_or(false) {
                TokenKind::Type
            } else {
                // A name immediately followed by '(' is a call.
                let mut k = j;
                while k < n && chars[k] == ' ' {
                    k += 1;
                }
                if k < n && chars[k] == '(' {
                    TokenKind::Function
                } else if word
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                    && lang == Language::Rust
                {
                    TokenKind::Type
                } else {
                    TokenKind::Attribute
                }
            };
            tokens.push(Token {
                start: i,
                len: j - i,
                kind,
            });
            i = j;
            continue;
        }

        // Operators and punctuation
        if "+-*/%=<>!&|^~?:".contains(c) {
            let mut j = i + 1;
            while j < n && "+-*/%=<>!&|^~?:".contains(chars[j]) {
                j += 1;
            }
            tokens.push(Token {
                start: i,
                len: j - i,
                kind: TokenKind::Operator,
            });
            i = j;
            continue;
        }

        if "(){}[];,.".contains(c) {
            tokens.push(Token {
                start: i,
                len: 1,
                kind: TokenKind::Punctuation,
            });
            i += 1;
            continue;
        }

        i += 1;
    }

    (tokens, st)
}

/// Very small Markdown highlighter: headings, code fences, inline code,
/// blockquotes and list bullets.
fn lex_markdown(line: &str) -> Vec<Token> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < n && chars[i] == ' ' {
        i += 1;
    }
    if i < n && chars[i] == '#' {
        tokens.push(Token {
            start: i,
            len: n - i,
            kind: TokenKind::Heading,
        });
        return tokens;
    }
    if i < n && (chars[i] == '>' || chars[i] == '-' || chars[i] == '*' || chars[i] == '+') {
        tokens.push(Token {
            start: i,
            len: 1,
            kind: TokenKind::Keyword,
        });
    }
    // Inline code spans.
    let mut idx = 0usize;
    while idx < n {
        if chars[idx] == '`' {
            let mut j = idx + 1;
            while j < n && chars[j] != '`' {
                j += 1;
            }
            let end = (j + 1).min(n);
            tokens.push(Token {
                start: idx,
                len: end - idx,
                kind: TokenKind::String,
            });
            idx = end;
            continue;
        }
        idx += 1;
    }
    tokens
}

/// Cache of per-line start states so scrolling does not re-lex the whole file.
#[derive(Debug, Default)]
pub struct StateCache {
    states: Vec<LexState>,
    /// First line whose cached state is known to be correct.
    valid_from: usize,
    /// Number of lines covered by `states`.
    len: usize,
    language: Option<Language>,
    revision: u64,
}

impl StateCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Invalidate from `line` onwards (called after an edit).
    pub fn invalidate_from(&mut self, line: usize) {
        if line < self.valid_from {
            self.valid_from = line;
        }
        self.len = self.len.min(line);
    }

    /// Reset the cache entirely (language change, reload).
    pub fn reset(&mut self, language: Language) {
        self.states.clear();
        self.valid_from = 0;
        self.len = 0;
        self.language = Some(language);
        self.revision = 0;
    }

    /// Start state for `line`, lexing forward as needed.
    pub fn state_for(
        &mut self,
        line: usize,
        lines: &mut dyn FnMut(usize) -> String,
        language: Language,
    ) -> LexState {
        if self.language != Some(language) {
            self.reset(language);
        }
        if line < self.valid_from {
            self.valid_from = line;
        }
        if line == 0 {
            return ST_NORMAL;
        }
        // Grow the state vector up to `line`.
        while self.states.len() < line {
            let idx = self.states.len();
            let text = lines(idx);
            let start = if idx == 0 {
                ST_NORMAL
            } else {
                self.states.get(idx - 1).copied().unwrap_or(ST_NORMAL)
            };
            let (_, end) = lex_line(language, &text, start);
            self.states.push(end);
        }
        self.states.get(line - 1).copied().unwrap_or(ST_NORMAL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: Language, line: &str) -> Vec<(TokenKind, String)> {
        let (tokens, _) = lex_line(lang, line, ST_NORMAL);
        let chars: Vec<char> = line.chars().collect();
        tokens
            .iter()
            .map(|t| (t.kind, chars[t.start..t.end()].iter().collect::<String>()))
            .collect()
    }

    #[test]
    fn detects_language_from_extension() {
        assert_eq!(Language::from_path(Path::new("a.rs")), Language::Rust);
        assert_eq!(Language::from_path(Path::new("a.PY")), Language::Python);
        assert_eq!(
            Language::from_path(Path::new("a.unknownext")),
            Language::Plain
        );
    }

    #[test]
    fn rust_keywords_and_strings() {
        let out = kinds(Language::Rust, "let x = \"hi\";");
        assert!(out.contains(&(TokenKind::Keyword, "let".into())));
        assert!(out.contains(&(TokenKind::String, "\"hi\"".into())));
    }

    #[test]
    fn comments_run_to_end_of_line() {
        let out = kinds(Language::Rust, "let x = 1; // note");
        assert!(out.contains(&(TokenKind::Comment, "// note".into())));
    }

    #[test]
    fn block_comment_state_carries_over() {
        let (_, st) = lex_line(Language::Rust, "/* start", ST_NORMAL);
        assert_eq!(st, ST_BLOCK_COMMENT);
        let (tokens, st2) = lex_line(Language::Rust, " still */ let x = 1;", st);
        assert_eq!(st2, ST_NORMAL);
        assert_eq!(tokens[0].kind, TokenKind::Comment);
    }

    #[test]
    fn python_triple_quotes_span_lines() {
        let (_, st) = lex_line(Language::Python, "x = \"\"\"doc", ST_NORMAL);
        assert_eq!(st, ST_TRIPLE_DOUBLE);
        let (tokens, st2) = lex_line(Language::Python, "more\"\"\"", st);
        assert_eq!(st2, ST_NORMAL);
        assert_eq!(tokens[0].kind, TokenKind::String);
    }

    #[test]
    fn numbers_and_functions() {
        let out = kinds(Language::Rust, "foo(42);");
        assert!(out.contains(&(TokenKind::Function, "foo".into())));
        assert!(out.contains(&(TokenKind::Number, "42".into())));
    }

    #[test]
    fn markdown_headings() {
        let out = kinds(Language::Markdown, "## Title");
        assert_eq!(out[0].0, TokenKind::Heading);
    }

    #[test]
    fn plain_text_is_not_lexed() {
        let (tokens, _) = lex_line(Language::Plain, "let x = 1;", ST_NORMAL);
        assert!(tokens.is_empty());
    }

    #[test]
    fn tokens_never_overlap_and_stay_in_bounds() {
        let line = "fn main() { let s = \"a\\\"b\"; } // tail";
        let (tokens, _) = lex_line(Language::Rust, line, ST_NORMAL);
        let n = line.chars().count();
        let mut prev_end = 0;
        for t in &tokens {
            assert!(t.start >= prev_end, "overlapping tokens");
            assert!(t.end() <= n, "token out of bounds");
            prev_end = t.end();
        }
    }
}
