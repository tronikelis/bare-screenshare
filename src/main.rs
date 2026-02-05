use std::{
    cell::RefCell,
    collections::HashMap,
    os::fd::{self, AsRawFd},
};

use gio::{
    Cancellable,
    glib::object::ObjectExt,
    prelude::{DBusProxyExt, UnixFDListExtManual},
};
use glib::variant::ToVariant;
use gstreamer::prelude::ElementExt;

mod pipewire;

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
        pipewire_fd.as_raw_fd(),
        pipewire_node_id,
    ))
    .unwrap();

    pipeline
        .set_state(gstreamer::State::Playing)
        .expect("Unable to set the pipeline to the `Playing` state");

    for message in pipeline.bus().unwrap().iter_timed_filtered(
        gstreamer::ClockTime::NONE,
        &[gstreamer::MessageType::Error, gstreamer::MessageType::Eos],
    ) {}
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

    fn open_pipewire_remote(&self, session_proxy: &SessionProxy) -> fd::OwnedFd {
        let response = self
            .proxy
            .call_with_unix_fd_list_sync(
                "OpenPipeWireRemote",
                Some(&glib::Variant::tuple_from_iter([
                    session_proxy.object_path().unwrap().to_variant(),
                    HashMap::<String, glib::Variant>::new().to_variant(),
                ])),
                gio::DBusCallFlags::empty(),
                -1,
                None::<&gio::UnixFDList>,
                None::<&Cancellable>,
            )
            .unwrap();

        response.1.unwrap().get(0).unwrap()
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
