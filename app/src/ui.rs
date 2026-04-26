use std::{
    io,
    net::SocketAddrV4,
    os::fd::{self, AsRawFd, RawFd},
    str::FromStr,
};

use futures::{channel::mpsc, prelude::*, stream};
use gio::prelude::*;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{self as gst_app};
use iced::{
    Element, Length, Task,
    advanced::{self, widget::operation::map},
    task,
    widget::{button, column, container, image, row},
};
use smol::net::UdpSocket;

use crate::{dbus, macros::dbg_err, pipeline, video};
use server::{
    conn::TcpId,
    rpc::{JoinLobbyData, RpcUserClient, rpc_user_notify_stream},
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
            dbg_err!(message_rx.recv().await)
                .ok()
                .map(|v| (v, message_rx))
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
    fn new(sink_socket_fd: RawFd) -> (Self, Task<VideoStreamMessage>) {
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

        let udpsink = gst::ElementFactory::make("multiudpsink")
            .property("socket", unsafe {
                gio::Socket::from_raw_fd(sink_socket_fd).unwrap()
            })
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

    fn update(&mut self, message: VideoStreamMessage) -> Task<VideoStreamMessage> {
        self.stream.update(message)
    }

    fn view(&self) -> Element<'_, VideoStreamMessage> {
        self.stream.view()
    }

    fn set_udp_sink(&self, address: &SocketAddrV4) {
        self.udp_valve.set_property("drop", false);

        println!("setting udpsink to {}", address.to_string());
        self.udpsink.set_property("clients", address.to_string());
    }
}

struct PeerStream {
    stream: VideoStream,
}

impl PeerStream {
    fn new(addr: &SocketAddrV4, src_socket_fd: RawFd) -> (Self, Task<VideoStreamMessage>) {
        let udpsrc = gst::ElementFactory::make("udpsrc")
            .property("socket", unsafe {
                // this closed????
                gio::Socket::from_raw_fd(src_socket_fd).unwrap()
            })
            .property_from_str("address", &addr.ip().to_string())
            .property_from_str("port", &addr.port().to_string())
            .property(
                "caps",
                &gst::Caps::from_str("application/x-rtp,media=video,payload=96,clock-rate=90000")
                    .unwrap(),
            )
            .build()
            .unwrap();

        let h264depay = gst::ElementFactory::make("rtph264depay").build().unwrap();
        let h264parse = gst::ElementFactory::make("avdec_h264").build().unwrap();
        let converter = gst::ElementFactory::make("videoconvertscale")
            .build()
            .unwrap();

        // TODO: VideoStream should hide this appsink somehow
        let appsink = gst_app::AppSink::builder()
            .drop(true)
            .caps(&gst::Caps::from_str("video/x-raw,format=RGBA").unwrap())
            .property("emit-signals", true)
            .build();

        let gst_pipeline = pipeline::Pipeline::new(gst::Pipeline::new())
            .link([
                &udpsrc,
                &h264depay,
                &h264parse,
                &converter,
                &appsink.clone().into(),
            ])
            .gst_pipeline();

        let (message_tx, message_rx) = mpsc::channel(1);
        let (stream, task) = VideoStream::new(
            video::VideoPipeline::new(message_tx, gst_pipeline, appsink),
            message_rx,
        );

        (PeerStream { stream }, task)
    }

    fn update(&mut self, message: VideoStreamMessage) -> Task<VideoStreamMessage> {
        self.stream.update(message)
    }

    fn view(&self) -> Element<'_, VideoStreamMessage> {
        self.stream.view()
    }
}

pub struct Lobby {
    id: String,
    my_stream: Option<MyStream>,
    peer_stream: Option<PeerStream>,
    server_client: ServerClient,
}

#[derive(Debug, Clone)]
pub enum LobbyMessage {
    StartStream,
    StopStream,
    Leave,
    VideoStreamMessage(VideoStreamMessage),
    PeerStreamMessage(VideoStreamMessage),
    RpcNotify(Result<LobbyInfoData, String>),
}

struct ServerClient {
    rpc: RpcUserClient,
    udp_socket: UdpSocket,
    tcp_id: TcpId,
}

impl ServerClient {
    async fn new() -> anyhow::Result<(Self, Task<LobbyMessage>)> {
        let (tcp_id, sender, receiver) = crate::TPC_SEND_RECEIVE_CLIENT.create().await?;

        let task = Task::stream(rpc_user_notify_stream(receiver.into()))
            .map(|v| LobbyMessage::RpcNotify(v.map_err(|e| e.to_string())));

        let rpc = RpcUserClient::new(sender.into());
        let udp_socket = UdpSocket::bind("127.0.0.1:0").await?;

        Ok((
            Self {
                rpc,
                udp_socket,
                tcp_id,
            },
            task,
        ))
    }

