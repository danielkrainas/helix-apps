//! `gdk::Paintable` showing the current video frame with the correct
//! display aspect ratio.

use crate::fit;
use crate::sink::Frame;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    pub struct VideoPaintable {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub display: Cell<(f64, f64)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VideoPaintable {
        const NAME: &'static str = "HelixVideoPaintable";
        type Type = super::VideoPaintable;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for VideoPaintable {}

    impl PaintableImpl for VideoPaintable {
        fn intrinsic_width(&self) -> i32 {
            self.display.get().0.round() as i32
        }

        fn intrinsic_height(&self) -> i32 {
            self.display.get().1.round() as i32
        }

        fn intrinsic_aspect_ratio(&self) -> f64 {
            let (w, h) = self.display.get();
            if h > 0.0 {
                w / h
            } else {
                0.0
            }
        }

        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            let Some(texture) = self.texture.borrow().clone() else {
                return;
            };
            let Some(snapshot) = snapshot.downcast_ref::<gtk::Snapshot>() else {
                return;
            };
            let (dw, dh) = self.display.get();
            let r = fit::fit_contain(dw, dh, width, height);
            let bounds =
                graphene::Rect::new(r.x as f32, r.y as f32, r.width as f32, r.height as f32);
            snapshot.append_scaled_texture(&texture, gsk::ScalingFilter::Linear, &bounds);
        }
    }
}

glib::wrapper! {
    pub struct VideoPaintable(ObjectSubclass<imp::VideoPaintable>) @implements gdk::Paintable;
}

impl Default for VideoPaintable {
    fn default() -> Self {
        glib::Object::new()
    }
}

fn memory_format(format: gst_video::VideoFormat) -> Option<gdk::MemoryFormat> {
    use gst_video::VideoFormat as V;
    Some(match format {
        V::Bgrx => gdk::MemoryFormat::B8g8r8x8,
        V::Bgra => gdk::MemoryFormat::B8g8r8a8,
        V::Rgba => gdk::MemoryFormat::R8g8b8a8,
        V::Rgbx => gdk::MemoryFormat::R8g8b8x8,
        _ => return None,
    })
}

/// Wraps the frame's buffer in a texture without copying the pixels.
pub fn texture_for(frame: &Frame) -> Option<gdk::Texture> {
    let format = memory_format(frame.format)?;
    let mapped = frame.buffer.clone().into_mapped_buffer_readable().ok()?;
    let bytes = glib::Bytes::from_owned(mapped);
    let len = frame.stride * (frame.height as usize - 1) + frame.width as usize * 4;
    let bytes = if frame.offset == 0 && bytes.len() == len {
        bytes
    } else {
        glib::Bytes::from_bytes(&bytes, frame.offset..frame.offset + len)
    };
    Some(
        gdk::MemoryTexture::new(
            frame.width as i32,
            frame.height as i32,
            format,
            &bytes,
            frame.stride,
        )
        .upcast(),
    )
}

impl VideoPaintable {
    pub fn set_frame(&self, frame: &Frame) {
        let Some(texture) = texture_for(frame) else {
            return;
        };
        let imp = self.imp();
        let display = fit::display_size(frame.width, frame.height, frame.par.0, frame.par.1);
        imp.texture.replace(Some(texture));
        if imp.display.replace(display) != display {
            self.invalidate_size();
        }
        self.invalidate_contents();
    }

    pub fn clear(&self) {
        let imp = self.imp();
        imp.texture.replace(None);
        imp.display.set((0.0, 0.0));
        self.invalidate_size();
        self.invalidate_contents();
    }

    pub fn texture(&self) -> Option<gdk::Texture> {
        self.imp().texture.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        format: gst_video::VideoFormat,
        w: u32,
        h: u32,
        stride: usize,
        offset: usize,
    ) -> Frame {
        gst::init().unwrap();
        let size = offset + stride * h as usize;
        let mut data = vec![0u8; size];
        // First pixel pure red in BGRx.
        data[offset + 2] = 255;
        Frame {
            format,
            width: w,
            height: h,
            stride,
            offset,
            par: (1, 1),
            fps: (25, 1),
            pts: None,
            buffer: gst::Buffer::from_slice(data),
        }
    }

    #[test]
    fn texture_and_screenshot_png() {
        let f = frame(gst_video::VideoFormat::Bgrx, 4, 2, 32, 8);
        let tex = texture_for(&f).expect("texture");
        assert_eq!((tex.width(), tex.height()), (4, 2));
        let dir = tempfile::tempdir().unwrap();
        let path =
            crate::screenshot::unique_path(dir.path(), "clip 0-00-01", "png", |p| p.exists());
        tex.save_to_png(&path).unwrap();
        let png = std::fs::read(&path).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        // Unsupported formats are rejected.
        assert!(texture_for(&frame(gst_video::VideoFormat::I420, 4, 2, 4, 0)).is_none());
    }
}
