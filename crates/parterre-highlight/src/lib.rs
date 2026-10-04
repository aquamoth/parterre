//! Syntax colour for the diff and blame windows (#209), in a crate of its own so that the
//! engine's dependencies live here alone (#214).
//!
//! Behind the `syntax` feature, tree-sitter parses a whole version of a file and hands back
//! language-neutral spans: what each piece of a line is ([`Kind`]), never a colour. The app
//! maps a kind to a colour of its palette. A whole file at a time, so that block comments,
//! strings and fenced code that span lines keep their state wherever a window starts
//! showing them. Without the feature, [`language_of`] knows no language and nothing is
//! coloured.
//!
//! One grammar crate per language, each compiling its parser (C) in its build script. The
//! grammars' own `highlights.scm` queries name what they find (`keyword`, `string.escape`,
//! `text.title`, …), each in its own dialect; [`engine::NAMES`] maps those names onto the
//! dozen kinds here.

use parterre_util::{Cancel, Poll};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a span of text is, for colouring. Deliberately coarse: a dozen kinds that every
/// grammar's captures map onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Comment,
    /// A string literal, quotes included.
    String,
    /// An escape sequence inside a string.
    Escape,
    Keyword,
    /// A type, class, interface, namespace or module name.
    Type,
    /// A function, method or macro, defined or called.
    Function,
    Number,
    /// `true`, `null`, `self`, an enum member, a defined constant.
    Constant,
    Operator,
    Punctuation,
    Variable,
    /// A field, property or key (JSON, YAML, TOML, CSS).
    Property,
    /// An attribute name in HTML and XML, an attribute in Rust and C#.
    Attribute,
    /// An HTML, XML or JSX tag name.
    Tag,
    /// Markdown: a heading, emphasis, strong emphasis, inline or block code, a link.
    Heading,
    Emphasis,
    Strong,
    Raw,
    Link,
}

/// A line's coloured spans: byte ranges of the line (without its ending), in order, not
/// overlapping. Where a grammar is nested in another (Rust in a Markdown fence, JavaScript
/// in HTML), the innermost grammar's kind wins.
pub type Spans = Vec<(Range<usize>, Kind)>;

/// A language with a grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Bash,
    C,
    Cpp,
    CSharp,
    Css,
    Dockerfile,
    Go,
    Html,
    Java,
    JavaScript,
    Jsx,
    Json,
    Makefile,
    Markdown,
    Proto,
    Python,
    Rust,
    Sql,
    Toml,
    TypeScript,
    Tsx,
    Xml,
    Yaml,
}

impl Language {
    pub const ALL: [Language; 23] = [
        Language::Bash,
        Language::C,
        Language::Cpp,
        Language::CSharp,
        Language::Css,
        Language::Dockerfile,
        Language::Go,
        Language::Html,
        Language::Java,
        Language::JavaScript,
        Language::Jsx,
        Language::Json,
        Language::Makefile,
        Language::Markdown,
        Language::Proto,
        Language::Python,
        Language::Rust,
        Language::Sql,
        Language::Toml,
        Language::TypeScript,
        Language::Tsx,
        Language::Xml,
        Language::Yaml,
    ];

    /// The name the child process and the injection queries know a language by.
    pub fn id(self) -> &'static str {
        match self {
            Language::Bash => "bash",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::CSharp => "c_sharp",
            Language::Css => "css",
            Language::Dockerfile => "dockerfile",
            Language::Go => "go",
            Language::Html => "html",
            Language::Java => "java",
            Language::JavaScript => "javascript",
            Language::Jsx => "jsx",
            Language::Json => "json",
            Language::Makefile => "make",
            Language::Markdown => "markdown",
            Language::Proto => "proto",
            Language::Python => "python",
            Language::Rust => "rust",
            Language::Sql => "sql",
            Language::Toml => "toml",
            Language::TypeScript => "typescript",
            Language::Tsx => "tsx",
            Language::Xml => "xml",
            Language::Yaml => "yaml",
        }
    }

    pub fn from_id(id: &str) -> Option<Language> {
        Language::ALL.into_iter().find(|l| l.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Bash => "Shell",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::CSharp => "C#",
            Language::Css => "CSS",
            Language::Dockerfile => "Dockerfile",
            Language::Go => "Go",
            Language::Html => "HTML",
            Language::Java => "Java",
            Language::JavaScript => "JavaScript",
            Language::Jsx => "JSX",
            Language::Json => "JSON",
            Language::Makefile => "Makefile",
            Language::Markdown => "Markdown",
            Language::Proto => "Protocol Buffers",
            Language::Python => "Python",
            Language::Rust => "Rust",
            Language::Sql => "SQL",
            Language::Toml => "TOML",
            Language::TypeScript => "TypeScript",
            Language::Tsx => "TSX",
            Language::Xml => "XML",
            Language::Yaml => "YAML",
        }
    }
}

