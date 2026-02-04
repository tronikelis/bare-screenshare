use gstreamer::prelude::GstBinExtManual;
use gstreamer::prelude::{ElementExt, ElementExtManual};
use std::cell::RefCell;
use std::io::Stdin;
use std::os::fd::AsRawFd;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::{collections::HashMap, time};

use gio::{Cancellable, glib::object::ObjectExt, prelude::DBusProxyExt};
use glib::variant::ToVariant;

mod pipewire;

// #[zbus::proxy(
//     interface = "org.freedesktop.portal.Request",
//     default_service = "org.freedesktop.portal.Desktop"
// )]
// pub trait Request {
//     #[zbus(signal)]
//     fn response(
//         &self,
//         response: u32,
//         results: HashMap<String, zbus::zvariant::Value<'_>>,
//     ) -> Result<()>;
// }
//
// #[zbus::proxy(
//     interface = "org.freedesktop.portal.Session",
//     default_service = "org.freedesktop.portal.Desktop"
// )]
// pub trait Session {}
//
// #[zbus::proxy(
//     interface = "org.freedesktop.portal.ScreenCast",
//     default_service = "org.freedesktop.portal.Desktop",
//     default_path = "/org/freedesktop/portal/desktop",
//     assume_defaults = true
// )]
// pub trait ScreenCast {
//     /// CreateSession method
//     fn create_session(
//         &self,
//         options: std::collections::HashMap<&str, &zbus::zvariant::Value<'_>>,
//     ) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
//
//     /// OpenPipeWireRemote method
//     fn open_pipe_wire_remote(
//         &self,
//         session_handle: &zbus::zvariant::ObjectPath<'_>,
//         options: std::collections::HashMap<&str, &zbus::zvariant::Value<'_>>,
//     ) -> zbus::Result<zbus::zvariant::OwnedFd>;
//
//     /// SelectSources method
//     fn select_sources(
//         &self,
//         session_handle: &zbus::zvariant::ObjectPath<'_>,
//         options: std::collections::HashMap<&str, &zbus::zvariant::Value<'_>>,
//     ) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
//
//     /// Start method
//     fn start(
//         &self,
//         session_handle: &zbus::zvariant::ObjectPath<'_>,
//         parent_window: &str,
//         options: std::collections::HashMap<&str, &zbus::zvariant::Value<'_>>,
//     ) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
//
//     /// AvailableCursorModes property
//     #[zbus(property)]
//     fn available_cursor_modes(&self) -> zbus::Result<u32>;
//
//     /// AvailableSourceTypes property
//     #[zbus(property)]
//     fn available_source_types(&self) -> zbus::Result<u32>;
//
//     /// version property
//     #[zbus(property, name = "version")]
//     fn version(&self) -> zbus::Result<u32>;
// }
//
// fn get_connection_bus_unique_name(bus: &zbus::Connection) -> String {
//     bus.unique_name()
//         .unwrap()
//         .trim_start_matches(':')
//         .replace(".", "_")
// }
//
// fn make_handle_token() -> String {
//     let mut ten: [u8; 20] = [0; 20];
//     rand::fill(&mut ten);
//     ten.iter()
//         .map(|v| format!("{:x}", v))
//         .collect::<Vec<_>>()
//         .join("")
// }
//
// struct XdgSession<'a> {
//     proxy: SessionProxy<'a>,
// }
//
// impl<'a> XdgSession<'a> {
//     async fn new(bus: &'a zbus::Connection) -> (Self, String) {
//         let handle_token = make_handle_token();
//
//         let proxy = SessionProxy::new(
//             bus,
//             format!(
//                 "/org/freedesktop/portal/desktop/session/{}/{}",
//                 get_connection_bus_unique_name(bus),
//                 &handle_token,
//             ),
//         )
//         .await
//         .unwrap();
//
//         (Self { proxy }, handle_token)
//     }
//
//     fn assert_path(&self, path: &str) {
//         assert_eq!(self.path(), path);
//     }
//
//     fn path(&self) -> &zbus::zvariant::ObjectPath<'_> {
//         self.proxy.0.path()
//     }
// }
//
// struct XdgRequest<'a> {
//     proxy: RequestProxy<'a>,
//     response_stream: ResponseStream,
// }
//
// impl<'a> XdgRequest<'a> {
//     async fn new(bus: &'a zbus::Connection) -> (Self, String) {
//         let handle_token = make_handle_token();
//
//         let proxy = RequestProxy::new(
//             bus,
//             format!(
//                 "/org/freedesktop/portal/desktop/request/{}/{}",
//                 get_connection_bus_unique_name(bus),
//                 &handle_token,
//             ),
//         )
//         .await
//         .unwrap();
//
//         let response_stream = proxy.receive_response().await.unwrap();
//
//         (
//             Self {
//                 response_stream,
//                 proxy,
//             },
//             handle_token,
//         )
//     }
//
//     async fn wait(&mut self) -> Response {
//         self.response_stream.next().await.unwrap()
//     }
//
//     fn assert_path(&self, path: &str) {
//         assert_eq!(self.path(), path);
//     }
//
//     fn path(&self) -> &zbus::zvariant::ObjectPath<'_> {
//         self.proxy.0.path()
//     }
// }
//
// struct XdgScreenCast<'a> {
//     bus: &'a zbus::Connection,
//     screen_cast_proxy: ScreenCastProxy<'a>,
// }
//
// impl<'a> XdgScreenCast<'a> {
//     async fn new(bus: &'a zbus::Connection) -> Self {
//         Self {
//             bus,
//             screen_cast_proxy: ScreenCastProxy::new(bus).await.unwrap(),
//         }
//     }
//
//     async fn create_session(&self) -> XdgSession<'_> {
//         let (mut request, handle_token) = XdgRequest::new(&self.bus).await;
//         let (session, session_handle_token) = XdgSession::new(&self.bus).await;
//
//         let handle_token_variant = zbus::zvariant::Value::new(handle_token.clone());
//         let session_handle_token_variant = zbus::zvariant::Value::new(session_handle_token.clone());
//
//         let (response, request_path) = futures::join!(
//             request.wait(),
//             self.screen_cast_proxy.create_session(HashMap::from([
//                 ("handle_token", &handle_token_variant),
//                 ("session_handle_token", &session_handle_token_variant),
//             ]))
//         );
//         request.assert_path(request_path.unwrap().as_str());
//         session.assert_path(
//             match response
//                 .args()
//                 .unwrap()
//                 .results()
//                 .get("session_handle")
//                 .unwrap()
//             {
//                 zbus::zvariant::Value::Str(v) => v.as_str(),
//                 _ => unreachable!(),
//             },
//         );
//
//         session
//     }
//
//     async fn select_sources(&self, session: &XdgSession<'_>) {
//         let (mut request, handle_token) = XdgRequest::new(&self.bus).await;
//
//         let handle_token_variant = zbus::zvariant::Value::new(handle_token.clone());
//         let (_, request_path) = futures::join!(
//             request.wait(),
//             self.screen_cast_proxy.select_sources(
//                 session.path(),
//                 HashMap::from([("handle_token", &handle_token_variant)]),
//             )
//         );
//         request.assert_path(request_path.unwrap().as_str());
//     }
//
//     async fn start(&self, session: &XdgSession<'_>) -> u32 {
//         let (mut request, handle_token) = XdgRequest::new(&self.bus).await;
//
//         let handle_token_variant = zbus::zvariant::Value::new(handle_token.clone());
//         let (response, request_path) = futures::join!(
//             request.wait(),
//             self.screen_cast_proxy.start(
//                 session.path(),
//                 "parent-window?",
//                 HashMap::from([("handle_token", &handle_token_variant)]),
//             )
//         );
//         request.assert_path(request_path.unwrap().as_str());
//
//         match response.args().unwrap().results.get("streams").unwrap() {
//             zbus::zvariant::Value::Array(array) => match array.first().unwrap() {
//                 zbus::zvariant::Value::Structure(structure) => {
//                     match structure.fields().first().unwrap() {
//                         zbus::zvariant::Value::U32(v) => *v,
//                         _ => unreachable!(),
//                     }
//                 }
//                 _ => unreachable!(),
//             },
//             _ => unreachable!(),
//         }
//     }
//
//     async fn open_pipe_wire_remote(
//         &self,
//         session: &XdgSession<'_>,
//         options: HashMap<&str, &zbus::zvariant::Value<'_>>,
//     ) -> zbus::zvariant::OwnedFd {
//         self.screen_cast_proxy
//             .open_pipe_wire_remote(session.path(), options)
//             .await
//             .unwrap()
//     }
// }

