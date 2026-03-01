use crate::{macros, video};

use futures::{channel, stream};
use iced::{Element, Task, advanced, task, widget::image};

#[derive(Debug, Clone)]
pub enum VideoStreamMessage {
    PipelineMessage(video::VideoMessage),
    FrameAllocated(Result<advanced::image::Allocation, advanced::image::Error>),
}

pub struct VideoStream {
    image_alloc_handle: Option<task::Handle>,
    image_alloc: Option<image::Allocation>,
    pipeline: video::VideoPipeline,
}

impl VideoStream {
    pub fn new() -> (Self, iced::Task<VideoStreamMessage>) {
        let (message_tx, message_rx) = channel::mpsc::channel(16);
        let pipeline = video::VideoPipeline::new(message_tx);

        let slf = Self {
            image_alloc_handle: None,
            image_alloc: None,
            pipeline,
        };

        let task = iced::Task::stream(stream::unfold(message_rx, async |mut frame_rx| {
            frame_rx.recv().await.ok().map(|v| (v, frame_rx))
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
                    let (task, abort) =
                        image::allocate(image::Handle::from_rgba(1920, 1080, bytes))
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