/// Files above this size are left plain: parsing is fast, but the tree of a huge generated
/// file costs hundreds of megabytes, and nobody reads one for its colours.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Highlighting a file takes at most this long; then its child process is killed and the
/// file stays plain. Error recovery in a grammar can go super-linear on odd input.
pub const BUDGET: Duration = Duration::from_secs(10);

/// The language of a file, from its path: by file name first (`Dockerfile`, `Makefile`,
/// `Cargo.lock`), then by extension. `None` for a file the engine can't colour, or when the
/// `syntax` feature is off.
pub fn language_of(path: &str) -> Option<Language> {
    if !cfg!(feature = "syntax") {
        return None;
    }
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    by_name(name).or_else(|| {
        let (_, ext) = name.rsplit_once('.')?;
        by_extension(&ext.to_ascii_lowercase())
    })
}

fn by_name(name: &str) -> Option<Language> {
    use Language::*;
    Some(match name {
        "Dockerfile" | "Containerfile" => Dockerfile,
        "Makefile" | "makefile" | "GNUmakefile" => Makefile,
        "Cargo.lock" | "Pipfile" | "poetry.lock" | "uv.lock" => Toml,
        ".bashrc" | ".bash_profile" | ".bash_aliases" | ".zshrc" | ".profile" => Bash,
        _ if name.starts_with("Dockerfile.") || name.starts_with("Containerfile.") => Dockerfile,
        _ => return None,
    })
}

fn by_extension(ext: &str) -> Option<Language> {
    use Language::*;
    Some(match ext {
        "sh" | "bash" | "zsh" | "ksh" => Bash,
        "c" => C,
        // `.h` as C++, as VS Code takes it: the grammar reads C too.
        "cc" | "cpp" | "cxx" | "c++" | "h" | "hh" | "hpp" | "hxx" | "h++" | "inl" | "ipp" => Cpp,
        "cs" | "csx" => CSharp,
        "css" => Css,
        "dockerfile" | "containerfile" => Dockerfile,
        "go" => Go,
        "html" | "htm" | "xhtml" => Html,
        "java" => Java,
        "js" | "mjs" | "cjs" => JavaScript,
        "jsx" => Jsx,
        "json" | "jsonc" | "webmanifest" => Json,
        "mk" | "make" => Makefile,
        "md" | "markdown" | "mdown" => Markdown,
        "proto" => Proto,
        "py" | "pyi" | "pyw" => Python,
        "rs" => Rust,
        "sql" | "ddl" | "dml" | "psql" | "mysql" | "tsql" => Sql,
        "toml" => Toml,
        "ts" | "mts" | "cts" => TypeScript,
        "tsx" => Tsx,
        "xml" | "xsd" | "xsl" | "xslt" | "svg" | "rng" | "csproj" | "vbproj" | "fsproj"
        | "props" | "targets" | "resx" | "nuspec" | "xaml" | "wsdl" | "plist" | "pom" => Xml,
        "yaml" | "yml" => Yaml,
        _ => return None,
    })
}

/// Highlights a whole file (`text`, line endings included) as `language`: the spans of each
/// line, in order. Byte ranges are of the line without its ending, so a span that runs over
/// several lines (a block comment) is cut into one piece per line. Empty for a file over
/// [`MAX_BYTES`], or one whose grammar failed to load.
pub fn highlight(language: Language, text: &str) -> Vec<Spans> {
    #[cfg(feature = "syntax")]
    {
        engine::highlight(language, text)
    }
    #[cfg(not(feature = "syntax"))]
    {
        let _ = (language, text);
        Vec::new()
    }
}

/// Where highlighting runs. The grammars are C, and one of them aborting on a file it can't
/// take (deeply nested YAML or Markdown overrun their scanners' state buffer today, and the
/// runtime's assertion then aborts the process) or parsing for minutes must not take the
/// window with it. So the app runs them in a child process: itself, as
/// `parterre --highlight LANG`, the text on stdin and the spans on stdout, killed when the
/// window closes or [`BUDGET`] runs out. In this process is for that child, and for tests;
/// off is for an app that doesn't know its own program.
#[derive(Clone, Debug, Default)]
pub enum Engine {
    #[default]
    InProcess,
    Child(PathBuf),
    Off,
}

impl Engine {
    /// [`highlight`], run as the engine says. Empty when the child failed, was killed, or
    /// `cancel` stopped it.
    pub fn highlight(&self, language: Language, text: &str, cancel: &Cancel) -> Vec<Spans> {
        match self {
            Engine::InProcess => highlight(language, text),
            Engine::Child(exe) => in_child(exe, language, text, cancel, BUDGET).unwrap_or_default(),
            Engine::Off => Vec::new(),
        }
    }
}