fn main() {
    let bus_connection =
        gio::functions::bus_get_sync(gio::BusType::Session, None::<&Cancellable>).unwrap();

    let screen_cast_proxy = ScreenCastProxy::new(&bus_connection);

    let session_proxy = screen_cast_proxy.create_session().unwrap();

    screen_cast_proxy.select_sources(&session_proxy).unwrap();
    let pipewire_node_id = screen_cast_proxy.start(&session_proxy).unwrap();
    let pipewire_fd = screen_cast_proxy.open_pipewire_remote(&session_proxy);

    gstreamer::init().unwrap();

    let pipeline = gstreamer::parse::launch(&format!(
        "pipewiresrc fd={} path={} do-timestamp=true ! videoconvertscale ! waylandsink sync=false enable-last-sample=false",
        pipewire_fd,
        pipewire_node_id,
    ))
    .unwrap();

    pipeline
        .set_state(gstreamer::State::Playing)
        .expect("Unable to set the pipeline to the `Playing` state");

    for message in pipeline.bus().unwrap().iter_timed_filtered(
        gstreamer::ClockTime::NONE,
        &[gstreamer::MessageType::Error, gstreamer::MessageType::Eos],
    ) {
        // use gstreamer::MessageView;
        // match message.view() {
        //     MessageView::Eos(..) => break,
        //     MessageView::Error(err) => {
        //         println!(
        //             "Error from {:?}: {} ({:?})",
        //             err.src().map(|s| s.to_string()),
        //             err.error(),
        //             err.debug()
        //         );
        //         break;
        //     }
        //     _ => {}
        // }
    }
}

