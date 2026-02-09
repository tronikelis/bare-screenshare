use std::{
    ffi::CStr,
    os::fd::{self, AsRawFd},
    ptr,
    sync::{self, Arc, Mutex, mpsc},
    thread, time,
};

use glib::object::ObjectExt;
use gstreamer::prelude::{ElementExt, GstBinExtManual};

mod dbus;
mod pipewire;

fn main() {
    let bus_connection = dbus::bus_connection_get_session();

    let screen_cast_proxy = dbus::ScreenCastProxy::new(&bus_connection);

    let session_proxy = screen_cast_proxy.create_session().unwrap();

    screen_cast_proxy.select_sources(&session_proxy).unwrap();
    let pipewire_node_id = screen_cast_proxy.start(&session_proxy).unwrap();
    let pipewire_fd = screen_cast_proxy.open_pipewire_remote(&session_proxy);

    gstreamer::init().unwrap();

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
        .property("emit-signals", true)
        .build();
    let appsinkfilter = gstreamer::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gstreamer::Caps::builder("video/x-raw")
                .field("format", "RGBA")
                .build(),
        )
        .build()
        .unwrap();

    let last_sample = Arc::new(Mutex::<Option<gstreamer::Sample>>::new(None));

    appsink.connect_closure("new-sample", true, {
        let last_sample = last_sample.clone();
        glib::closure!(|sink: &gstreamer_app::AppSink| {
            let sample = sink.pull_sample().unwrap();
            *last_sample.lock().unwrap() = Some(sample);
            // dbg!(&sample);
            // let buffer = sample.buffer().unwrap();
            // let memory = buffer.memory(0).unwrap();
            // let mapped = memory.into_mapped_memory_readable().unwrap();
            // let slic = mapped.as_slice();
            // // let r = slic[0];
            // // let g = slic[1];
            // // let b = slic[2];
            // // let a = slic[3];
            // //
            // // dbg!(r, g, b, a);

            gstreamer::FlowReturn::Ok
        })
    });

    pipeline
        .add_many([
            &pipewiresrc,
            &videoconvertscale,
            &appsink.clone().into(),
            &appsinkfilter,
        ])
        .unwrap();

    gstreamer::Element::link_many([
        &pipewiresrc,
        &videoconvertscale,
        &appsinkfilter,
        &appsink.clone().into(),
    ])
    .unwrap();

    let gpipeline = thread::spawn({
        let pipeline = pipeline.clone();
        move || {
            pipeline.set_state(gstreamer::State::Playing).unwrap();

            for message in pipeline.bus().unwrap().iter_timed_filtered(
                gstreamer::ClockTime::NONE,
                &[gstreamer::MessageType::Error, gstreamer::MessageType::Eos],
            ) {
                match message.view() {
                    gstreamer::MessageView::Eos(_) => {}
                    gstreamer::MessageView::Error(v) => {
                        dbg!("error", v);
                    }
                    _ => unreachable!(),
                };
                return;
            }
        }
    });

    let width = 1920;
    let height = 1080;

    let rcontext = raylib::RContext::new();
    rcontext
        .init_window(width, height, "sup")
        .set_target_fps(60)
        .set_config_flags(raylib::ConfigFlags::WindowResizable as _);

    while !rcontext.window_should_close() {
        rcontext.begin_drawing();

        let sample = last_sample.lock().unwrap().clone().unwrap();
        let memory = sample.buffer().unwrap().memory(0).unwrap();
        let memory_readable = memory.into_mapped_memory_readable().unwrap();
        let as_slice = memory_readable.as_slice();

        let image = unsafe {
            raylib::Image::new(
                width,
                height,
                raylib::PixelFormat::UncompressedR8G8B8A8,
                1,
                as_slice.as_ptr() as _,
                false,
            )
        };

        let texture = image.load_texture();
        texture.draw(0, 0, raylib::Color::max());

        rcontext.end_drawing();
    }

    gpipeline.join().unwrap();
}