/// Runs `exe --highlight LANG` on `text`, killing it once `cancel` says so or `budget` is
/// spent. `None` when the child could not be run, failed, or was killed. The child is held
/// by `cancel` while it runs, as git's commands are, so cancelling kills it at once, and a
/// window's drop (the app's exit too) leaves none behind.
pub fn in_child(
    exe: &Path,
    language: Language,
    text: &str,
    cancel: &Cancel,
    budget: Duration,
) -> Option<Vec<Spans>> {
    if text.len() > MAX_BYTES || cancel.is_cancelled() {
        return None;
    }
    let mut cmd = Command::new(exe);
    cmd.arg("--highlight")
        .arg(language.id())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        // A GUI-subsystem app on Windows; without this the child would flash a console.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let mut stdout = child.stdout.take()?;
    cancel.hold(child).ok()?;
    let start = Instant::now();
    let mut out = Vec::new();
    let finished = std::thread::scope(|s| {
        // The text goes in from a thread of its own, so a full pipe can't deadlock against
        // the output; the watchdog reaps the child, or kills it when time is up.
        s.spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
        let watchdog = s.spawn(move || watch(cancel, start, budget));
        let read = stdout.read_to_end(&mut out).is_ok();
        read && watchdog.join().unwrap_or(false)
    });
    if !finished {
        return None;
    }
    decode(&out)
}

