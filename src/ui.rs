use futures::{channel, stream};
use gstreamer as gst;
use iced::{Element, Task, advanced, task, widget::image};

use crate::{macros, video};

#[derive(Debug, Clone)]
pub enum VideoStreamMessage {
    PipelineMessage(video::VideoMessage),
    FrameAllocated(Result<advanced::image::Allocation, advanced::image::Error>),
}

pub struct VideoStream {
    image_alloc_handle: Option<task::Handle>,
    image_alloc: Option<image::Allocation>,
    pipeline: video::VideoPipeline,
    caps: Option<gst::Caps>,
}

impl VideoStream {
    pub fn new() -> (Self, iced::Task<VideoStreamMessage>) {
        let (message_tx, message_rx) = channel::mpsc::channel(16);
        let pipeline = video::VideoPipeline::new(message_tx);

        let slf = Self {
            image_alloc_handle: None,
            image_alloc: None,
            caps: None,
            pipeline,
        };

        let task = iced::Task::stream(stream::unfold(message_rx, async |mut message_rx| {
            message_rx.recv().await.ok().map(|v| (v, message_rx))
        }))
        .map(|v| VideoStreamMessage::PipelineMessage(v));

        (slf, task)
    }

    pub fn view<T>(&self) -> Element<'_, T> {
        self.image_alloc.clone().map(|v| image(v.handle())).into()
    }

    pub fn update(&mut self, message: VideoStreamMessage) -> iced::Task<VideoStreamMessage> {
        match message {
            VideoStreamMessage::PipelineMessage(message) => match message {
                video::VideoMessage::Frame(bytes) => {
                    if let Some(caps) = self.caps.as_ref() {
                        let structure = caps.structure(0).unwrap();
                        let width: i32 = structure.get("width").unwrap();
                        let height: i32 = structure.get("height").unwrap();

                        let (task, abort) = image::allocate(image::Handle::from_rgba(
                            width as _,
                            height as _,
                            bytes,
                        ))
                        .map(|v| VideoStreamMessage::FrameAllocated(v))
                        .abortable();
                        self.image_alloc_handle = Some(abort.abort_on_drop());

                        task
                    } else {
                        Task::none()
                    }
                }
                video::VideoMessage::Caps(caps) => {
                    self.caps = Some(caps);
                    Task::none()
                }
            },
            VideoStreamMessage::FrameAllocated(allocation) => {
                self.image_alloc = Some(allocation.unwrap());
                Task::none()
            }
        }
    }
}
