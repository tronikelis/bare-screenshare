use std::{
    os::fd::{AsRawFd, OwnedFd},
    str::FromStr,
    sync::Mutex,
    thread,
};

use futures::{SinkExt, channel, executor};
use glib::object::ObjectExt;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;

use crate::{dbus, macros, pipeline};

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
    Caps(gst::Caps),
}

pub struct VideoPipeline {
    pipeline: gst::Pipeline,
    worker: Option<thread::JoinHandle<()>>,

    // only for dropping later
    _pipewirefd: OwnedFd,
    _screen_cast_proxy: dbus::ScreenCastProxy,
}

impl VideoPipeline {
    pub fn new(mut message_tx: channel::mpsc::Sender<VideoMessage>) -> Self {
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

        let vp9enc = gst::ElementFactory::make("vp9enc")
            .property("deadline", 1i64)
            .property("cpu-used", 16)
            .build()
            .unwrap();
        let vp9payloader = gst::ElementFactory::make("rtpvp9pay").build().unwrap();

        let appsink = gst_app::AppSink::builder()
            .sync(false)
            .drop(true)
            .caps(&gst::Caps::from_str("video/x-raw,format=RGBA").unwrap())
            .property("emit-signals", true)
            .build();

        let udpsink = gst::ElementFactory::make("udpsink")
            .property("sync", false)
            .property("port", 3000)
            .build()
            .unwrap();

        let pipeline = gst::Pipeline::new();
        pipeline::Pipeline::new(&pipeline)
            .link([&pipewiresrc, &tee])
            .link([&tee, &queue1, &videoconvertscale1, &appsink.clone().into()])
            .link([
                &tee,
                &queue2,
                &videoconvertscale2,
                &vp9enc,
                &vp9payloader,
                &udpsink,
            ]);

        let message_tx_mu = Mutex::new(message_tx.clone());
        appsink.connect_closure(
            "new-sample",
            true,
            glib::closure!(move |sink: &gst_app::AppSink| {
                let mut message_tx = message_tx_mu.lock().unwrap();
                let sample_bytes: SampleBytes = sink.pull_sample().unwrap().into();
                let _ = message_tx.try_send(VideoMessage::Frame(sample_bytes.into()));
                gst::FlowReturn::Ok
            }),
        );

        let worker = thread::spawn(macros::clone_expr!(
            pipeline => move || {
                pipeline.set_state(gst::State::Playing).unwrap();
                for message in pipeline.bus().unwrap().iter_timed_filtered(
                    gst::ClockTime::NONE,
                    &[gst::MessageType::Error, gst::MessageType::Eos, gst::MessageType::StateChanged],
                ) {
                    match message.view() {
                        gst::MessageView::StateChanged(v) => {
                            match v.current() {
                                gst::State::Null => {
                                    break
                                }
                                gst::State::Playing => {
                                    if let Some(caps) = appsink.pads()[0].current_caps() {
                                        executor::block_on(async {
                                            let _ = message_tx.send(VideoMessage::Caps(
                                                caps
                                            )).await;
                                        })
                                    }
                                }
                                _ => {}
                            }
                        }
                        gst::MessageView::Eos(_) => {
                            dbg!("eos");
                            break;
                        }
                        gst::MessageView::Error(v) => {
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