/// Waits for the child `cancel` holds, killing it once over budget; `cancel` itself kills
/// it when cancelled. True if it exited well.
fn watch(cancel: &Cancel, start: Instant, budget: Duration) -> bool {
    loop {
        match cancel.poll() {
            // Killed by `cancel()`, or unreadable and killed.
            Poll::Gone => return false,
            Poll::Exited(status) => return status.success(),
            Poll::Running if start.elapsed() <= budget => {}
            Poll::Running => {
                cancel.kill();
                return false;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The child's side of [`Engine::Child`]: highlights stdin as the language `id` names and
/// writes the spans to stdout. The exit code: 0, or 2 for an unknown language or unreadable
/// input.
pub fn serve(id: &str) -> u8 {
    let Some(language) = Language::from_id(id) else {
        return 2;
    };
    let mut text = String::new();
    if std::io::stdin().read_to_string(&mut text).is_err() {
        return 2;
    }
    let spans = highlight(language, &text);
    let out = std::io::stdout().lock();
    if serde_json::to_writer(out, &compact(&spans)).is_err() {
        return 2;
    }
    0
}

/// The spans as the child writes them: a line's `(start, end, kind)` triples.
type Compact = Vec<Vec<(usize, usize, Kind)>>;

fn compact(spans: &[Spans]) -> Compact {
    spans
        .iter()
        .map(|line| line.iter().map(|(r, k)| (r.start, r.end, *k)).collect())
        .collect()
}

fn decode(bytes: &[u8]) -> Option<Vec<Spans>> {
    let compact: Compact = serde_json::from_slice(bytes).ok()?;
    Some(
        compact
            .into_iter()
            .map(|line| line.into_iter().map(|(a, b, k)| (a..b, k)).collect())
            .collect(),
    )
}

#[cfg(feature = "syntax")]
mod engine {
    use super::{Kind, Language, MAX_BYTES, Spans};
    use std::sync::OnceLock;
    use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

    /// The Protocol Buffers highlight query, which its crate doesn't export.
    const PROTO_HIGHLIGHTS: &str = include_str!("proto_highlights.scm");

    /// The capture names recognised, and what each means. `configure` maps a grammar's
    /// capture to the longest recognised dotted prefix, so `keyword.operator` counts as
    /// `keyword`, `function.call` as `function` and `string.special.key` as itself. Every
    /// grammar is configured with this same list, as the highlighter requires for the
    /// indices to agree where one grammar is injected into another.
    pub(super) const NAMES: &[(&str, Kind)] = &[
        ("attribute", Kind::Attribute),
        ("boolean", Kind::Constant),
        // CSS at-rules, captured by their own names.
        ("charset", Kind::Keyword),
        ("import", Kind::Keyword),
        ("keyframes", Kind::Keyword),
        ("media", Kind::Keyword),
        ("namespace", Kind::Keyword),
        ("supports", Kind::Keyword),
        ("comment", Kind::Comment),
        // nvim-treesitter's older names, used by the Makefile and SQL grammars.
        ("conditional", Kind::Keyword),
        ("exception", Kind::Keyword),
        ("include", Kind::Keyword),
        ("repeat", Kind::Keyword),
        ("storageclass", Kind::Keyword),
        ("constant", Kind::Constant),
        ("constructor", Kind::Type),
        ("delimiter", Kind::Punctuation),
        ("escape", Kind::Escape),
        ("field", Kind::Property),
        ("float", Kind::Number),
        ("function", Kind::Function),
        ("keyword", Kind::Keyword),
        ("label", Kind::Keyword),
        ("markup.bold", Kind::Strong),
        ("markup.heading", Kind::Heading),
        ("markup.italic", Kind::Emphasis),
        ("markup.link", Kind::Link),
        ("markup.raw", Kind::Raw),
        ("module", Kind::Type),
        ("number", Kind::Number),
        ("operator", Kind::Operator),
        ("parameter", Kind::Variable),
        ("property", Kind::Property),
        ("punctuation", Kind::Punctuation),
        ("string", Kind::String),
        ("string.escape", Kind::Escape),
        ("string.special.key", Kind::Property),
        ("string.special.symbol", Kind::String),
        ("tag", Kind::Tag),
        // Markdown's grammar names its markup `text.*`; the Makefile grammar marks
        // TODO-like words in comments.
        ("text.danger", Kind::Comment),
        ("text.emphasis", Kind::Emphasis),
        ("text.literal", Kind::Raw),
        ("text.note", Kind::Comment),
        ("text.reference", Kind::Link),
        ("text.strong", Kind::Strong),
        ("text.title", Kind::Heading),
        ("text.uri", Kind::Link),
        ("text.warning", Kind::Comment),
        ("type", Kind::Type),
        ("type.qualifier", Kind::Keyword),
        ("variable", Kind::Variable),
        ("variable.builtin", Kind::Constant),
        ("variable.member", Kind::Property),
    ];

    /// What the recognised names mean when `language` is the file's: a grammar's capture
    /// can mean something else than the shared name says.
    fn kinds(language: Language) -> Vec<Kind> {
        NAMES
            .iter()
            .map(|&(name, kind)| match (name, language) {
                // Rust's `#[derive(Debug)]` and C#'s `[Fact]` are macros and types, not
                // the attribute names of HTML.
                ("attribute", Language::Rust) => Kind::Function,
                ("attribute", Language::CSharp) => Kind::Type,
                // XML's grammar calls attribute names properties; HTML's calls them
                // attributes, and they should look the same.
                ("property", Language::Xml) => Kind::Attribute,
                _ => kind,
            })
            .collect()
    }

    /// A grammar: a language, or one that only ever runs inside another.
    #[derive(Clone, Copy)]
    enum Grammar {
        Lang(Language),
        MarkdownInline,
    }

    const GRAMMARS: usize = Language::ALL.len() + 1;

    fn slot(g: Grammar) -> usize {
        match g {
            Grammar::Lang(l) => l as usize,
            Grammar::MarkdownInline => Language::ALL.len(),
        }
    }

    static LOADED: [OnceLock<Option<HighlightConfiguration>>; GRAMMARS] =
        [const { OnceLock::new() }; GRAMMARS];

    /// A grammar with its queries compiled, once; `None` if they failed to compile.
    fn loaded(g: Grammar) -> Option<&'static HighlightConfiguration> {
        LOADED[slot(g)].get_or_init(|| load(g)).as_ref()
    }

    fn load(g: Grammar) -> Option<HighlightConfiguration> {
        let (language, name, highlights, injections) = queries(g);
        let (highlights, injections) = (tidy(&highlights), tidy(&injections));
        let mut config = HighlightConfiguration::new(language, name, &highlights, &injections, "")
            .inspect_err(|e| {
                debug_assert!(false, "the {name} highlight query doesn't compile: {e}");
            })
            .ok()?;
        config.configure(&NAMES.iter().map(|(n, _)| *n).collect::<Vec<_>>());
        Some(config)
    }

    /// A query without the captures meant for an editor's spell checker: where several
    /// captures sit on one node, the highlighter takes the last, and an unrecognised one
    /// would leave the node plain.
    fn tidy(query: &str) -> String {
        query.replace(" @spell", "").replace(" @nospell", "")
    }

    /// Each grammar's language, its name for injections, its highlight query and its
    /// injection query. A grammar that extends another (C++, TypeScript, JSX) gets the base
    /// grammar's query first and its own after, since the later pattern wins where both
    /// match a node, as the grammars' own `tree-sitter.json` lists them.
    fn queries(g: Grammar) -> (tree_sitter::Language, &'static str, String, String) {
        use Language::*;
        let l = match g {
            Grammar::MarkdownInline => {
                return (
                    tree_sitter_md::INLINE_LANGUAGE.into(),
                    "markdown_inline",
                    tree_sitter_md::HIGHLIGHT_QUERY_INLINE.into(),
                    tree_sitter_md::INJECTION_QUERY_INLINE.into(),
                );
            }
            Grammar::Lang(l) => l,
        };
        match l {
            Bash => (
                tree_sitter_bash::LANGUAGE.into(),
                "bash",
                tree_sitter_bash::HIGHLIGHT_QUERY.into(),
                String::new(),
            ),
            C => (
                tree_sitter_c::LANGUAGE.into(),
                "c",
                tree_sitter_c::HIGHLIGHT_QUERY.into(),
                String::new(),
            ),
            Cpp => (
                tree_sitter_cpp::LANGUAGE.into(),
                "cpp",
                [tree_sitter_c::HIGHLIGHT_QUERY, tree_sitter_cpp::HIGHLIGHT_QUERY].concat(),
                String::new(),
            ),
            CSharp => (
                tree_sitter_c_sharp::LANGUAGE.into(),
                "c_sharp",
                tree_sitter_c_sharp::HIGHLIGHTS_QUERY.into(),
                String::new(),
            ),
            Css => (
                tree_sitter_css::LANGUAGE.into(),
                "css",
                tree_sitter_css::HIGHLIGHTS_QUERY.into(),
                String::new(),
            ),
            Dockerfile => (
                tree_sitter_containerfile::LANGUAGE.into(),
                "dockerfile",
                tree_sitter_containerfile::HIGHLIGHTS_QUERY.into(),
                tree_sitter_containerfile::INJECTIONS_QUERY.into(),
            ),
            Go => (
                tree_sitter_go::LANGUAGE.into(),
                "go",
                tree_sitter_go::HIGHLIGHTS_QUERY.into(),
                String::new(),
            ),
            Html => (
                tree_sitter_html::LANGUAGE.into(),
                "html",
                tree_sitter_html::HIGHLIGHTS_QUERY.into(),
                tree_sitter_html::INJECTIONS_QUERY.into(),
            ),
            Java => (
                tree_sitter_java::LANGUAGE.into(),
                "java",
                tree_sitter_java::HIGHLIGHTS_QUERY.into(),
                String::new(),
            ),
            JavaScript => (
                tree_sitter_javascript::LANGUAGE.into(),
                "javascript",
                tree_sitter_javascript::HIGHLIGHT_QUERY.into(),
                String::new(),
            ),
            Jsx => (
                tree_sitter_javascript::LANGUAGE.into(),
                "jsx",
                [
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                ]
                .concat(),
                String::new(),
            ),
            // The grammar's query names keys before strings, so under "the later pattern
            // wins" keys would be strings; naming them again last keeps them keys.
            Json => (
                tree_sitter_json::LANGUAGE.into(),
                "json",
                [
                    tree_sitter_json::HIGHLIGHTS_QUERY,
                    "\n(pair key: (_) @string.special.key)\n",
                ]
                .concat(),
                String::new(),
            ),
            Makefile => (
                tree_sitter_make::LANGUAGE.into(),
                "make",
                tree_sitter_make::HIGHLIGHTS_QUERY.into(),
                String::new(),
            ),
            // The inline grammar must see the whole `inline` node, children included; the
            // highlighter leaves children out unless told, and then it sees nothing. A
            // fenced block is not marked as code as a whole: its language's colours go
            // inside, and what they leave stays plain, as in an editor.
            Markdown => (
                tree_sitter_md::LANGUAGE.into(),
                "markdown",
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK
                    .replace("  (fenced_code_block)\n] @text.literal", "] @text.literal"),
                tree_sitter_md::INJECTION_QUERY_BLOCK
                    .replace(
                        "(#set! injection.language \"markdown_inline\")",
                        "(#set! injection.language \"markdown_inline\")\n (#set! injection.include-children)",
                    )
                    .replace(
                        "(code_fence_content) @injection.content)",
                        "(code_fence_content) @injection.content\n  (#set! injection.include-children))",
                    ),
            ),
            Proto => (
                tree_sitter_proto::LANGUAGE.into(),
                "proto",
                PROTO_HIGHLIGHTS.into(),
                String::new(),
            ),
            Python => (
                tree_sitter_python::LANGUAGE.into(),
                "python",
                tree_sitter_python::HIGHLIGHTS_QUERY.into(),
                String::new(),
            ),
            Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                "rust",
                tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
                tree_sitter_rust::INJECTIONS_QUERY.into(),
            ),
            Sql => (
                tree_sitter_sequel::LANGUAGE.into(),
                "sql",
                tree_sitter_sequel::HIGHLIGHTS_QUERY.into(),
                String::new(),
            ),
            Toml => (
                tree_sitter_toml_ng::LANGUAGE.into(),
                "toml",
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(),
                String::new(),
            ),
            TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                "typescript",
                [
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                ]
                .concat(),
                String::new(),
            ),
            Tsx => (
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                "tsx",
                [
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                ]
                .concat(),
                String::new(),
            ),
            Xml => (
                tree_sitter_xml::LANGUAGE_XML.into(),
                "xml",
                tree_sitter_xml::XML_HIGHLIGHT_QUERY.into(),
                String::new(),
            ),
            Yaml => (
                tree_sitter_yaml::LANGUAGE.into(),
                "yaml",
                tree_sitter_yaml::HIGHLIGHTS_QUERY.into(),
                String::new(),
            ),
        }
    }

    /// The grammar a query injects by name: Markdown's inline grammar, a fence's language
    /// (`rust`, `ts`, `sh`, …), HTML's `javascript` and `css`, a Dockerfile's `bash`. For
    /// whatever lifetime the highlighter wants; the grammars live for the program.
    fn injection<'a>(name: &str) -> Option<&'a HighlightConfiguration> {
        injected(name)
    }

    fn injected(name: &str) -> Option<&'static HighlightConfiguration> {
        use Language::*;
        let language = match name.to_ascii_lowercase().as_str() {
            "markdown_inline" => return loaded(Grammar::MarkdownInline),
            "bash" | "sh" | "shell" | "zsh" => Bash,
            "c" => C,
            "cpp" | "c++" | "cxx" => Cpp,
            "c_sharp" | "csharp" | "cs" | "c#" => CSharp,
            "css" => Css,
            "dockerfile" | "docker" | "containerfile" => Dockerfile,
            "go" | "golang" => Go,
            "html" => Html,
            "java" => Java,
            "javascript" | "js" => JavaScript,
            "jsx" => Jsx,
            "json" | "jsonc" => Json,
            "make" | "makefile" => Makefile,
            "markdown" | "md" => Markdown,
            "proto" | "protobuf" => Proto,
            "python" | "py" => Python,
            "rust" | "rs" => Rust,
            "sql" => Sql,
            "toml" => Toml,
            "typescript" | "ts" => TypeScript,
            "tsx" => Tsx,
            "xml" | "svg" => Xml,
            "yaml" | "yml" => Yaml,
            _ => return None,
        };
        loaded(Grammar::Lang(language))
    }

    pub fn highlight(language: Language, text: &str) -> Vec<Spans> {
        if text.len() > MAX_BYTES {
            return Vec::new();
        }
        let Some(config) = loaded(Grammar::Lang(language)) else {
            return Vec::new();
        };
        let kinds = kinds(language);
        // Where each line starts, and where its content ends (before `\n` or `\r\n`).
        let mut starts = vec![0];
        let mut ends = Vec::new();
        let bytes = text.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                let end = if i > 0 && bytes[i - 1] == b'\r' {
                    i - 1
                } else {
                    i
                };
                ends.push(end);
                starts.push(i + 1);
            }
        }
        if *starts.last().unwrap() < text.len() {
            ends.push(text.len());
        } else {
            starts.pop();
        }
        let mut lines: Vec<Spans> = vec![Vec::new(); starts.len()];
        let mut highlighter = Highlighter::new();
        let Ok(events) = highlighter.highlight(config, bytes, None, None, injection) else {
            return lines;
        };
        // The innermost highlight colours the text: a string's escape over the string, a
        // fence's Rust over the Markdown.
        let mut stack: Vec<Kind> = Vec::new();
        for event in events {
            let Ok(event) = event else { break };
            match event {
                HighlightEvent::HighlightStart(h) => stack.push(kinds[h.0]),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    let Some(&kind) = stack.last() else { continue };
                    let mut i = starts.partition_point(|&s| s <= start).saturating_sub(1);
                    let mut at = start;
                    while at < end && i < starts.len() {
                        let (base, content_end) = (starts[i], ends[i]);
                        let stop = end.min(content_end);
                        if at < stop {
                            lines[i].push((at - base..stop - base, kind));
                        }
                        i += 1;
                        at = starts.get(i).copied().unwrap_or(end);
                    }
                }
            }
        }
        lines
    }
}

