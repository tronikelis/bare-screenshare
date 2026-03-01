use iced::{
    Element,
    widget::{button, column, text},
};

mod dbus;
mod macros;
mod ui;
mod video;

#[derive(Debug, Clone)]
enum Message {
    Start,
    UiVideoStream(ui::VideoStreamMessage),
}

struct App {
    ui_video_stream: Option<ui::VideoStream>,
}

impl App {
    fn new() -> Self {
        Self {
            ui_video_stream: None,
        }
    }

    fn view(&self) -> Element<'_, Message> {
        column![
            button("Start").on_press(Message::Start),
            text("nice"),
            self.ui_video_stream.as_ref().map(|v| v.view()),
        ]
        .spacing(10)
        .into()
    }

    fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::Start => {
                let (video_stream, task) = ui::VideoStream::new();
                self.ui_video_stream = Some(video_stream);
                task.map(|v| Message::UiVideoStream(v))
            }
            Message::UiVideoStream(v) => self
                .ui_video_stream
                .as_mut()
                .map(move |video_stream| video_stream.update(v))
                .unwrap_or(iced::Task::none())
                .map(|v| Message::UiVideoStream(v)),
        }
    }
}

fn main() {
    iced::application(App::new, App::update, App::view)
        .run()
        .unwrap()
}