fn call_request_proxy_signal(
    dbus: &gio::DBusConnection,
    call: impl FnOnce(&str),
) -> Option<glib::Variant> {
    let context = glib::MainContext::new();
    let response = RefCell::new(None);

    context
        .with_thread_default(|| {
            let main_loop = glib::MainLoop::new(Some(&context), false);

            let handle_token = make_dbus_handle_token();
            let unique_name = DBusConnection::from(dbus.clone())
                .xdg_unique_name()
                .unwrap();

            let proxy = gio::DBusProxy::new_sync(
                dbus,
                gio::DBusProxyFlags::empty(),
                None,
                Some("org.freedesktop.portal.Desktop"),
                &format!(
                    "/org/freedesktop/portal/desktop/request/{}/{}",
                    unique_name, handle_token
                ),
                "org.freedesktop.portal.Request",
                None::<&Cancellable>,
            )
            .unwrap();

            // safe, we're not creating new threads here
            unsafe {
                proxy.connect_unsafe("g-signal::Response", true, {
                    |value: &[glib::Value]| {
                        *response.borrow_mut() = Some(value.to_vec());
                        main_loop.quit();
                        None
                    }
                });
            }

            call(&handle_token);
            main_loop.run();
        })
        .unwrap();

    let response = response.borrow_mut().take()?;
    let results: Result<glib::variant::Variant, _> = response[3].get();
    Some(results.unwrap())
}

fn make_dbus_handle_token() -> String {
    let mut ten: [u8; 20] = [0; 20];
    rand::fill(&mut ten);
    ten.iter()
        .map(|v| format!("{:x}", v))
        .collect::<Vec<_>>()
        .join("")
}

struct SessionProxy {
    proxy: gio::DBusProxy,
}

impl SessionProxy {
    fn new(dbus: &gio::DBusConnection, object_path: String) -> Self {
        let proxy = gio::DBusProxy::new_sync(
            dbus,
            gio::DBusProxyFlags::empty(),
            None,
            Some("org.freedesktop.portal.Desktop"),
            &object_path,
            "org.freedesktop.portal.Session",
            None::<&Cancellable>,
        )
        .unwrap();

        Self { proxy }
    }

    fn object_path(&self) -> Result<glib::variant::ObjectPath, glib::BoolError> {
        self.proxy.object_path().to_string().try_into()
    }
}

struct ScreenCastProxy<'a> {
    proxy: gio::DBusProxy,
    bus: &'a gio::DBusConnection,
}