#[cfg(all(test, feature = "syntax"))]
mod tests {
    use super::*;

    /// Every coloured piece of `text`, with its kind, in order.
    fn kinds_of(language: Language, text: &str) -> Vec<(std::string::String, Kind)> {
        let lines = highlight(language, text);
        let mut out = Vec::new();
        for (line, spans) in text.split('\n').zip(&lines) {
            let line = line.strip_suffix('\r').unwrap_or(line);
            for (r, k) in spans {
                out.push((line[r.clone()].to_string(), *k));
            }
        }
        out
    }

    #[track_caller]
    fn expect(language: Language, text: &str, expected: &[(&str, Kind)]) {
        let got = kinds_of(language, text);
        for (piece, kind) in expected {
            assert!(
                got.iter().any(|(p, k)| p == piece && k == kind),
                "{language:?}: expected {piece:?} as {kind:?}; got {got:?}"
            );
        }
    }

    use Kind::*;

    #[test]
    fn every_grammar_loads_and_colours_something() {
        for language in Language::ALL {
            let spans = highlight(language, "x = 1 // ? # ; /* */ <a>\n");
            assert_eq!(spans.len(), 1, "{language:?}");
        }
    }

    #[test]
    fn bash() {
        expect(
            Language::Bash,
            "# note\nif [ -f x ]; then exit 1; fi\n",
            &[("# note", Comment), ("if", Keyword), ("exit", Function)],
        );
    }

