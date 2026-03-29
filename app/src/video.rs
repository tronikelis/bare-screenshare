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
    Frame(bytes::Bytes, gst::Caps),
    // todo: add error here
}

pub struct VideoPipeline {
    pipeline: gst::Pipeline,
    worker: Option<thread::JoinHandle<()>>,
}

impl VideoPipeline {
    pub fn new(
        message_tx: channel::mpsc::Sender<VideoMessage>,
        pipeline: gst::Pipeline,
        appsink: gst_app::AppSink,
    ) -> Self {
        let message_tx_mu = Mutex::new(message_tx.clone());
        appsink.connect_closure(
            "new-sample",
            true,
            glib::closure!(move |sink: &gst_app::AppSink| {
                let mut message_tx = message_tx_mu.lock().unwrap();
                let sample_bytes: SampleBytes = sink.pull_sample().unwrap().into();
                let caps = sink.pads()[0].current_caps().unwrap();
                let _ = message_tx.try_send(VideoMessage::Frame(sample_bytes.into(), caps));
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
                                gst::State::Playing => {}
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
