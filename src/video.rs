use gstreamer as gst;
use gstreamer_app as gst_app;

use std::{
    cell::{RefCell, UnsafeCell},
    os::fd::{AsRawFd, OwnedFd},
    sync::{Arc, Mutex},
    thread,
};

use futures::channel;
use glib::object::ObjectExt;
use gst::prelude::{ElementExt, GstBinExtManual};

use crate::{dbus, macros};

pub type Frame = Arc<[u8]>;

pub struct VideoPipeline {
    pipeline: gst::Pipeline,
    appsink: gst_app::AppSink,
    worker: Option<thread::JoinHandle<()>>,

    // only for dropping later
    _pipewirefd: OwnedFd,
    _screen_cast_proxy: dbus::ScreenCastProxy,
}

fn sample_to_bytes(sample: &gst::Sample) -> Vec<u8> {
    let memory = sample.buffer().unwrap().memory(0).unwrap();
    let memory_readable = memory.map_readable().unwrap();
    memory_readable.as_slice().to_vec()
}

impl VideoPipeline {
    pub fn new(frame_tx: channel::mpsc::Sender<Frame>) -> Self {
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
        let appsink = gstreamer_app::AppSink::builder()
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

        let frame_tx = Mutex::new(frame_tx);
        appsink.connect_closure(
            "new-sample",
            true,
            glib::closure!(move |sink: &gstreamer_app::AppSink| {
                let _ = frame_tx
                    .lock()
                    .unwrap()
                    .try_send(Arc::from(sample_to_bytes(&sink.pull_sample().unwrap())));
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
            appsink,
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
