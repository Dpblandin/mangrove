//! Shared image presentation for committed files, local changes and stashes.
use crate::{
    file_browser::ReadGate,
    preferences::Preferences,
    theme::{Palette, RADIUS_LG, icon},
};
use gpui_kit::{assets::IconName, prelude::*, *};
use image::{ImageDecoder, ImageReader};
use mangrove_core::{
    Workspace,
    model::{FileImageData, FileImageSource, FileImages},
};
use std::{io::Cursor, path::Path, sync::Arc};

const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;

pub fn supports_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff"
            )
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageTarget {
    pub session_id: String,
    pub source: FileImageSource,
}

enum PreviewImage {
    Absent,
    Ready {
        image: Arc<RenderImage>,
        width: u32,
        height: u32,
        bytes: usize,
    },
    Unavailable(String),
}

struct PreparedImages {
    before: Option<PreviewImage>,
    after: PreviewImage,
}

fn decode(bytes: &[u8]) -> Result<(Arc<RenderImage>, u32, u32), String> {
    if bytes.starts_with(b"version https://git-lfs.github.com/spec/v1") {
        return Err("Objet Git LFS : l’image n’est pas disponible dans le dépôt.".into());
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "Image illisible ou format non pris en charge.")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "Image illisible ou format non pris en charge.")?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS || decoder.total_bytes() > MAX_DECODE_BYTES
    {
        return Err("Dimensions trop grandes : aperçu limité à 16 mégapixels.".into());
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| "Image illisible ou format non pris en charge.")?;
    decoded.apply_orientation(orientation);
    let mut rgba = decoded.into_rgba8();
    let (width, height) = rgba.dimensions();
    // GPUI uploads BGRA pixels, while the decoder returns RGBA.
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok((
        Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])),
        width,
        height,
    ))
}

fn prepare(data: FileImageData) -> PreviewImage {
    match data {
        FileImageData::Absent => PreviewImage::Absent,
        FileImageData::Unavailable(message) => PreviewImage::Unavailable(message),
        FileImageData::Bytes(bytes) => match decode(&bytes) {
            Ok((image, width, height)) => PreviewImage::Ready {
                image,
                width,
                height,
                bytes: bytes.len(),
            },
            Err(error) => PreviewImage::Unavailable(error),
        },
    }
}

pub struct ImagePreview {
    engine: Arc<Workspace>,
    gate: ReadGate,
    target: Option<ImageTarget>,
    revision: u64,
    generation: u64,
    pending: Option<(u64, ImageTarget)>,
    active: bool,
    task: Option<Task<()>>,
    images: Option<PreparedImages>,
    error: Option<String>,
    palette: Palette,
    font_size_offset: f32,
    compact: bool,
}

impl ImagePreview {
    pub fn new(engine: Arc<Workspace>, gate: ReadGate) -> Self {
        Self {
            engine,
            gate,
            target: None,
            revision: 0,
            generation: 0,
            pending: None,
            active: false,
            task: None,
            images: None,
            error: None,
            palette: Palette::default(),
            font_size_offset: 1.,
            compact: false,
        }
    }

    pub fn has_target(&self) -> bool {
        self.target.is_some()
    }

    pub fn set_style(&mut self, preferences: &Preferences, compact: bool, cx: &mut Context<Self>) {
        let palette = preferences.palette();
        if self.palette != palette
            || self.font_size_offset != preferences.font_size_offset
            || self.compact != compact
        {
            self.palette = palette;
            self.font_size_offset = preferences.font_size_offset;
            self.compact = compact;
            cx.notify();
        }
    }

