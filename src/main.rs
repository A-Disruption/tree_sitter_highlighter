use iced::widget::{column, container, pick_list, text_editor, text, row};
use iced::{Element, Length, Task, Theme};
use iced::Color;
use iced::font::Font;
use iced::advanced::text::Highlighter;
use iced::advanced::text::highlighter::Format;
use iced::theme::palette::Extended;
use tree_sitter_highlighter::{Highlighter as tsHighlighter, TokenKind};
use std::ops::Range;
use std::sync::Arc;

pub struct CodeView {
    content: text_editor::Content,
    theme: Theme,
    full_text: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Edit(text_editor::Action),
    ChangeTheme(Theme),
}

impl CodeView {
    pub fn new() -> Self {
        Self {
            content: text_editor::Content::new(),
            theme: Theme::Dark,
            full_text: String::new(),
        }
    }

    pub fn theme(&self) -> Theme {
        self.theme.clone()
    }

    pub fn view(state: &CodeView) -> Element<'_, Message> {
        let settings = TsSettings {
            text: Arc::<str>::from(state.content.text()),
        };
        let extended = state.theme.extended_palette();

        // Generate the syntax highlighting color scheme
        let syntax = SyntaxHighlight::generate(extended);

        column!(
            pick_list(
                Theme::ALL,
                Some(state.theme.clone()),
                Message::ChangeTheme,
            ),
            text_editor(&state.content)
                .placeholder("Type something here...")
                .highlight_with::<TreeSitterIcedHighlighter>(
                    settings,
                    TreeSitterIcedHighlighter::to_format,
                )
                .on_action(Message::Edit)
                .height(Length::Fill)
                .style(code_gen_text_editor_style)
                .size(14.0),
            row![
                container(text("Keyword")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.keyword)), ..Default::default() }),
                container(text("Type")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.type_name)), ..Default::default() }),
                container(text("Function")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.function)), ..Default::default() }),
                container(text("String")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.string)), ..Default::default() }),
                container(text("Number")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.number)), ..Default::default() }),
                container(text("Comment")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.comment)), ..Default::default() }),
                container(text("Operator")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.operator)), ..Default::default() }),
                container(text("punctuation")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.punctuation)), ..Default::default() }),
                container(text("Variable")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.variable)), ..Default::default() }),
                container(text("Constant")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.constant)), ..Default::default() }),
                container(text("Macro")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.macro_call)), ..Default::default() }),
                container(text("Namespace")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.namespace)), ..Default::default() }),
                container(text("Self")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.self_keyword)), ..Default::default() }),
                container(text("Escape")).style(move |_| container::Style { background: Some(iced::Background::Color(syntax.escape)), ..Default::default() }),
            ].spacing(10.0).padding(10),
        ).into()
    }

    pub fn update(state: &mut CodeView, message: Message) -> Task<Message>  {
        match message {
            Message::Edit(action) => {
                state.content.perform(action);
                state.full_text = state.content.text();
            }

            Message::ChangeTheme(theme) => {
                state.theme = theme;
            }
        }
        Task::none()
    }
}

fn main() -> iced::Result {
    iced::application(
        CodeView::new,
        CodeView::update,
        CodeView::view,
    )
    .theme(CodeView::theme)
    .run()
}

pub fn code_gen_text_editor_style(theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let palette = theme.extended_palette();

    let active = text_editor::Style {
        background: iced::Background::Color(palette.background.weakest.color),
        border: iced::Border {
            radius: 4.0.into(),
            width: 1.0,
            color: palette.background.weak.color,
        },
        placeholder: palette.secondary.base.color,
        value: palette.background.base.text,
        selection: palette.primary.weak.color,
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

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TsSettings {
    pub text: Arc<str>,
}

/// Adapts your document-level Tree-sitter highlighter to iced's line-by-line `Highlighter` trait.
///
/// iced will:
/// - call `change_line(i)` when line `i` changed
/// - then call `highlight_line` for line `i`, `i+1`, ... in order
pub struct TreeSitterIcedHighlighter {
    ts: tsHighlighter,
    settings: TsSettings,

    // sequential line state
    current: usize,
    // byte offsets of each line start in the full text
    line_starts: Vec<usize>,
}

impl Highlighter for TreeSitterIcedHighlighter {
    type Settings = TsSettings;
    type Highlight = TokenKind;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Self::Highlight)>;

    fn new(settings: &Self::Settings) -> Self {
        let mut ts = tsHighlighter::new().expect("ts init");
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

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        let text = self.settings.text.as_ref();

        // Compute this line's global byte range from line_starts
        let line_start = self.line_starts.get(self.current).copied().unwrap_or(text.len());
        let line_end = if self.current + 1 < self.line_starts.len() {
            self.line_starts[self.current + 1].saturating_sub(1) // exclude '\n'
        } else {
            text.len()
        };

        // Safety clamp (line param should match this slice length)
        let line_len = line.len();
        let expected_len = line_end.saturating_sub(line_start);
        let effective_end = line_start + expected_len.min(line_len);

        let spans = match self.ts.highlight_range(text, line_start..effective_end) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("highlight_range error: {e:?}");
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
    pub fn to_format(kind: &TokenKind, theme: &Theme) -> Format<Font> {
        let extended = theme.extended_palette();
        let syntax = SyntaxHighlight::generate(extended);

        // Use a generic font or specific one if you have it loaded
        let font = None; 

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
            // Rainbow brackets - 6 colors: Red, Yellow, Green, Blue, Orange, Pink
            TokenKind::Rainbow1 => syntax.rainbow[0],
            TokenKind::Rainbow2 => syntax.rainbow[1],
            TokenKind::Rainbow3 => syntax.rainbow[2],
            TokenKind::Rainbow4 => syntax.rainbow[3],
            TokenKind::Rainbow5 => syntax.rainbow[4],
            TokenKind::Rainbow6 => syntax.rainbow[5],
            _ => syntax.variable,
        };

        Format {
            color: Some(color),
            font,
        }
    }
}

/// A set of colors for syntax highlighting in text editors.
///
/// These colors are derived from the palette and optimized for code readability
/// on various backgrounds. They maintain sufficient contrast while providing
/// distinct colors for different syntax elements.
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
    // Rainbow brackets - 6 colors shared by all bracket types
    pub rainbow: [Color; 6],
}