    #[test]
    fn c() {
        expect(
            Language::C,
            "/* c */\nint main(void) { return 0; }\n",
            &[
                ("/* c */", Comment),
                ("int", Type),
                ("main", Function),
                ("return", Keyword),
                ("0", Number),
            ],
        );
    }

    #[test]
    fn cpp() {
        expect(
            Language::Cpp,
            "// x\nclass A { public: virtual ~A() = default; };\nint n = 2;\n",
            &[
                ("// x", Comment),
                ("class", Keyword),
                ("int", Type),
                ("2", Number),
            ],
        );
    }

    #[test]
    fn c_sharp() {
        expect(
            Language::CSharp,
            "// c\nnamespace N { [Fact] public class A { string S => \"x\"; } }\n",
            &[
                ("// c", Comment),
                ("class", Keyword),
                ("\"x\"", String),
                ("string", Type),
                ("Fact", Type),
            ],
        );
    }

    #[test]
    fn css() {
        expect(
            Language::Css,
            "/* c */\na:hover { color: #fff; margin: 1px; }\n",
            &[("/* c */", Comment), ("color", Property), ("a", Tag)],
        );
    }

    #[test]
    fn dockerfile() {
        expect(
            Language::Dockerfile,
            "# c\nFROM alpine:3.19\nRUN apk add git\n",
            &[("# c", Comment), ("FROM", Keyword), ("RUN", Keyword)],
        );
    }

