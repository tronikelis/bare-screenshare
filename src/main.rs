use iced::{Element, Task};

mod dbus;
mod macros;
mod pipeline;
mod ui;
mod video;

struct App {
    screen: ui::Screen,
}

impl App {
    fn new() -> Self {
        Self {
            screen: ui::Screen::MainMenu(ui::MainMenu::new()),
        }
    }

    fn view(&self) -> Element<'_, ui::Message> {
        match &self.screen {
            ui::Screen::MainMenu(v) => v.view().map(ui::Message::MainMenuMessage),
            ui::Screen::Lobby(v) => v.view().map(ui::Message::LobbyMessage),
        }
    }

    fn update(&mut self, message: ui::Message) -> Task<ui::Message> {
        match message {
            ui::Message::MainMenuMessage(v) => match v {
                ui::MainMenuMessage::CreateLobby => {
                    self.screen = ui::Screen::Lobby(ui::Lobby::new(String::from("foo-bar")));
                    Task::none()
                }
            },
            ui::Message::LobbyMessage(v) => match v {
                ui::LobbyMessage::Leave => {
                    self.screen = ui::Screen::MainMenu(ui::MainMenu::new());
                    Task::none()
                }
                v => self.screen.lobby().update(v).map(ui::Message::LobbyMessage),
            },
        }
    }
}

fn main() {
    gstreamer::init().unwrap();
    iced::application(App::new, App::update, App::view)
        .run()
        .unwrap()
}
