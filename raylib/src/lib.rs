use std::{
    ffi::{self, c_void},
    marker::PhantomData,
    rc::Rc,
    str::FromStr,
};

#[repr(u32)]
pub enum ConfigFlags {
    WindowResizable = raylib_sys::ConfigFlags_FLAG_WINDOW_RESIZABLE,
}

pub struct RContext(PhantomData<Rc<()>>);

impl RContext {
    pub fn new() -> Self {
        Self(PhantomData)
    }

    pub fn init_window(&self, width: i32, height: i32, title: &str) -> &Self {
        unsafe {
            raylib_sys::InitWindow(
                width,
                height,
                ffi::CString::from_str(title).unwrap().as_ptr(),
            );
        }
        self
    }

    pub fn window_should_close(&self) -> bool {
        unsafe { raylib_sys::WindowShouldClose() }
    }

    pub fn set_target_fps(&self, fps: i32) -> &Self {
        unsafe { raylib_sys::SetTargetFPS(fps) }
        self
    }

    pub fn set_config_flags(&self, flags: u32) -> &Self {
        unsafe { raylib_sys::SetConfigFlags(flags) }
        self
    }

    pub fn begin_drawing(&self) {
        unsafe { raylib_sys::BeginDrawing() }
    }

    pub fn end_drawing(&self) {
        unsafe { raylib_sys::EndDrawing() }
    }
}

#[repr(u32)]
pub enum PixelFormat {
    UncompressedR8G8B8A8 = raylib_sys::PixelFormat_PIXELFORMAT_UNCOMPRESSED_R8G8B8A8,
}

pub struct Image {
    inner: raylib_sys::Image,
    drop: bool,
}

impl Image {
    pub unsafe fn new(
        width: i32,
        height: i32,
        format: PixelFormat,
        mipmaps: i32,
        data: *mut c_void,
        drop: bool,
    ) -> Self {
        Self {
            inner: raylib_sys::Image {
                width,
                height,
                format: format as _,
                mipmaps,
                data: data as _,
            },
            drop,
        }
    }

    pub fn load_texture(&self) -> Texture {
        Texture::new(unsafe { raylib_sys::LoadTextureFromImage(self.inner) })
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        if self.drop {
            unsafe { raylib_sys::UnloadImage(self.inner) }
        }
    }
}

pub struct Texture {
    inner: raylib_sys::Texture,
}

impl Texture {
    pub fn new(texture: raylib_sys::Texture) -> Self {
        Self { inner: texture }
    }

    pub fn draw(&self, posx: i32, posy: i32, tint: Color) {
        unsafe { raylib_sys::DrawTexture(self.inner, posx, posy, tint.inner) }
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        unsafe { raylib_sys::UnloadTexture(self.inner) }
    }
}

pub struct Color {
    inner: raylib_sys::Color,
}

impl Color {
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            inner: raylib_sys::Color { r, g, b, a },
        }
    }

    pub fn zero() -> Self {
        Self::new(0, 0, 0, 0)
    }

    pub fn max() -> Self {
        Self::new(u8::MAX, u8::MAX, u8::MAX, u8::MAX)
    }
}
