use std::{
    os::fd::{self, AsRawFd},
    str::FromStr,
};

use futures::{channel::mpsc, prelude::*, stream};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{self as gst_app};
use iced::{
    Element, Length, Task,
    advanced::{self, widget::operation::map},
    task,
    widget::{button, column, container, image, row},
};
use smol::net::UdpSocket;

use crate::{dbus, macros, pipeline, video};
use server::{
    rpc::{JoinLobbyData, RpcUserClient, RpcUserNotifyClient, create_user_udp_socket},
    state::LobbyInfoData,
};

#[derive(Debug, Clone)]
pub enum Message {
    // Start,
    // UiVideoStream(ui::VideoStreamMessage),
    MainMenuMessage(MainMenuMessage),
    LobbyMessage(LobbyMessage),
}

pub enum Screen {
    MainMenu(MainMenu),
    Lobby(Lobby),
}

impl Screen {
    pub fn main_menu(&mut self) -> &mut MainMenu {
        if let Self::MainMenu(v) = self {
            v
        } else {
            panic!("main_menu assert");
        }
    }

    pub fn lobby(&mut self) -> &mut Lobby {
        if let Self::Lobby(v) = self {
            v
        } else {
            panic!("lobby assert");
        }
    }
}

#[derive(Debug, Clone)]
pub enum VideoStreamMessage {
    PipelineMessage(video::VideoMessage),
    FrameAllocated(Result<advanced::image::Allocation, advanced::image::Error>),
}

pub struct VideoStream {
    image_alloc_handle: Option<task::Handle>,
    image_alloc: Option<image::Allocation>,
    // drop
    _pipeline: video::VideoPipeline,
}

impl VideoStream {
    pub fn new(
        pipeline: video::VideoPipeline,
        message_rx: mpsc::Receiver<video::VideoMessage>,
    ) -> (Self, Task<VideoStreamMessage>) {
        let slf = Self {
            image_alloc_handle: None,
            image_alloc: None,
            _pipeline: pipeline,
        };

        let task = Task::stream(stream::unfold(message_rx, async |mut message_rx| {
            message_rx.recv().await.ok().map(|v| (v, message_rx))
        }))
        .map(|v| VideoStreamMessage::PipelineMessage(v));

        (slf, task)
    }

