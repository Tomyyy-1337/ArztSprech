use iced::theme::Palette;
use iced::widget::{button, column, container, pick_list, row, rich_text, scrollable, span, text, text_editor, text_input};
use iced::{Background, Border, Color, Element, Fill, Length, Padding, Theme};

use arztsprech_core::protocol::Model;

use super::state::{App, Message, SYSTEM_MIC, model_label};

pub(super) fn app_theme() -> Theme {
    Theme::custom(
        "Arztsprech".to_string(),
        Palette {
            background: rgb(22, 23, 28),
            text: rgb(220, 222, 228),
            primary: rgb(88, 130, 255),
            success: rgb(90, 200, 120),
            warning: rgb(230, 180, 60),
            danger: rgb(235, 70, 70),
        },
    )
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

fn surface_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(rgb(30, 32, 38))),
        ..Default::default()
    }
}

fn outline_style(_: &Theme) -> container::Style {
    container::Style {
        border: Border { color: rgb(88, 92, 104), width: 1.0, radius: 6.0.into() },
        ..Default::default()
    }
}

/// Wrapper so `pick_list` can display a `Model` via `model_label`.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ModelChoice(Model);

impl std::fmt::Display for ModelChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", model_label(self.0))
    }
}

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        if self.show_settings {
            return self.settings_view();
        }
        let status_color = rgb(120, 160, 255);
        let mut status_row = row![container(text(format!("Status: {}", self.status)).size(12).color(status_color)).width(Fill).max_height(32).clip(true)]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center);
        let settings_icon = container(text("⚙").size(20)).padding(Padding { top: 4.0, ..Default::default() }).center_x(Fill).center_y(Fill);
        let settings_button = button(settings_icon).on_press(Message::ToggleSettings).style(button::primary).padding(0).width(32).height(32);
        status_row = status_row.push(settings_button);
        let status_bar = container(status_row).padding([6, 10]).width(Fill).style(surface_style);

        let live_style: fn(&Theme, button::Status) -> button::Style = if self.live { button::success } else { button::primary };
        let live_button = button("Automatisch einfügen").on_press(Message::ToggleLive(!self.live)).style(live_style).padding([6, 12]).width(Fill);

        let toolbar_row = row![
            button("Kopieren").on_press(Message::Copy).style(button::primary).padding([6, 12]).width(Length::FillPortion(1)),
            button("Löschen").on_press(Message::Clear).style(button::primary).padding([6, 12]).width(Length::FillPortion(1)),
            live_button.width(Length::FillPortion(2)),
        ]
        .spacing(8)
        .align_y(iced::alignment::Vertical::Center);
        let toolbar = container(toolbar_row).padding([6, 10]).width(Fill).style(surface_style);

        let body = column![status_bar, self.transcript_view(), toolbar].spacing(8).padding(8);
        container(body).width(Fill).height(Fill).into()
    }

    fn settings_view(&self) -> Element<'_, Message> {
        let header = row![
            text("Einstellungen").size(20).width(Fill),
            button("X").on_press(Message::ToggleSettings),
        ]
        .align_y(iced::alignment::Vertical::Center);

        let mut content = column![].spacing(8).width(Fill);
        if cfg!(feature = "remote") {
            content = content
                .push(text("Server (IP:Port)"))
                .push(
                    row![
                        text_input("127.0.0.1:7878", &self.server_input)
                            .on_input(Message::ServerInput)
                            .on_submit(Message::ApplyServer)
                            .width(Fill),
                        button("Übernehmen").on_press(Message::ApplyServer),
                    ]
                    .spacing(8),
                )
                .push(text(format!("Aktuell: {}", self.server)).color(rgb(140, 144, 155)));
        }
        let models: Vec<ModelChoice> = Model::ALL.map(ModelChoice).to_vec();
        let mut mic_options = vec![SYSTEM_MIC.to_string()];
        mic_options.extend(self.mic_options.iter().cloned());
        content = content
            .push(text("Modell"))
            .push(
                pick_list(models, Some(ModelChoice(self.model)), |choice: ModelChoice| {
                    Message::ModelSelected(choice.0)
                })
                .width(Fill),
            )
            .push(text("Mikrofon"))
            .push(
                pick_list(mic_options, Some(self.mic.clone().unwrap_or_else(|| SYSTEM_MIC.to_string())), Message::MicSelected)
                    .width(Fill),
            )
            .push(text("Systemprompt"))
            .push(container(text_editor(&self.prompt_input).on_action(Message::PromptEdit).height(200)).style(outline_style))
            .push(button("Systemprompt übernehmen").on_press(Message::ApplyPrompt));

        let page = column![header, scrollable(content).width(Fill).height(Fill)].spacing(12);
        container(page)
            .padding(12)
            .width(Fill)
            .height(Fill)
            .style(surface_style)
            .into()
    }

    fn transcript_view(&self) -> Element<'_, Message> {
        let mut spans = vec![span::<(), _>(self.text.clone())];
        if !self.partial.is_empty() {
            let sep = if self.text.is_empty() { "" } else { " " };
            spans.push(span::<(), _>(format!("{sep}{}", self.partial)).color(rgb(120, 124, 135)));
        }
        container(
            scrollable(rich_text(spans).width(Fill))
                .anchor_bottom()
                .width(Fill)
                .height(Length::Fill),
        )
        .padding(8)
        .width(Fill)
        .height(Length::Fill)
        .style(outline_style)
        .into()
    }
}