    #[test]
    fn go() {
        expect(
            Language::Go,
            "// c\npackage main\nfunc main() { fmt.Println(\"x\", 1) }\n",
            &[
                ("// c", Comment),
                ("package", Keyword),
                ("func", Keyword),
                ("\"x\"", String),
                ("1", Number),
            ],
        );
    }

    #[test]
    fn html_with_injected_script() {
        expect(
            Language::Html,
            "<!-- c --><div class=\"a\"><script>let x = 1;</script></div>\n",
            &[
                ("<!-- c -->", Comment),
                ("div", Tag),
                ("class", Attribute),
                ("a", String),
                ("let", Keyword),
            ],
        );
    }

    #[test]
    fn java() {
        expect(
            Language::Java,
            "// c\npublic class A { private int x = 1; }\n",
            &[
                ("// c", Comment),
                ("class", Keyword),
                ("int", Type),
                ("1", Number),
            ],
        );
    }

    #[test]
    fn javascript_and_jsx() {
        expect(
            Language::JavaScript,
            "// c\nconst f = (a) => a + 1;\n",
            &[("// c", Comment), ("const", Keyword), ("1", Number)],
        );
        expect(
            Language::Jsx,
            "const a = <div className=\"x\">{1}</div>;\n",
            &[("div", Tag), ("\"x\"", String)],
        );
    }

    #[test]
    fn json() {
        expect(
            Language::Json,
            "{\"a\": [1, true, \"s\"]}\n",
            &[
                ("\"a\"", Property),
                ("1", Number),
                ("true", Constant),
                ("\"s\"", String),
            ],
        );
    }

    #[test]
    fn makefile() {
        expect(
            Language::Makefile,
            "# c\nall: main.o\n\t$(CC) -o $@ main.o\n",
            &[("# c", Comment)],
        );
    }

    #[test]
    fn markdown_with_fenced_rust() {
        expect(
            Language::Markdown,
            "# Title\n\nSome *em* and **strong** and `code`.\n\n```rust\nfn main() {}\n```\n",
            &[
                ("Title", Heading),
                ("em", Emphasis),
                ("strong", Strong),
                ("code", Raw),
                ("fn", Keyword),
                ("main", Function),
            ],
        );
    }

    #[test]
    fn proto() {
        expect(
            Language::Proto,
            "syntax = \"proto3\";\n// c\nmessage A { int32 id = 1; }\n",
            &[
                ("\"proto3\"", String),
                ("// c", Comment),
                ("message", Keyword),
                ("int32", Type),
                ("1", Number),
            ],
        );
    }