    pub fn view<T>(&self) -> Element<'_, T> {
        self.image_alloc.clone().map(|v| image(v.handle())).into()
    }

    pub fn update(&mut self, message: VideoStreamMessage) -> Task<VideoStreamMessage> {
        match message {
            VideoStreamMessage::PipelineMessage(message) => match message {
                video::VideoMessage::Frame(bytes, caps) => {
                    let structure = caps.structure(0).unwrap();
                    let width: i32 = structure.get("width").unwrap();
                    let height: i32 = structure.get("height").unwrap();

                    let (task, abort) =
                        image::allocate(image::Handle::from_rgba(width as _, height as _, bytes))
                            .map(|v| VideoStreamMessage::FrameAllocated(v))
                            .abortable();
                    self.image_alloc_handle = Some(abort.abort_on_drop());

                    task
                }
            },
            VideoStreamMessage::FrameAllocated(allocation) => {
                self.image_alloc = Some(allocation.unwrap());
                Task::none()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum MainMenuMessage {
    CreateLobby,
}

pub struct MainMenu {}

impl MainMenu {
    pub fn new() -> Self {
        Self {}
    }

    pub fn view(&self) -> Element<'_, MainMenuMessage> {
        container(column!(
            button("Create Lobby").on_press(MainMenuMessage::CreateLobby)
        ))
        .center(Length::Fill)
        .into()
    }
}

struct MyStream {
    stream: VideoStream,
    udpsink: gst::Element,
    udp_valve: gst::Element,
    // drop
    _pipewire_fd: fd::OwnedFd,
    _screen_cast_proxy: dbus::ScreenCastProxy,
}

impl MyStream {
    fn new() -> (Self, Task<VideoStreamMessage>) {
        let bus_connection = dbus::bus_connection_get_session();
        let screen_cast_proxy = dbus::ScreenCastProxy::new(bus_connection);

        screen_cast_proxy.select_sources().unwrap();

        let pipewire_node_id = screen_cast_proxy.start().unwrap();
        let pipewire_fd = screen_cast_proxy.open_pipewire_remote();

        let pipewiresrc = gst::ElementFactory::make("pipewiresrc")
            .property("do-timestamp", true)
            .property("fd", pipewire_fd.as_raw_fd())
            .property("path", pipewire_node_id.to_string())
            .build()
            .unwrap();

        let tee = gst::ElementFactory::make("tee").build().unwrap();

        let videoconvertscale1 = gst::ElementFactory::make("videoconvertscale")
            .build()
            .unwrap();
        let videoconvertscale2 = gst::ElementFactory::make("videoconvertscale")
            .build()
            .unwrap();

        let queue1 = gst::ElementFactory::make("queue")
            .property_from_str("leaky", "downstream")
            .build()
            .unwrap();
        let queue2 = gst::ElementFactory::make("queue").build().unwrap();

        let encoder = gst::ElementFactory::make("x264enc")
            .property_from_str("tune", "zerolatency")
            .build()
            .unwrap();
        let payloader = gst::ElementFactory::make("rtph264pay").build().unwrap();

        let appsink = gst_app::AppSink::builder()
            .drop(true)
            .caps(&gst::Caps::from_str("video/x-raw,format=RGBA").unwrap())
            .property("emit-signals", true)
            .build();

        let udp_valve = gst::ElementFactory::make("valve")
            .property("drop", true)
            .build()
            .unwrap();

        let udpsink = gst::ElementFactory::make("udpsink")
            .property("port", 3000)
            .property("async", false)
            .build()
            .unwrap();

        let gst_pipeline = pipeline::Pipeline::new(gst::Pipeline::new())
            .link([&pipewiresrc, &tee])
            .link([&tee, &queue1, &videoconvertscale1, &appsink.clone().into()])
            .link([
                &tee,
                &queue2,
                &udp_valve,
                &videoconvertscale2,
                &encoder,
                &payloader,
                &udpsink,
            ])
            .gst_pipeline();

        let (message_tx, message_rx) = mpsc::channel(1);
        let (stream, task) = VideoStream::new(
            video::VideoPipeline::new(message_tx, gst_pipeline, appsink),
            message_rx,
        );

        (
            Self {
                stream,
                udpsink,
                udp_valve,
                _pipewire_fd: pipewire_fd,
                _screen_cast_proxy: screen_cast_proxy,
            },
            task,
        )
    }
}

pub struct Lobby {
    id: String,
    my_stream: Option<MyStream>,
    rpc_client: RpcUserClient,
    udp_socket: UdpSocket,
}

#[derive(Debug, Clone)]
pub enum LobbyMessage {
    StartStream,
    StopStream,
    Leave,
    VideoStreamMessage(VideoStreamMessage),
    RpcNotify(Result<LobbyInfoData, String>),
}

impl Lobby {
    pub async fn new(id: String) -> anyhow::Result<(Self, Task<LobbyMessage>)> {
        let (tcp_id, sender, receiver) = crate::TPC_SEND_RECEIVE_CLIENT.create().await?;

        let notify_stream = RpcUserNotifyClient::new(receiver.into()).stream();

        let task = Task::stream(notify_stream)
            .map(|v| LobbyMessage::RpcNotify(v.map_err(|e| e.to_string())));

        let mut rpc_client = RpcUserClient::new(sender.into());
        rpc_client
            .join_lobby(JoinLobbyData { id: id.clone() })
            .await?;

        let udp_socket = create_user_udp_socket(&tcp_id).await?;

        Ok((
            Self {
                id,
                rpc_client,
                udp_socket,
                my_stream: None,
            },
            task,
        ))
    }

    fn view_clients(&self) -> Element<'_, LobbyMessage> {
        column!("Clients").into()
    }

    fn view_my_stream(&self) -> Element<'_, LobbyMessage> {
        container(match &self.my_stream {
            Some(v) => Element::<LobbyMessage>::from(column!(
                button("Stop Stream").on_press(LobbyMessage::StopStream),
                v.stream.view().map(LobbyMessage::VideoStreamMessage),
            )),
            None => button("Start stream")
                .on_press(LobbyMessage::StartStream)
                .into(),
        })
        .width(Length::Fill)
        .into()
    }

    pub fn view(&self) -> Element<'_, LobbyMessage> {
        column!(
            container(row!(
                self.id.as_str(),
                button("Leave").on_press(LobbyMessage::Leave)
            )),
            row!(self.view_my_stream(), self.view_clients(),),
        )
        .into()
    }

    pub fn update(&mut self, message: LobbyMessage) -> Task<LobbyMessage> {
        match message {
            LobbyMessage::VideoStreamMessage(v) => self
                .my_stream
                .as_mut()
                .unwrap()
                .stream
                .update(v)
                .map(LobbyMessage::VideoStreamMessage),
            LobbyMessage::StartStream => {
                let (my_stream, task) = MyStream::new();
                self.my_stream = Some(my_stream);
                task.map(LobbyMessage::VideoStreamMessage)
            }
            LobbyMessage::StopStream => {
                self.my_stream = None;
                Task::none()
            }
            LobbyMessage::RpcNotify(v) => match v {
                Ok(v) => {
                    println!("got message: {v:#?}");
                    Task::none()
                }
                Err(e) => {
                    // todo: retry connection
                    println!("rpc notify failed: {e}");
                    Task::none()
                }
            },
            LobbyMessage::Leave => unreachable!("should be handled above"),
        }
    }
}