    pub fn present(&mut self, target: Option<ImageTarget>, revision: u64, cx: &mut Context<Self>) {
        if self.target == target && self.revision == revision {
            return;
        }
        self.generation += 1;
        self.revision = revision;
        self.target = target.clone();
        self.images = None;
        self.error = None;
        self.pending = target.map(|target| (self.generation, target));
        self.pump(cx);
        cx.notify();
    }

    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.active {
            return;
        }
        let Some((generation, target)) = self.pending.take() else {
            return;
        };
        self.active = true;
        let engine = self.engine.clone();
        let gate = self.gate.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let images: Result<FileImages, String> = {
                        let _guard = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        engine.file_images(&target.session_id, &target.source)
                    };
                    images.map(|images| PreparedImages {
                        before: images.before.map(prepare),
                        after: prepare(images.after),
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.active = false;
                if view.generation == generation {
                    match result {
                        Ok(images) => view.images = Some(images),
                        Err(error) => view.error = Some(error),
                    }
                    cx.notify();
                }
                view.pump(cx);
            });
        }));
    }

    fn panel(&self, title: &'static str, image: &PreviewImage, color: u32) -> AnyElement {
        let p = self.palette;
        let mut body = div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .bg(rgb(p.bg));
        let mut header = div()
            .flex_none()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.))
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(rgb(p.border))
            .text_size(px(11. + self.font_size_offset))
            .text_color(rgb(color))
            .child(icon(IconName::Image, 14., color))
            .child(title);
        match image {
            PreviewImage::Ready {
                image,
                width,
                height,
                bytes,
            } => {
                header = header
                    .child(div().flex_1())
                    .child(div().text_color(rgb(p.muted)).child(format!(
                        "{width} × {height} · {:.0} Kio",
                        *bytes as f64 / 1024.
                    )));
                body = body.child(
                    img(image.clone())
                        .size_full()
                        .object_fit(ObjectFit::ScaleDown),
                );
            }
            PreviewImage::Absent => {
                body = body.child(self.message("Fichier absent à cette révision."))
            }
            PreviewImage::Unavailable(message) => body = body.child(self.message(message)),
        }
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .border_1()
            .border_color(rgb(p.border))
            .rounded(RADIUS_LG)
            .overflow_hidden()
            .bg(rgb(p.panel))
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn message(&self, message: &str) -> AnyElement {
        div()
            .p(px(24.))
            .text_center()
            .text_size(px(12. + self.font_size_offset))
            .text_color(rgb(self.palette.muted))
            .whitespace_normal()
            .child(message.to_owned())
            .into_any_element()
    }
}

impl Render for ImagePreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let mut root = div()
            .id("image-preview")
            .role(accesskit::Role::Group)
            .aria_label("Aperçu des images")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .gap(px(16.))
            .p(px(if self.compact { 12. } else { 20. }))
            .bg(rgb(p.reading_bg))
            .when(self.compact, |view| view.flex_col());
        if let Some(images) = &self.images {
            match (&images.before, &images.after) {
                (None, after) => root = root.child(self.panel("Image", after, p.text)),
                (Some(PreviewImage::Absent), after) => {
                    root = root.child(self.panel("Après · Image ajoutée", after, p.accent))
                }
                (Some(before), PreviewImage::Absent) => {
                    root = root.child(self.panel("Avant · Image supprimée", before, p.danger))
                }
                (Some(before), after) => {
                    root = root
                        .child(self.panel("Avant", before, p.danger))
                        .child(self.panel("Après", after, p.accent))
                }
            }
        } else {
            root = root
                .items_center()
                .justify_center()
                .child(self.message(self.error.as_deref().unwrap_or("Chargement de l’image…")));
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::decode;
    use std::io::Cursor;

    #[test]
    fn png_preserves_dimensions_transparency_and_gpui_channel_order() {
        let pixels = image::RgbaImage::from_pixel(2, 3, image::Rgba([10, 20, 30, 40]));
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let (image, width, height) = decode(encoded.get_ref()).unwrap();
        assert_eq!((width, height), (2, 3));
        assert_eq!(&image.as_bytes(0).unwrap()[..4], &[30, 20, 10, 40]);
    }

    #[test]
    fn animated_images_show_only_the_first_frame() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                        1,
                        1,
                        image::Rgba(color),
                    )))
                    .unwrap();
            }
        }
        let (image, _, _) = decode(&bytes).unwrap();
        assert_eq!(image.frame_count(), 1);
        assert_eq!(image.as_bytes(0).unwrap(), &[0, 0, 255, 255]);
    }

    #[test]
    fn corrupt_lfs_and_oversized_images_have_explicit_fallbacks() {
        assert!(decode(b"not an image").is_err());
        assert!(
            decode(b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 42\n")
                .unwrap_err()
                .contains("LFS")
        );
        // A BMP header advertises 25 megapixels without allocating that buffer.
        let mut bmp = vec![0u8; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&5000u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&5000u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        assert!(decode(&bmp).is_err());
    }
}
