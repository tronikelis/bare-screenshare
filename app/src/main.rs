use iced::{Element, Task};
use std::{cell::LazyCell, sync::LazyLock};

use server::conn::TcpSendReceiveClient;

mod dbus;
mod macros;
mod pipeline;
mod ui;
mod video;

pub static TPC_SEND_RECEIVE_CLIENT: LazyLock<TcpSendReceiveClient> =
    LazyLock::new(|| TcpSendReceiveClient::new("localhost".to_string(), 3000));

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
                    let (lobby, task) = smol::block_on(async {
                        ui::Lobby::new("foo-bar".to_string()).await.unwrap()
                    });
                    self.screen = ui::Screen::Lobby(lobby);
                    task.map(ui::Message::LobbyMessage)
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