struct DBusConnection(pub gio::DBusConnection);

impl DBusConnection {
    fn xdg_unique_name(&self) -> Option<String> {
        Some(
            self.0
                .unique_name()?
                .to_string()
                .trim_start_matches(':')
                .replace(".", "_"),
        )
    }
}

impl From<gio::DBusConnection> for DBusConnection {
    fn from(value: gio::DBusConnection) -> Self {
        Self(value)
    }
}

impl<'a> ScreenCastProxy<'a> {
    fn new(bus: &'a gio::DBusConnection) -> Self {
        let proxy = gio::DBusProxy::new_sync(
            bus,
            gio::DBusProxyFlags::empty(),
            None,
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.ScreenCast",
            None::<&Cancellable>,
        )
        .unwrap();

        Self { proxy, bus }
    }

    fn create_session(&self) -> Result<SessionProxy, ()> {
        let response = call_request_proxy_signal(self.bus, |handle_token| {
            self.proxy
                .call_sync(
                    "CreateSession",
                    Some(&glib::Variant::tuple_from_iter([HashMap::from([
                        ("handle_token", handle_token.to_variant()),
                        ("session_handle_token", make_dbus_handle_token().into()),
                    ])
                    .to_variant()])),
                    gio::DBusCallFlags::empty(),
                    -1,
                    None::<&Cancellable>,
                )
                .unwrap();
        })
        .unwrap();

        let response: Option<(u32, HashMap<String, glib::Variant>)> = response.get();
        let response = response.unwrap();

        let session = SessionProxy::new(&self.bus, {
            let v: &glib::variant::Variant = response.1.get("session_handle").unwrap();
            v.get().unwrap()
        });

        if response.0 != 0 {
            Err(())
        } else {
            Ok(session)
        }
    }

    fn open_pipewire_remote(&self, session_proxy: &SessionProxy) -> i32 {
        let response = self
            .proxy
            .call_sync(
                "OpenPipeWireRemote",
                Some(&glib::Variant::tuple_from_iter([
                    session_proxy.object_path().unwrap().to_variant(),
                    HashMap::<String, glib::Variant>::new().to_variant(),
                ])),
                gio::DBusCallFlags::empty(),
                -1,
                None::<&Cancellable>,
            )
            .unwrap();

        let response: Option<(glib::variant::Handle,)> = response.get();
        let response = response.unwrap();
        response.0.0
    }

    fn select_sources(&self, session_proxy: &SessionProxy) -> Result<(), ()> {
        let response = call_request_proxy_signal(self.bus, |handle_token| {
            self.proxy
                .call_sync(
                    "SelectSources",
                    Some(&glib::Variant::tuple_from_iter([
                        session_proxy.object_path().unwrap().to_variant(),
                        HashMap::from([("handle_token", handle_token.to_variant())]).to_variant(),
                    ])),
                    gio::DBusCallFlags::empty(),
                    -1,
                    None::<&Cancellable>,
                )
                .unwrap();
        })
        .unwrap();
        let response: Option<(u32, HashMap<String, glib::Variant>)> = response.get();
        let response = response.unwrap();
        if response.0 != 0 { Err(()) } else { Ok(()) }
    }

    fn start(&self, session_proxy: &SessionProxy) -> Result<u32, ()> {
        let response = call_request_proxy_signal(self.bus, |handle_token| {
            self.proxy
                .call_sync(
                    "Start",
                    Some(&glib::Variant::tuple_from_iter([
                        session_proxy.object_path().unwrap().to_variant(),
                        "parent-window?".to_variant(),
                        HashMap::from([("handle_token", handle_token.to_variant())]).to_variant(),
                    ])),
                    gio::DBusCallFlags::empty(),
                    -1,
                    None::<&Cancellable>,
                )
                .unwrap();
        })
        .unwrap();
        let response: Option<(u32, HashMap<String, glib::Variant>)> = response.get();
        let response = response.unwrap();
        if response.0 != 0 {
            Err(())
        } else {
            let arrs: Option<Vec<(u32, HashMap<String, glib::Variant>)>> =
                response.1.get("streams").unwrap().get();
            let arrs = arrs.unwrap();
            Ok(arrs[0].0)
        }
    }
}