impl SyntaxHighlight {
    pub fn generate(extended: &Extended) -> Self {
        let is_dark = extended.is_dark;

        // Theme-aware color tweaker
        let adjust = |color: Color, hue_shift: f32, sat_mult: f32, light_target: f32| -> Color {
            let mut c = to_oklch(color);
            c.h += hue_shift;
            c.c = (c.c * sat_mult).clamp(0.0, 0.25); // Keep low saturation for Everforest/Modern look
            
            if is_dark {
                c.l = c.l * light_target;
            } else {
                let inv = 1.0 - c.l;
                c.l = inv * light_target;
            }
            from_oklch(c)
        };

        // --- Theme::Dark <-> EVERFOREST DARK MAPPING ---

        // Keywords (pub, fn, struct): Red/Soft Pink
        // We use danger (usually Red) as base.
        let keyword = adjust(extended.danger.strong.color, 0.0, 0.75, 1.00);

        // Self keyword (self, Self)
        let self_keyword = adjust(extended.danger.strong.color, -0.5, 1.30, 1.50);

        // Functions (new, title)
        let function = adjust(extended.success.strong.color, -0.5, 1.30, 1.40);

        // Types (String, App)
        let type_name = adjust(extended.primary.strong.color, -0.5, 1.30, 1.65);

        // Namespaces (iced::, std::)
        let namespace = function;

        // Macros/Attributes (derive)
        let macro_call = self_keyword;

        // Strings
        let string = adjust(extended.warning.base.color, 0.20, 1.00, 1.00);

        // Numbers
        let number = self_keyword; 

        // Constants (Enum variants)
        let constant = adjust(extended.danger.base.color, -0.3, 0.8, 1.0); // Purple-ish

        // Variables:
        let variable = extended.background.base.text;

        // Constructors:
        let constructor = type_name;

        // ModulePath (in fn main() )
        let module_path = adjust(extended.success.strong.color, 0.0, 0.75, 0.90);

        // Comments: Grey (Muted)
        let comment = {
            let mut c = to_oklch(extended.background.strong.color);
            c.c = 0.02; // Desaturate
            c.l = if is_dark { 0.55 } else { 0.6 }; 
            from_oklch(c)
        };

        // Operators ( :: -> & ):
        let operator = adjust(extended.warning.strong.color, 0.0, 1.30, 1.00);
        let punctuation = variable;

        // Rainbow brackets - 6 colors: Red, Yellow, Green, Blue, Orange, Pink
        let rainbow = [
            adjust(extended.danger.strong.color, 0.0, 0.75, 1.00),    // Red
            adjust(extended.warning.base.color, 0.0, 1.30, 1.20),    // Yellow
            adjust(extended.success.strong.color, -0.5, 1.30, 1.40), // Green
            adjust(extended.primary.strong.color, -0.5, 1.30, 1.65), // Blue
            adjust(extended.warning.strong.color, 0.0, 1.30, 1.00),  // Orange
            adjust(extended.danger.strong.color, -0.5, 1.30, 1.50),  // Pink
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

struct Oklch {
    l: f32,
    c: f32,
    h: f32,
    a: f32,
}

// https://en.wikipedia.org/wiki/Oklab_color_space#Conversions_between_color_spaces
fn to_oklch(color: Color) -> Oklch {
    let [r, g, b, alpha] = color.into_linear();

    // linear RGB → LMS
    let l = 0.41222146 * r + 0.53633255 * g + 0.051445995 * b;
    let m = 0.2119035 * r + 0.6806995 * g + 0.10739696 * b;
    let s = 0.08830246 * r + 0.28171885 * g + 0.6299787 * b;

    // Nonlinear transform (cube root)
    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    // LMS → Oklab
    let l = 0.21045426 * l_ + 0.7936178 * m_ - 0.004072047 * s_;
    let a = 1.9779985 * l_ - 2.4285922 * m_ + 0.4505937 * s_;
    let b = 0.025904037 * l_ + 0.78277177 * m_ - 0.80867577 * s_;

    // Oklab → Oklch
    let c = (a * a + b * b).sqrt();
    let h = b.atan2(a); // radians

    Oklch { l, c, h, a: alpha }
}

// https://en.wikipedia.org/wiki/Oklab_color_space#Conversions_between_color_spaces
fn from_oklch(oklch: Oklch) -> Color {
    let Oklch { l, c, h, a: alpha } = oklch;

    let a = c * h.cos();
    let b = c * h.sin();

    // Oklab → LMS (nonlinear)
    let l_ = l + 0.39633778 * a + 0.21580376 * b;
    let m_ = l - 0.105561346 * a - 0.06385417 * b;
    let s_ = l - 0.08948418 * a - 1.2914855 * b;

    // Cubing back
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