    async fn join_lobby(&mut self, data: JoinLobbyData) -> anyhow::Result<()> {
        self.rpc.join_lobby(data).await?;
        self.udp_socket
            .send_to(&self.tcp_id, "127.0.0.1:4000")
            .await?;
        Ok(())
    }

    async fn send_hello(&mut self, addresses: &[SocketAddrV4]) -> anyhow::Result<()> {
        let self_im = &self;
        let futs = addresses
            .into_iter()
            .map(|addr| async move {
                self_im.udp_socket.send_to(b"hello", addr).await?;
                io::Result::Ok(())
            })
            .collect::<Vec<_>>();
        for v in futures::future::join_all(futs).await {
            v?;
        }
        Ok(())
    }
}

impl Lobby {
    pub async fn new(id: String) -> anyhow::Result<(Self, Task<LobbyMessage>)> {
        let (mut client, task) = ServerClient::new().await?;
        client.join_lobby(JoinLobbyData { id: id.clone() }).await?;

        Ok((
            Self {
                id,
                server_client: client,
                my_stream: None,
                peer_stream: None,
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
                v.view().map(LobbyMessage::VideoStreamMessage),
            )),
            None => button("Start stream")
                .on_press(LobbyMessage::StartStream)
                .into(),
        })
        .width(Length::Fill)
        .into()
    }

    fn view_peer_stream(&self) -> Option<Element<'_, LobbyMessage>> {
        self.peer_stream
            .as_ref()
            .map(|v| v.view().map(LobbyMessage::PeerStreamMessage))
    }

    pub fn view(&self) -> Element<'_, LobbyMessage> {
        column!(
            container(row!(
                self.id.as_str(),
                button("Leave").on_press(LobbyMessage::Leave)
            )),
            row!(
                self.view_my_stream(),
                self.view_peer_stream(),
                self.view_clients(),
            ),
        )
        .into()
    }

    async fn handle_lobby_info(
        &mut self,
        info: LobbyInfoData,
        tasks: &mut Vec<Task<LobbyMessage>>,
    ) -> anyhow::Result<()> {
        let client_addresses = info
            .clients
            .iter()
            .map(|v| v.udp_addr)
            .filter(|v| v.is_some())
            .map(|v| match v {
                Some(v) => v,
                None => unreachable!(),
            })
            .collect::<Vec<_>>();
        println!("hole punching {:?}", &client_addresses);
        self.server_client.send_hello(&client_addresses).await?;

        if let Some(client) = info.clients.get(0) {
            if client.is_streaming {
                let (peer_stream, task) = PeerStream::new(
                    &client.udp_addr.unwrap(),
                    self.server_client.udp_socket.as_raw_fd(),
                );
                println!("starting peer stream");
                tasks.push(task.map(LobbyMessage::PeerStreamMessage));
                self.peer_stream = Some(peer_stream);
            }
        }

        if let Some(address) = client_addresses.get(0) {
            if let Some(my_stream) = &self.my_stream {
                my_stream.set_udp_sink(address);
            }
        }

        Ok(())
    }

    pub fn update(&mut self, message: LobbyMessage) -> Task<LobbyMessage> {
        match message {
            LobbyMessage::VideoStreamMessage(v) => self
                .my_stream
                .as_mut()
                .unwrap()
                .update(v)
                .map(LobbyMessage::VideoStreamMessage),
            LobbyMessage::PeerStreamMessage(v) => self
                .peer_stream
                .as_mut()
                .unwrap()
                .update(v)
                .map(LobbyMessage::PeerStreamMessage),
            LobbyMessage::StartStream => {
                let (my_stream, task) = MyStream::new(self.server_client.udp_socket.as_raw_fd());
                self.my_stream = Some(my_stream);
                smol::block_on(async { self.server_client.rpc.start_stream().await }).unwrap();
                task.map(LobbyMessage::VideoStreamMessage)
            }
            LobbyMessage::StopStream => {
                self.my_stream = None;
                Task::none()
            }
            LobbyMessage::RpcNotify(v) => match v {
                Ok(v) => {
                    println!("got message: {v:#?}");
                    let mut tasks = Vec::new();
                    smol::block_on(async { self.handle_lobby_info(v, &mut tasks).await }).unwrap();
                    Task::batch(tasks)
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
