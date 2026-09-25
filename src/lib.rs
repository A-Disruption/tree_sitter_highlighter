//! Minimal Tree-sitter Rust highlighter with incremental edits + span output.
//!
//! This library provides a ready-to-use Tree-sitter highlighter for iced applications.
//! You can use it directly with `text_editor::highlight_with` without any configuration:
//!
//! ```ignore
//! use tree_sitter_highlighter::{TsSettings, TreeSitterIcedHighlighter, code_gen_text_editor_style};
//!
//! text_editor(&state.content)
//!     .highlight_with::<TreeSitterIcedHighlighter>(
//!         TsSettings { text: Arc::from(state.content.text()) },
//!         TreeSitterIcedHighlighter::to_format,
//!     )
//!     .style(code_gen_text_editor_style)
//! ```

use std::{cmp::Ordering, ops::Range, sync::Arc};

use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, Tree};
use tree_sitter::StreamingIterator;
use iced::advanced::text::highlighter::Style as HighlightStyle;
use iced::advanced::text::Parser as IcedParser;
use iced::widget::text_editor;
use iced::theme::Palette;
use iced::{Color, Theme};

const EXTRA_QUERY: &str = r#"
; Highlight identifiers that appear inside `use ...` declarations.
; Use wildcards at a few depths to cover: use foo::bar::{baz};
; Low priority so the real Rust highlights win when both match.
((use_declaration (identifier) @namespace) (#set! "priority" "-50"))
((use_declaration (_ (identifier) @namespace)) (#set! "priority" "-50"))
((use_declaration (_ (_ (identifier) @namespace))) (#set! "priority" "-50"))
((use_declaration (_ (_ (_ (identifier) @namespace)))) (#set! "priority" "-50"))
((use_declaration (_ (_ (_ (_ (identifier) @namespace))))) (#set! "priority" "-50"))

; Also catch identifiers that are children of scoped paths.
((use_declaration (scoped_identifier (identifier) @namespace)) (#set! "priority" "-50"))
((use_declaration (_ (scoped_identifier (identifier) @namespace))) (#set! "priority" "-50"))
((use_declaration (_ (_ (scoped_identifier (identifier) @namespace)))) (#set! "priority" "-50"))
"#;

const EXTRA_ATTR_QUERY: &str = r#"
; Color attribute name like `derive` in #[derive(...)]
((attribute_item (attribute (identifier) @attribute.name))
  (#set! "priority" "80"))
"#;

const EXTRA_ARROW_QUERY: &str = r#"
; Color arrow operators => -> and ::
(("->" @arrow) (#set! "priority" "-60"))
(("=>" @arrow) (#set! "priority" "-60"))
(("::" @arrow) (#set! "priority" "100"))
"#;

const EXTRA_SELF_QUERY: &str = r#"
; Match Self (the type) - can be identifier or type_identifier depending on context
((identifier) @self_type
  (#eq? @self_type "Self")
  (#set! "priority" "110"))
((type_identifier) @self_type
  (#eq? @self_type "Self")
  (#set! "priority" "110"))
"#;

const EXTRA_STRUCT_ENUM_QUERY: &str = r#"
; Override struct, enum, and mut keywords to style them differently
(("struct" @keyword.struct) (#set! "priority" "110"))
(("enum" @keyword.enum) (#set! "priority" "110"))
((mutable_specifier) @keyword.mut (#set! "priority" "110"))
"#;

const EXTRA_MODULE_PATH_QUERY: &str = r#"
; Color lowercase module paths in expressions (iced::application)
((scoped_identifier
  path: (identifier) @module_path)
  (#match? @module_path "^[a-z]")
  (#set! "priority" "100"))

; Nested scoped paths that start lowercase (std::io::Result)
((scoped_identifier
  path: (scoped_identifier
    path: (identifier) @module_path))
  (#match? @module_path "^[a-z]")
  (#set! "priority" "100"))

; Function references passed as arguments (App::new in foo(App::new))
; These are scoped_identifiers used as values, not calls
((call_expression
  arguments: (arguments
    (scoped_identifier
      path: (identifier) @module_path)))
  (#set! "priority" "100"))

; Module paths in type positions (iced::Result in return types)
((scoped_type_identifier
  path: (identifier) @namespace)
  (#match? @namespace "^[a-z]")
  (#set! "priority" "100"))
"#;

/// A coarse set of "kinds" you can map to your iced theme.
/// You'll likely collapse many Tree-sitter captures into a smaller set like this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Comment,
    String,
    Number,
    Keyword,
    Type,
    Function,
    Constructor, // New: for Enum variants like 'None' or 'Some'
    Macro,       // New: for macros and attributes like 'derive'
    Namespace,   // New: for 'iced', 'std' in use declarations
    ModulePath,  // New: for module/type paths like 'iced::' or 'App::' in expressions
    SelfKeyword, // New: for 'self' and 'Self'
    Operator,
    Arrow,       // New: for '=>', '->', '::'
    Reference,   // New: for '&' and '&mut'
    Punctuation,
    Variable,
    Parameter,   // New: for function arguments
    Field,
    Lifetime,
    Error,
    Default,
    // Rainbow brackets - 6 colors shared by all bracket types
    // Red, Yellow, Green, Blue, Orange, Pink
    Rainbow1,
    Rainbow2,
    Rainbow3,
    Rainbow4,
    Rainbow5,
    Rainbow6,
}

/// Highlight span expressed in *line-local byte columns* (not grapheme columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub kind: TokenKind,
}

/// A simple “replace this byte range” edit description.
/// - `start_byte`: where the edit begins (byte offset in old/new)
/// - `old_end_byte`: end of replaced range in the old text
/// - `new_end_byte`: end of inserted range in the new text
#[derive(Debug, Clone, Copy)]
pub struct SimpleEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
}

/// Internal: precomputed line-start offsets so we can convert bytes <-> (row,col).
#[derive(Debug, Clone)]
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = Vec::with_capacity(256);
        starts.push(0);

        // Track byte offsets of line starts.
        for (i, b) in text.as_bytes().iter().enumerate() {
            if *b == b'\n' {
                // Next line starts after '\n'
                starts.push(i + 1);
            }
        }

        Self { starts }
    }

    fn line_of_byte(&self, byte: usize) -> usize {
        match self.starts.binary_search(&byte) {
            Ok(idx) => idx,
            Err(insert) => insert.saturating_sub(1),
        }
    }

    fn line_start(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or(0)
    }

    fn point_for_byte(&self, byte: usize) -> Point {
        let row = self.line_of_byte(byte);
        let col = byte.saturating_sub(self.line_start(row));
        Point { row, column: col }
    }

    fn line_byte_range(&self, text: &str, line: usize) -> Range<usize> {
        let start = self.line_start(line);
        let end = if line + 1 < self.starts.len() {
            self.starts[line + 1]
        } else {
            text.len()
        };
        start..end
    }

    fn clamp_range_to_full_lines(&self, text: &str, range: Range<usize>) -> Range<usize> {
        let start_line = self.line_of_byte(range.start.min(text.len()));
        let end_line = self.line_of_byte(range.end.min(text.len()));
        let start = self.line_start(start_line);
        let end = self
            .line_byte_range(text, end_line)
            .end;
        start..end
    }
}

/// Raw capture span (global byte offsets) with a priority for overlap resolution.
#[derive(Debug, Clone)]
struct RawSpan {
    start: usize,
    end: usize,
    kind: TokenKind,
    priority: i32,
    capture_name: String,
}

pub struct Highlighter {
    parser: Parser,
    query: Query,
    cursor: QueryCursor,
    tree: Option<Tree>,
}

impl Highlighter {
    pub fn new() -> anyhow::Result<Self> {
        let mut parser = Parser::new();
        let language = tree_sitter_rust::LANGUAGE;
        parser
            .set_language(&language.into())
            .map_err(|e| anyhow::anyhow!("set_language failed: {e:?}"))?;

        // Use the language's maintained highlight query.
        let ts_language = language.into();
        let combined = format!("{}\n{}\n{}\n{}\n{}\n{}\n{}", tree_sitter_rust::HIGHLIGHTS_QUERY, EXTRA_QUERY, EXTRA_ATTR_QUERY, EXTRA_ARROW_QUERY, EXTRA_SELF_QUERY, EXTRA_STRUCT_ENUM_QUERY, EXTRA_MODULE_PATH_QUERY);
        let query = Query::new(&ts_language, &combined)
            .map_err(|e| anyhow::anyhow!("Query::new failed: {e:?}"))?;

        Ok(Self {
            parser,
            query,
            cursor: QueryCursor::new(),
            tree: None,
        })
    }

    /// Parse the whole document (first load).
    pub fn set_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.tree = self
            .parser
            .parse(text, None)
            .ok_or_else(|| anyhow::anyhow!("parse returned None"))?
            .into();
        Ok(())
    }

    /// Apply an incremental edit and reparse using the previous tree.
    ///
    /// Returns byte ranges whose *syntactic structure* changed (good default ranges to re-highlight).
    pub fn apply_edit(&mut self, old_text: &str, new_text: &str, edit: SimpleEdit) -> anyhow::Result<Vec<Range<usize>>> {
        let mut old_tree = self
            .tree
            .take()
            .ok_or_else(|| anyhow::anyhow!("apply_edit called before set_text"))?;

        let old_index = LineIndex::new(old_text);
        let new_index = LineIndex::new(new_text);

        let input_edit = InputEdit {
            start_byte: edit.start_byte,
            old_end_byte: edit.old_end_byte,
            new_end_byte: edit.new_end_byte,
            start_position: old_index.point_for_byte(edit.start_byte),
            old_end_position: old_index.point_for_byte(edit.old_end_byte),
            new_end_position: new_index.point_for_byte(edit.new_end_byte),
        };

        old_tree.edit(&input_edit);

        let new_tree = self
            .parser
            .parse(new_text, Some(&old_tree))
            .ok_or_else(|| anyhow::anyhow!("parse returned None"))?;

        let changed: Vec<std::ops::Range<usize>> = old_tree
            .changed_ranges(&new_tree)
            .map(|r| r.start_byte..r.end_byte)
            .collect();

        self.tree = Some(new_tree);
        Ok(changed)
    }

    /// Highlight only a byte range of the document. (Pass full text; we’ll restrict query execution.)
    ///
    /// Output spans are line-local (line, start_col_bytes, end_col_bytes).
    pub fn highlight_range(&mut self, text: &str, range: Range<usize>) -> anyhow::Result<Vec<Span>> {
        let tree = self
            .tree
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("highlight_range called before set_text"))?;

        let index = LineIndex::new(text);
        let range = index.clamp_range_to_full_lines(text, range);

        // Run query only over the range (still returns matches intersecting it).
        self.cursor.set_byte_range(range.clone());

        let root = tree.root_node();

        // Collect raw spans with priorities.
        let mut raw = Vec::<RawSpan>::new();

        // Use matches() so we can read per-pattern properties like `#set! priority`.
        let mut matches = self.cursor.matches(&self.query, root, text.as_bytes());
        matches.advance();
        while let Some(m) = matches.get() {
            let priority = extract_priority(&self.query, m.pattern_index);

            for cap in m.captures {
                let cap_name = self.query.capture_names()[cap.index as usize];
                let kind = classify_capture(cap_name);
                if kind == TokenKind::Default {
                    continue;
                }

                let start = cap.node.start_byte();
                let end = cap.node.end_byte();

                if end <= range.start || start >= range.end || start == end {
                    continue;
                }

                raw.push(RawSpan {
                    start: start.max(range.start),
                    end: end.min(range.end),
                    kind,
                    priority,
                    capture_name: cap_name.to_owned(),
                });
            }

            matches.advance();
        }

        // Resolve overlaps into a flat list of non-overlapping segments.
        let segments = resolve_overlaps(raw);

        // Apply rainbow bracket coloring based on nesting depth
        let segments = colorize_brackets(text, range.start, range.end, segments);

        // Convert global byte segments into line-local spans.
        Ok(segments_to_line_spans(text, &index, segments))
    }
}

impl Highlighter {
    pub fn to_format(kind: TokenKind, theme: &Theme) -> HighlightStyle {
        let p = theme.palette();

        let color = match kind {
            TokenKind::Comment => p.background.strong.color,
            TokenKind::Macro => p.primary.strong.color,
            TokenKind::String => p.success.base.color,
            TokenKind::Number => p.primary.strong.color,
            TokenKind::Keyword => p.primary.base.color,
            TokenKind::Type => p.secondary.base.color,
            TokenKind::Function => p.primary.weak.color,
            TokenKind::Operator => p.background.base.text,
            TokenKind::Arrow => p.secondary.weak.color,      // =>, ->, :: same color
            TokenKind::Reference => p.secondary.weak.color,  // &, &mut same as arrows
            TokenKind::Punctuation => p.background.base.text,
            TokenKind::Variable => p.background.base.text,
            TokenKind::Parameter => p.background.base.text,
            TokenKind::Field => p.success.base.color,
            TokenKind::Lifetime => p.danger.weak.color,
            TokenKind::Error => p.danger.strong.color,
            TokenKind::Default => p.background.base.text,
            TokenKind::Namespace => p.success.strong.color,
            TokenKind::ModulePath => p.success.strong.color, // Same as Namespace
            TokenKind::Constructor => p.success.strong.color,
            TokenKind::SelfKeyword => p.success.strong.color,
            // Rainbow brackets - 6 colors: Red, Yellow, Green, Blue, Orange, Pink
            TokenKind::Rainbow1 => p.danger.strong.color,   // Red
            TokenKind::Rainbow2 => p.warning.base.color,    // Yellow
            TokenKind::Rainbow3 => p.success.strong.color,  // Green
            TokenKind::Rainbow4 => p.primary.strong.color,  // Blue
            TokenKind::Rainbow5 => p.warning.strong.color,  // Orange
            TokenKind::Rainbow6 => p.danger.base.color,     // Pink
        };

        HighlightStyle {
            color: Some(color),
            style: None,
        }
    }
}

/// Extract an integer priority from `#set! "priority" "N"` if present.
fn extract_priority(query: &Query, pattern_index: usize) -> i32 {
    query
        .property_settings(pattern_index)
        .iter()
        .find_map(|p| {
            if p.key.as_ref() == "priority" {
                p.value
                    .as_deref()
                    .and_then(|v| v.parse::<i32>().ok())
            } else {
                None
            }
        })
        .unwrap_or(0)
}

/// Collapse Tree-sitter capture names into a smaller TokenKind set.
fn classify_capture(name: &str) -> TokenKind {
    // Handle specific captures that need special treatment
    match name {
        "variable.builtin" => return TokenKind::SelfKeyword, // self
        "type.builtin" => return TokenKind::SelfKeyword,     // Self (type context)
        "self_type" => return TokenKind::SelfKeyword,        // Self (our custom query)
        "constructor" => return TokenKind::Constructor,      // Enum variants
        "attribute" => return TokenKind::Macro,              // #[...]
        "attribute.name" => return TokenKind::Macro,         // derive
        "variable.parameter" => return TokenKind::Parameter,
        "function.method" => return TokenKind::Function,     // treat methods as functions
        "arrow" => return TokenKind::Arrow,                  // => and -> and ::
        "module_path" => return TokenKind::ModulePath,       // iced::, App::
        "keyword.struct" => return TokenKind::Operator,      // struct keyword styled like operator
        "keyword.enum" => return TokenKind::Operator,        // enum keyword styled like operator
        "keyword.mut" => return TokenKind::Operator,         // mut keyword styled like operator
        _ => {}
    }

    let prefix = name.split('.').next().unwrap_or(name);

    match prefix {
        "comment" => TokenKind::Comment,
        "string" => TokenKind::String,
        "number" => TokenKind::Number,
        "keyword" => TokenKind::Keyword,
        "type" => TokenKind::Type,
        "namespace" => TokenKind::Namespace,
        "module_path" => TokenKind::ModulePath,
        "function" => TokenKind::Function,
        "constant" => TokenKind::Constructor,
        "operator" => TokenKind::Operator,
        "punctuation" => TokenKind::Punctuation,
        "variable" => TokenKind::Variable,
        "property" => TokenKind::Field,
        "lifetime" => TokenKind::Lifetime,
        "label" => TokenKind::Lifetime,
        "escape" => TokenKind::String,
        "error" => TokenKind::Error,
        _ => TokenKind::Default,
    }
}

/// Resolve overlapping spans by priority, producing non-overlapping segments.
///
/// Policy:
/// - Highest priority wins
/// - If tie: prefer "more specific" capture (more dots)
/// - If tie: prefer later capture name lexicographically (stable-ish)
fn resolve_overlaps(spans: Vec<RawSpan>) -> Vec<RawSpan> {
    if spans.is_empty() {
        return spans;
    }

    // Event sweep.
    #[derive(Clone, Copy)]
    enum Kind {
        Start,
        End,
    }

    #[derive(Clone, Copy)]
    struct Event {
        pos: usize,
        kind: Kind,
        id: usize,
    }

    // Keep original spans in vec, and reference by index.
    let mut events = Vec::with_capacity(spans.len() * 2);
    for (id, s) in spans.iter().enumerate() {
        events.push(Event {
            pos: s.start,
            kind: Kind::Start,
            id,
        });
        events.push(Event {
            pos: s.end,
            kind: Kind::End,
            id,
        });
    }

    events.sort_by(|a, b| {
        match a.pos.cmp(&b.pos) {
            Ordering::Equal => {
                // End before Start at the same position to avoid zero-length bleed.
                match (a.kind, b.kind) {
                    (Kind::End, Kind::Start) => Ordering::Less,
                    (Kind::Start, Kind::End) => Ordering::Greater,
                    _ => a.id.cmp(&b.id),
                }
            }
            other => other,
        }
    });

    let mut active: Vec<usize> = Vec::new();
    let mut out: Vec<RawSpan> = Vec::new();

    let mut i = 0;
    while i < events.len() {
        let pos = events[i].pos;

        // Apply all events at this position.
        while i < events.len() && events[i].pos == pos {
            match events[i].kind {
                Kind::Start => active.push(events[i].id),
                Kind::End => {
                    if let Some(idx) = active.iter().position(|x| *x == events[i].id) {
                        active.swap_remove(idx);
                    }
                }
            }
            i += 1;
        }

        let next_pos = events.get(i).map(|e| e.pos);
        let Some(next_pos) = next_pos else { break };

        if pos == next_pos {
            continue;
        }

        // Choose best active span for [pos, next_pos).
        let best = active
            .iter()
            .copied()
            .max_by(|a, b| {
                let sa = &spans[*a];
                let sb = &spans[*b];

                // priority
                match sa.priority.cmp(&sb.priority) {
                    Ordering::Equal => {
                        // specificity (dots)
                        let da = sa.capture_name.matches('.').count();
                        let db = sb.capture_name.matches('.').count();
                        match da.cmp(&db) {
                            Ordering::Equal => sa.capture_name.cmp(&sb.capture_name),
                            other => other,
                        }
                    }
                    other => other,
                }
            });

        if let Some(best_id) = best {
            let s = &spans[best_id];
            let seg = RawSpan {
                start: pos,
                end: next_pos,
                kind: s.kind,
                priority: s.priority,
                capture_name: s.capture_name.clone(),
            };

            // Merge adjacent equal-kinded segments.
            if let Some(last) = out.last_mut() {
                if last.end == seg.start && last.kind == seg.kind {
                    last.end = seg.end;
                    continue;
                }
            }

            out.push(seg);
        }
    }

    out
}

/// Colorize brackets based on nesting depth.
/// All bracket types share the same depth counter (like VSCode).
fn colorize_brackets(text: &str, range_start: usize, range_end: usize, mut segments: Vec<RawSpan>) -> Vec<RawSpan> {
    let bytes = text.as_bytes();

    // Scan from document start to range_start to establish initial bracket depth
    // All bracket types share one unified depth counter
    let mut depth: i32 = 0;

    for &byte in &bytes[..range_start.min(bytes.len())] {
        match byte {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => depth = (depth - 1).max(0),
            _ => {}
        }
    }

    // Helper to get the rainbow TokenKind based on depth (6 colors)
    fn rainbow_kind(depth: i32) -> TokenKind {
        match depth % 6 {
            0 => TokenKind::Rainbow1, // Red
            1 => TokenKind::Rainbow2, // Yellow
            2 => TokenKind::Rainbow3, // Green
            3 => TokenKind::Rainbow4, // Blue
            4 => TokenKind::Rainbow5, // Orange
            _ => TokenKind::Rainbow6, // Pink
        }
    }

    // Collect bracket positions and their token kinds within the range
    let mut bracket_spans: Vec<RawSpan> = Vec::new();

    for pos in range_start..range_end.min(bytes.len()) {
        let byte = bytes[pos];
        let kind = match byte {
            b'(' | b'{' | b'[' => {
                let kind = rainbow_kind(depth);
                depth += 1;
                Some(kind)
            }
            b')' | b'}' | b']' => {
                depth = (depth - 1).max(0);
                Some(rainbow_kind(depth))
            }
            _ => None,
        };

        if let Some(kind) = kind {
            bracket_spans.push(RawSpan {
                start: pos,
                end: pos + 1,
                kind,
                priority: 200, // High priority to override other captures
                capture_name: "bracket".to_string(),
            });
        }
    }

    // Merge bracket spans with existing segments
    // Remove any existing segments that overlap with bracket positions
    segments.retain(|seg| {
        !bracket_spans.iter().any(|b| b.start < seg.end && b.end > seg.start && seg.end - seg.start == 1)
    });

    // Add bracket spans and re-sort by position
    segments.extend(bracket_spans);
    segments.sort_by_key(|s| s.start);

    segments
}

/// Convert global-byte segments into line-local spans.
fn segments_to_line_spans(text: &str, index: &LineIndex, segments: Vec<RawSpan>) -> Vec<Span> {
    let mut out = Vec::with_capacity(segments.len());

    for seg in segments {
        let mut start = seg.start;
        let end = seg.end;

        while start < end {
            let line = index.line_of_byte(start);
            let line_range = index.line_byte_range(text, line);

            let seg_end = end.min(line_range.end);
            let start_col = start - line_range.start;
            let end_col = seg_end - line_range.start;

            // Avoid emitting spans that are only the '\n' or empty.
            if start_col < end_col {
                out.push(Span {
                    line,
                    start_col,
                    end_col,
                    kind: seg.kind,
                });
            }

            start = seg_end;
        }
    }

    out
}

// =============================================================================
// Iced Integration - Ready-to-use highlighter adapter and styles
// =============================================================================

/// Settings for the Tree-sitter iced highlighter.
///
/// Pass the current text content to enable syntax highlighting.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TsSettings {
    pub text: Arc<str>,
}

/// Adapts the Tree-sitter highlighter to iced's line-by-line `Highlighter` trait.
///
/// This adapter handles the conversion between iced's line-based highlighting
/// and Tree-sitter's document-level parsing. Use it with `text_editor::highlight_with`.
///
/// # Example
/// ```ignore
/// text_editor(&state.content)
///     .highlight_with::<TreeSitterIcedHighlighter>(
///         TsSettings { text: Arc::from(state.content.text()) },
///         TreeSitterIcedHighlighter::to_format,
///     )
/// ```
pub struct TreeSitterIcedHighlighter {
    ts: Highlighter,
    settings: TsSettings,
    current: usize,
    line_starts: Vec<usize>,
}

impl IcedParser for TreeSitterIcedHighlighter {
    type Settings = TsSettings;
    type Output = TokenKind;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Self::Output)>;

    fn new(settings: &Self::Settings) -> Self {
        let mut ts = Highlighter::new().expect("ts init");
        let _ = ts.set_text(&settings.text);

        Self {
            ts,
            settings: settings.clone(),
            current: 0,
            line_starts: compute_line_starts(&settings.text),
        }
    }

    fn update(&mut self, new_settings: &Self::Settings) {
        if self.settings.text.as_ref() != new_settings.text.as_ref() {
            self.settings = new_settings.clone();
            self.current = 0;
            self.line_starts = compute_line_starts(&self.settings.text);
            let _ = self.ts.set_text(&self.settings.text);
        }
    }

    fn change_line(&mut self, line: usize) {
        self.current = line;
    }

    fn parse_line(&mut self, line: &str) -> Self::Iterator<'_> {
        let text = self.settings.text.as_ref();

        let line_start = self.line_starts.get(self.current).copied().unwrap_or(text.len());
        let line_end = if self.current + 1 < self.line_starts.len() {
            self.line_starts[self.current + 1].saturating_sub(1)
        } else {
            text.len()
        };

        let line_len = line.len();
        let expected_len = line_end.saturating_sub(line_start);
        let effective_end = line_start + expected_len.min(line_len);

        let spans = match self.ts.highlight_range(text, line_start..effective_end) {
            Ok(s) => s,
            Err(_) => {
                Vec::new()
            }
        };

        let mut out: Vec<(Range<usize>, TokenKind)> = Vec::with_capacity(spans.len());

        for s in spans {
            let a = s.start_col.min(line_len);
            let b = s.end_col.min(line_len);
            if a < b {
                out.push((a..b, s.kind));
            }
        }

        self.current += 1;
        out.into_iter()
    }

    fn current_line(&self) -> usize {
        self.current
    }
}

fn compute_line_starts(text: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    starts.push(0);
    for (i, b) in text.as_bytes().iter().enumerate() {
        if *b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

impl TreeSitterIcedHighlighter {
    /// Convert a `TokenKind` to an iced `Format` using the theme's color scheme.
    ///
    /// Use this as the second argument to `highlight_with`.
    pub fn to_format(kind: TokenKind, theme: &Theme) -> HighlightStyle {
        let extended = theme.palette();
        let syntax = SyntaxHighlight::generate(extended);

        // master's `highlighter::Style` carries a font *style* (italic, oblique)
        // rather than a whole `Font`; nothing here italicises a token yet.
        let style = None;

        let color = match kind {
            TokenKind::Comment => syntax.comment,
            TokenKind::String => syntax.string,
            TokenKind::Number => syntax.number,
            TokenKind::Keyword => syntax.keyword,
            TokenKind::Type => syntax.type_name,
            TokenKind::Function => syntax.function,
            TokenKind::Constructor => syntax.constructor,
            TokenKind::Operator => syntax.operator,
            TokenKind::Arrow => syntax.operator,
            TokenKind::Reference => syntax.operator,
            TokenKind::Namespace => syntax.namespace,
            TokenKind::Punctuation => syntax.punctuation,
            TokenKind::Macro => syntax.macro_call,
            TokenKind::SelfKeyword => syntax.self_keyword,
            TokenKind::Variable => syntax.variable,
            TokenKind::Parameter => syntax.variable,
            TokenKind::Field => syntax.variable,
            TokenKind::Lifetime => syntax.escape,
            TokenKind::ModulePath => syntax.module_path,
            TokenKind::Error => extended.danger.strong.color,
            TokenKind::Rainbow1 => syntax.rainbow[0],
            TokenKind::Rainbow2 => syntax.rainbow[1],
            TokenKind::Rainbow3 => syntax.rainbow[2],
            TokenKind::Rainbow4 => syntax.rainbow[3],
            TokenKind::Rainbow5 => syntax.rainbow[4],
            TokenKind::Rainbow6 => syntax.rainbow[5],
            _ => syntax.variable,
        };

        HighlightStyle {
            color: Some(color),
            style,
        }
    }
}

// =============================================================================
// Syntax Highlighting Color Scheme
// =============================================================================

/// A set of colors for syntax highlighting in text editors.
///
/// These colors are derived from the theme's extended palette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyntaxHighlight {
    pub keyword: Color,
    pub type_name: Color,
    pub function: Color,
    pub string: Color,
    pub number: Color,
    pub comment: Color,
    pub operator: Color,
    pub module_path: Color,
    pub punctuation: Color,
    pub variable: Color,
    pub constant: Color,
    pub macro_call: Color,
    pub namespace: Color,
    pub self_keyword: Color,
    pub escape: Color,
    pub constructor: Color,
    pub rainbow: [Color; 6],
}

impl SyntaxHighlight {
    /// Generate a syntax highlighting color scheme from an iced extended palette.
    ///
    /// The colors are automatically adjusted for light/dark themes.
    pub fn generate(extended: &Palette) -> Self {
        let is_dark = extended.is_dark;

        let adjust = |color: Color, hue_shift: f32, sat_mult: f32, light_target: f32| -> Color {
            let mut c = to_oklch(color);
            c.h += hue_shift;
            c.c = (c.c * sat_mult).clamp(0.0, 0.25);

            if is_dark {
                c.l = c.l * light_target;
            } else {
                let inv = 1.0 - c.l;
                c.l = inv * light_target;
            }
            from_oklch(c)
        };

        let keyword = adjust(extended.danger.strong.color, 0.0, 0.75, 1.00);
        let self_keyword = adjust(extended.danger.strong.color, -0.5, 1.30, 1.50);
        let function = adjust(extended.success.strong.color, -0.5, 1.30, 1.40);
        let type_name = adjust(extended.primary.strong.color, -0.5, 1.30, 1.65);
        let namespace = function;
        let macro_call = self_keyword;
        let string = adjust(extended.warning.base.color, 0.20, 1.00, 1.00);
        let number = self_keyword;
        let constant = adjust(extended.danger.base.color, -0.3, 0.8, 1.0);
        let variable = extended.background.base.text;
        let constructor = type_name;
        let module_path = adjust(extended.success.strong.color, 0.0, 0.75, 0.90);

        let comment = {
            let mut c = to_oklch(extended.background.strong.color);
            c.c = 0.02;
            c.l = if is_dark { 0.55 } else { 0.6 };
            from_oklch(c)
        };

        let operator = adjust(extended.warning.strong.color, 0.0, 1.30, 1.00);
        let punctuation = variable;

        let rainbow = [
            adjust(extended.danger.strong.color, 0.0, 0.75, 1.00),
            adjust(extended.warning.base.color, 0.0, 1.30, 1.20),
            adjust(extended.success.strong.color, -0.5, 1.30, 1.40),
            adjust(extended.primary.strong.color, -0.5, 1.30, 1.65),
            adjust(extended.warning.strong.color, 0.0, 1.30, 1.00),
            adjust(extended.danger.strong.color, -0.5, 1.30, 1.50),
        ];

        Self {
            keyword,
            type_name,
            function,
            string,
            number,
            comment,
            operator,
            module_path,
            punctuation,
            variable,
            constant,
            macro_call,
            namespace,
            self_keyword,
            escape: type_name,
            constructor,
            rainbow,
        }
    }
}

// =============================================================================
// Text Editor Style
// =============================================================================

/// A default text editor style suitable for code editing.
///
/// This style uses the theme's extended palette for a consistent look.
pub fn code_gen_text_editor_style(theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let palette = theme.palette();

    let active = text_editor::Style {
        background: iced::Background::Color(palette.background.weakest.color),
        border: iced::Border {
            radius: 4.0.into(),
            width: 1.0,
            color: palette.background.weak.color,
        },
        placeholder: palette.secondary.base.color,
        value: palette.background.base.text,
        selection: palette.secondary.strong.color.scale_alpha(0.5),
    };

    match status {
        text_editor::Status::Active => active,
        text_editor::Status::Hovered => text_editor::Style {
            border: iced::Border {
                color: palette.background.base.text,
                ..active.border
            },
            ..active
        },
        text_editor::Status::Focused { .. } => text_editor::Style {
            border: iced::Border {
                color: palette.primary.strong.color,
                ..active.border
            },
            ..active
        },
        text_editor::Status::Disabled => text_editor::Style {
            background: iced::Background::Color(palette.background.weakest.color),
            value: active.placeholder,
            placeholder: palette.background.strongest.color,
            ..active
        },
    }
}

// =============================================================================
// Color Conversion Utilities (Oklch color space, copied iced implementation since it's not public)
// =============================================================================

struct Oklch {
    l: f32,
    c: f32,
    h: f32,
    a: f32,
}

fn to_oklch(color: Color) -> Oklch {
    let [r, g, b, alpha] = color.into_linear();

    let l = 0.41222146 * r + 0.53633255 * g + 0.051445995 * b;
    let m = 0.2119035 * r + 0.6806995 * g + 0.10739696 * b;
    let s = 0.08830246 * r + 0.28171885 * g + 0.6299787 * b;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    let l = 0.21045426 * l_ + 0.7936178 * m_ - 0.004072047 * s_;
    let a = 1.9779985 * l_ - 2.4285922 * m_ + 0.4505937 * s_;
    let b = 0.025904037 * l_ + 0.78277177 * m_ - 0.80867577 * s_;

    let c = (a * a + b * b).sqrt();
    let h = b.atan2(a);

    Oklch { l, c, h, a: alpha }
}

fn from_oklch(oklch: Oklch) -> Color {
    let Oklch { l, c, h, a: alpha } = oklch;

    let a = c * h.cos();
    let b = c * h.sin();

    let l_ = l + 0.39633778 * a + 0.21580376 * b;
    let m_ = l - 0.105561346 * a - 0.06385417 * b;
    let s_ = l - 0.08948418 * a - 1.2914855 * b;

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    let r = 4.0767417 * l - 3.3077116 * m + 0.23096994 * s;
    let g = -1.268438 * l + 2.6097574 * m - 0.34131938 * s;
    let b = -0.0041960863 * l - 0.7034186 * m + 1.7076147 * s;

    Color::from_linear_rgba(
        r.clamp(0.0, 1.0),
        g.clamp(0.0, 1.0),
        b.clamp(0.0, 1.0),
        alpha,
    )
}
