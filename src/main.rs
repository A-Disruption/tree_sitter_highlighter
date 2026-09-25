use iced::widget::{column, container, pick_list, text_editor, text, row};
use iced::{Element, Length, Task, Theme};
use std::sync::Arc;

use tree_sitter_highlighter::{
    TsSettings, TreeSitterIcedHighlighter, SyntaxHighlight, code_gen_text_editor_style
};

pub struct CodeView {
    content: text_editor::Content,
    theme: Theme,
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
        }
    }

    pub fn theme(&self) -> Theme {
        self.theme.clone()
    }

    pub fn view(state: &CodeView) -> Element<'_, Message> {
        let settings = TsSettings {
            text: Arc::<str>::from(state.content.text()),
        };
        let extended = state.theme.palette();
        let syntax = SyntaxHighlight::generate(extended);

        column!(
            pick_list(
                Some(state.theme.clone()),
                Theme::ALL,
                ToString::to_string,
            )
            .on_select(Message::ChangeTheme),
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

    pub fn update(state: &mut CodeView, message: Message) -> Task<Message> {
        match message {
            Message::Edit(action) => {
                state.content.perform(action);
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
