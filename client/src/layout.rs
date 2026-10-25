use iced::theme::Palette;
use iced::widget::{button, column, container, mouse_area, pick_list, row, rich_text, scrollable, slider, span, text, text_editor, text_input, toggler};
use iced::{Background, Border, Color, Element, Fill, Length, Padding, Theme};

use arztsprech_core::protocol::Model;

use super::state::{App, Message, PANEL_WIDTH, SYSTEM_MIC, model_label};

/// Formatiert Sekunden als `m:ss`.
fn fmt_time(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub(super) fn app_theme(dark: bool) -> Theme {
    let (name, palette) = if dark {
        (
            "Arztsprech Dunkel",
            Palette {
                background: rgb(22, 23, 28),
                text: rgb(220, 222, 228),
                primary: rgb(88, 130, 255),
                success: rgb(90, 200, 120),
                warning: rgb(230, 180, 60),
                danger: rgb(235, 70, 70),
            },
        )
    } else {
        (
            "Arztsprech Hell",
            Palette {
                background: rgb(250, 250, 252),
                text: rgb(30, 32, 38),
                primary: rgb(60, 110, 240),
                success: rgb(40, 160, 90),
                warning: rgb(200, 140, 20),
                danger: rgb(210, 50, 50),
            },
        )
    };
    Theme::custom(name.to_string(), palette)
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

fn is_dark(theme: &Theme) -> bool {
    let background = theme.palette().background;
    (background.r + background.g + background.b) / 3.0 < 0.5
}

fn surface_style(theme: &Theme) -> container::Style {
    let color = if is_dark(theme) { rgb(30, 32, 38) } else { rgb(232, 234, 240) };
    container::Style {
        background: Some(Background::Color(color)),
        ..Default::default()
    }
}

fn outline_style(theme: &Theme) -> container::Style {
    let color = if is_dark(theme) { rgb(88, 92, 104) } else { rgb(170, 174, 186) };
    container::Style {
        border: Border { color, width: 1.0, radius: 6.0.into() },
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

/// Wrapper so `pick_list` can display the design choice; `true` means dark.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ThemeChoice(bool);

impl std::fmt::Display for ThemeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", if self.0 { "Dunkel" } else { "Hell" })
    }
}

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        if self.show_settings {
            return self.settings_view();
        }
        let status_color = if self.dark { rgb(120, 160, 255) } else { rgb(40, 90, 200) };
        let mut status_row = row![container(text(format!("Status: {}", self.status)).size(12).color(status_color)).width(Fill).max_height(32).clip(true)]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center);
        let settings_icon = container(text("⚙").size(20)).padding(Padding { top: 4.0, ..Default::default() }).center_x(Fill).center_y(Fill);
        let settings_button = button(settings_icon).on_press(Message::ToggleSettings).style(button::primary).padding(0).width(32).height(32);
        let panel_style: fn(&Theme, button::Status) -> button::Style = if self.panel_open { button::success } else { button::primary };
        let panel_icon = container(text("☰").size(16)).center_x(Fill).center_y(Fill);
        let panel_button = button(panel_icon).on_press(Message::TogglePanel).style(panel_style).padding(0).width(32).height(32);
        status_row = status_row.push(panel_button);
        status_row = status_row.push(settings_button);
        let status_bar = container(status_row).padding([6, 10]).width(Fill).style(surface_style);

        let record_style: fn(&Theme, button::Status) -> button::Style = if self.recording { button::success } else { button::primary };
        let record_button = button("Aufnahme").on_press(Message::ToggleRecording(!self.recording)).style(record_style).padding([6, 12]).width(Fill);

        let toolbar_row = row![
            button("Kopieren").on_press(Message::Copy).style(button::primary).padding([6, 12]).width(Length::FillPortion(1)),
            button("Löschen").on_press(Message::Clear).style(button::primary).padding([6, 12]).width(Length::FillPortion(1)),
            record_button.width(Length::FillPortion(2)),
        ]
        .spacing(8)
        .align_y(iced::alignment::Vertical::Center);
        let toolbar = container(toolbar_row).padding([6, 10]).width(Fill).style(surface_style);

        let body = column![status_bar, self.transcript_view(), toolbar].spacing(8).padding(8);
        let main = container(body).width(Fill).height(Fill);
        if self.panel_open {
            row![main, self.clip_panel()].height(Fill).into()
        } else {
            main.into()
        }
    }

    fn clip_panel(&self) -> Element<'_, Message> {
        let mut list = column![].spacing(6);
        for clip in self.clips.iter().rev() {
            let end = clip.end.unwrap_or_else(|| self.started.elapsed());
            let label = format!("{} · {}", clip.clock, fmt_time(end.saturating_sub(clip.start).as_secs_f64()));
            let selected = self.selected_clip == Some(clip.id);
            let style: fn(&Theme, button::Status) -> button::Style = if selected { button::success } else { button::secondary };
            let mut entry = button(text(label).size(13)).style(style).padding([6, 10]).width(Fill);
            if clip.end.is_some() {
                entry = entry.on_press(Message::SelectClip(clip.id));
            }
            list = list.push(mouse_area(entry).on_enter(Message::HoverClip(Some(clip.id))).on_exit(Message::HoverClip(None)));
        }

        let header = row![text("Aufnahmen").size(14), button(text("Speicherort öffnen").size(12)).on_press(Message::OpenAudioFolder).style(button::secondary).padding([4, 8])]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center);
        let mut panel = column![header, scrollable(list).width(Fill).height(Fill)].spacing(8);
        if let Some(player) = self.player.as_ref() {
            let pos = player.position_secs();
            let dur = player.duration_secs();
            let playing = !(player.is_paused() || player.is_finished());
            let toggle_label = if playing { "Pause" } else { "Abspielen" };
            let controls = row![
                button(text(toggle_label).size(13)).on_press(Message::TogglePlayback).style(button::primary).padding([6, 12]),
                text(format!("{} / {}", fmt_time(pos), fmt_time(dur))).size(12),
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center);
            let timeline = slider(0.0..=dur.max(0.001), pos.min(dur), Message::Seek).step(0.1);
            panel = panel.push(column![controls, timeline].spacing(6));
        }

        container(panel).padding(8).width(PANEL_WIDTH).height(Fill).style(surface_style).into()
    }

    fn settings_view(&self) -> Element<'_, Message> {
        let header = row![
            text("Einstellungen").size(20).width(Fill),
            button("X").on_press(Message::ToggleSettings),
        ]
        .align_y(iced::alignment::Vertical::Center);

        let models: Vec<ModelChoice> = Model::ALL.map(ModelChoice).to_vec();
        let mut mic_options = vec![SYSTEM_MIC.to_string()];
        mic_options.extend(self.mic_options.iter().cloned());
        let mut content = column![]
            .spacing(8)
            .width(Fill)
            .push(text("Design"))
            .push(
                pick_list(vec![ThemeChoice(true), ThemeChoice(false)], Some(ThemeChoice(self.dark)), |choice: ThemeChoice| {
                    Message::ThemeSelected(choice.0)
                })
                .width(Fill),
            )
            .push(text("Mikrofon"))
            .push(
                pick_list(mic_options, Some(self.mic.clone().unwrap_or_else(|| SYSTEM_MIC.to_string())), Message::MicSelected)
                    .width(Fill),
            )
            .push(text("Modell"))
            .push(
                pick_list(models, Some(ModelChoice(self.model)), |choice: ModelChoice| {
                    Message::ModelSelected(choice.0)
                })
                .width(Fill),
            )
            .push(
                row![
                    column![
                        text("Automatisch einfügen"),
                        container(
                            toggler(self.auto_insert)
                                .label("Erkannten Text automatisch einfügen")
                                .on_toggle(Message::AutoInsertToggled)
                                .size(22)
                                .text_size(14)
                                .spacing(4),
                        )
                        .padding([2, 10]),
                    ]
                    .spacing(8)
                    .width(Length::FillPortion(1)),
                    column![
                        text(" "),
                        container(
                            toggler(self.live_edit)
                                .label("Vorschau mit einfügen")
                                .on_toggle(Message::LiveEditToggled)
                                .size(22)
                                .text_size(14)
                                .spacing(4),
                        )
                        .padding([2, 10]),
                    ]
                    .spacing(8)
                    .width(Length::FillPortion(1)),
                ]
                .spacing(16),
            )
            .push(text("Systemprompt"))
            .push(container(text_editor(&self.prompt_input).on_action(Message::PromptEdit).height(200)).style(outline_style))
            .push(button("Systemprompt übernehmen").on_press(Message::ApplyPrompt));
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
        #[cfg(feature = "remote")]
        {
            content = content
                .push(text("Zugangsdaten"))
                .push(
                    row![
                        text_input("Benutzername", &self.username_input)
                            .on_input(Message::UsernameInput)
                            .on_submit(Message::ApplyCredentials)
                            .width(Length::FillPortion(1)),
                        text_input("Produktschlüssel", &self.product_key_input)
                            .on_input(Message::ProductKeyInput)
                            .on_submit(Message::ApplyCredentials)
                            .width(Length::FillPortion(1)),
                        button("Übernehmen").on_press(Message::ApplyCredentials),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                );
        }

        let page = column![header, scrollable(content).width(Fill).height(Fill)].spacing(12);
        container(page)
            .padding(12)
            .width(Fill)
            .height(Fill)
            .style(surface_style)
            .into()
    }

    fn transcript_view(&self) -> Element<'_, Message> {
        let window = self.hovered_clip.and_then(|id| self.clips.iter().find(|clip| clip.id == id)).map(|clip| {
            (clip.start, clip.end.unwrap_or_else(|| self.started.elapsed()))
        });
        let bold = iced::Font { weight: iced::font::Weight::Bold, ..iced::Font::DEFAULT };
        let mut spans = Vec::new();
        let mut cursor = 0;
        for segment in &self.segments {
            if segment.range.start > cursor {
                spans.push(span::<(), iced::Font>(self.text[cursor..segment.range.start].to_string()));
            }
            let in_clip = window.is_some_and(|(start, end)| segment.at >= start && segment.at < end);
            let mut piece = span::<(), iced::Font>(self.text[segment.range.clone()].to_string());
            if in_clip {
                piece = piece.font(bold);
            }
            spans.push(piece);
            cursor = segment.range.end;
        }
        if cursor < self.text.len() {
            spans.push(span::<(), iced::Font>(self.text[cursor..].to_string()));
        }
        if !self.partial.is_empty() {
            let sep = if self.text.is_empty() { "" } else { " " };
            let partial_color = if self.dark { rgb(120, 124, 135) } else { rgb(130, 134, 145) };
            spans.push(span::<(), iced::Font>(format!("{sep}{}", self.partial)).color(partial_color));
        }
        container(
            scrollable(rich_text(spans).width(Fill))
                .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().spacing(8)))
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