    #[test]
    fn python() {
        expect(
            Language::Python,
            "# c\ndef f(x):\n    return \"s\" + str(1)\n",
            &[
                ("# c", Comment),
                ("def", Keyword),
                ("f", Function),
                ("\"s\"", String),
                ("1", Number),
            ],
        );
    }

    #[test]
    fn rust_with_escape_and_attribute() {
        expect(
            Language::Rust,
            "/// d\n#[derive(Debug)]\nfn main() { let s = \"x\\n\"; }\n",
            &[
                ("/// d", Comment),
                ("fn", Keyword),
                ("main", Function),
                ("\\n", Escape),
                ("derive", Function),
            ],
        );
    }

    #[test]
    fn sql() {
        let text = "/* a comment\n   over two lines */\nSELECT name FROM t WHERE id = 'x';\n";
        let lines = highlight(Language::Sql, text);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], vec![(0..12, Comment)]);
        assert_eq!(lines[1], vec![(0..20, Comment)]);
        expect(Language::Sql, text, &[("SELECT", Keyword), ("'x'", String)]);
        expect(Language::Sql, "-- c\nSELECT 1;\n", &[("-- c", Comment)]);
    }

    #[test]
    fn toml() {
        let text = "# c\n[pkg]\nname = \"x\"\nn = 1\n";
        expect(
            Language::Toml,
            text,
            &[
                ("# c", Comment),
                ("pkg", Type),
                ("\"x\"", String),
                ("1", Number),
            ],
        );
        // The grammar's query marks the whole pair as a property, its key included.
        let got = kinds_of(Language::Toml, text);
        assert!(
            got.iter()
                .any(|(p, k)| p.starts_with("name") && *k == Property),
            "{got:?}"
        );
    }

    #[test]
    fn typescript_and_tsx() {
        expect(
            Language::TypeScript,
            "// c\nfunction f(a: number): string { return `${a}`; }\n",
            &[
                ("// c", Comment),
                ("function", Keyword),
                ("number", Type),
                ("string", Type),
                ("f", Function),
            ],
        );
        expect(
            Language::Tsx,
            "const a = <div className=\"x\">{1}</div>;\n",
            &[("div", Tag), ("const", Keyword)],
        );
    }

    #[test]
    fn xml() {
        expect(
            Language::Xml,
            "<!-- c --><a b=\"1\">t</a>\n",
            &[
                ("<!-- c -->", Comment),
                ("a", Tag),
                ("b", Attribute),
                ("1", String),
            ],
        );
    }

    #[test]
    fn yaml() {
        expect(
            Language::Yaml,
            "# c\nkey: value\nn: 1\nlist:\n  - true\n",
            &[
                ("# c", Comment),
                ("key", Property),
                ("1", Number),
                ("true", Constant),
            ],
        );
    }

    #[test]
    fn crlf_lines_keep_their_offsets() {
        let text = "SELECT 1;\r\nSELECT 2;\r\n";
        let lines = highlight(Language::Sql, text);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].iter().any(|(r, k)| *k == Keyword && r.start == 0));
        assert!(lines[1].iter().all(|(r, _)| r.end <= 9));
    }

    #[test]
    fn language_from_path() {
        assert_eq!(language_of("db/schema.SQL"), Some(Language::Sql));
        assert_eq!(language_of("src/main.rs"), Some(Language::Rust));
        assert_eq!(language_of("a\\b\\Dockerfile"), Some(Language::Dockerfile));
        assert_eq!(language_of("Dockerfile.dev"), Some(Language::Dockerfile));
        assert_eq!(language_of("Makefile"), Some(Language::Makefile));
        assert_eq!(language_of("Cargo.lock"), Some(Language::Toml));
        assert_eq!(language_of("api/v1.proto"), Some(Language::Proto));
        assert_eq!(language_of("app.tsx"), Some(Language::Tsx));
        assert_eq!(language_of("notes.txt"), None);
        assert_eq!(language_of("LICENSE"), None);
    }

    #[test]
    fn ids_name_every_language_once() {
        for language in Language::ALL {
            assert_eq!(Language::from_id(language.id()), Some(language));
        }
        assert_eq!(Language::from_id("cobol"), None);
    }

    #[test]
    fn the_wire_format_round_trips() {
        let spans = highlight(Language::Rust, "fn main() {}\n// c\n");
        let bytes = serde_json::to_vec(&compact(&spans)).unwrap();
        assert_eq!(decode(&bytes), Some(spans));
        assert_eq!(decode(b"nonsense"), None);
    }

    #[test]
    fn huge_files_are_left_plain() {
        let text = "x\n".repeat(MAX_BYTES / 2 + 1);
        assert!(highlight(Language::Yaml, &text).is_empty());
    }
}
