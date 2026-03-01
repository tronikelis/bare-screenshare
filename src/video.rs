use gstreamer as gst;
use gstreamer_app as gst_app;

use std::{
    os::fd::{AsRawFd, OwnedFd},
    sync::{Arc, Mutex},
    thread,
};

use futures::channel;
use glib::object::ObjectExt;
use gst::prelude::{ElementExt, GstBinExtManual};

use crate::{dbus, macros};

struct SampleBytes {
    memory: gst::MappedMemory<gst::memory::Readable>,
}

impl From<gst::Sample> for SampleBytes {
    fn from(value: gst::Sample) -> Self {
        Self {
            memory: value
                .buffer()
                .unwrap()
                .memory(0)
                .unwrap()
                .into_mapped_memory_readable()
                .unwrap(),
        }
    }
}

impl From<SampleBytes> for bytes::Bytes {
    fn from(value: SampleBytes) -> Self {
        Self::from_owner(value)
    }
}

impl AsRef<[u8]> for SampleBytes {
    fn as_ref(&self) -> &[u8] {
        self.memory.as_ref()
    }
}

#[derive(Debug, Clone)]
pub enum VideoMessage {
    Frame(bytes::Bytes),
}

pub struct VideoPipeline {
    pipeline: gst::Pipeline,
    worker: Option<thread::JoinHandle<()>>,

    // only for dropping later
    _pipewirefd: OwnedFd,
    _screen_cast_proxy: dbus::ScreenCastProxy,
}

impl VideoPipeline {
    pub fn new(message_tx: channel::mpsc::Sender<VideoMessage>) -> Self {
        let bus_connection = dbus::bus_connection_get_session();
        let screen_cast_proxy = dbus::ScreenCastProxy::new(bus_connection);

        screen_cast_proxy.select_sources().unwrap();

        let pipewire_node_id = screen_cast_proxy.start().unwrap();
        let pipewire_fd = screen_cast_proxy.open_pipewire_remote();

        gst::init().unwrap();

        let pipeline = gstreamer::Pipeline::new();
        let pipewiresrc = gstreamer::ElementFactory::make("pipewiresrc")
            .property("fd", pipewire_fd.as_raw_fd())
            .property("path", pipewire_node_id.to_string())
            .build()
            .unwrap();

        let videoconvertscale = gstreamer::ElementFactory::make("videoconvertscale")
            .build()
            .unwrap();
        let appsink = gst_app::AppSink::builder()
            .drop(true)
            .caps(
                &gstreamer::Caps::builder("video/x-raw")
                    .field("format", "RGBA")
                    .build(),
            )
            .property("emit-signals", true)
            .build();

        let elements = [&pipewiresrc, &videoconvertscale, &appsink.clone().into()];
        pipeline.add_many(elements).unwrap();
        gstreamer::Element::link_many(elements).unwrap();

        let message_tx = Mutex::new(message_tx);
        appsink.connect_closure(
            "new-sample",
            true,
            glib::closure!(move |sink: &gstreamer_app::AppSink| {
                let mut message_tx = message_tx.lock().unwrap();
                let sample_bytes: SampleBytes = sink.pull_sample().unwrap().into();
                let _ = message_tx.try_send(VideoMessage::Frame(sample_bytes.into()));
                gstreamer::FlowReturn::Ok
            }),
        );

        let worker = thread::spawn(macros::clone_expr!(
            pipeline => move || {
                pipeline.set_state(gstreamer::State::Playing).unwrap();
                for message in pipeline.bus().unwrap().iter_timed_filtered(
                    gstreamer::ClockTime::NONE,
                    &[gstreamer::MessageType::Error, gstreamer::MessageType::Eos, gst::MessageType::StateChanged],
                ) {
                    match message.view() {
                        gst::MessageView::StateChanged(v) => {
                            match v.current() {
                                gst::State::Null => {
                                    break
                                }
                                _ => {}
                            }
                        }
                        gstreamer::MessageView::Eos(_) => {
                            dbg!("eos");
                            break;
                        }
                        gstreamer::MessageView::Error(v) => {
                            dbg!("err", v);
                            break;
                        }
                        _ => unreachable!(),
                    };
                }
            }
        ));

        Self {
            pipeline,
            worker: Some(worker),

            _pipewirefd: pipewire_fd,
            _screen_cast_proxy: screen_cast_proxy,
        }
    }
}

impl Drop for VideoPipeline {
    fn drop(&mut self) {
        self.pipeline.set_state(gst::State::Null).unwrap();
        if let Some(v) = self.worker.take() {
            v.join().unwrap();
        }
    }
}
