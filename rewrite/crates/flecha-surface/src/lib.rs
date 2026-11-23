use std::sync::Arc;

use anyhow::{Context as _, Result, ensure};
use cef::AcceleratedPaintInfo;
use gpui::{Bounds, Canvas, ContentMask, Pixels, RenderImage, Styled as _, canvas};

/// A frame whose texture and renderer provenance are assigned together.
///
/// ```compile_fail
/// fn inspect(frame: flecha_surface::Frame) {
///     let flecha_surface::Frame(..) = frame;
/// }
/// ```
#[derive(Clone)]
pub struct Frame(FrameStorage);

#[derive(Clone)]
enum FrameStorage {
    #[cfg(target_os = "linux")]
    Texture {
        view: wgpu::TextureView,
        device: wgpu::Device,
        source_uv: [f32; 4],
        software_texture: Option<wgpu::Texture>,
    },
    Image(Arc<RenderImage>),
}

#[cfg(target_os = "linux")]
fn upload_regions(dirty: &[cef::Rect], width: u32, height: u32) -> Vec<(u32, u32, u32, u32)> {
    let regions = dirty
        .iter()
        .map(|rect| {
            let x = u32::try_from(rect.x).ok()?;
            let y = u32::try_from(rect.y).ok()?;
            let w = u32::try_from(rect.width).ok()?;
            let h = u32::try_from(rect.height).ok()?;
            (x <= width && y <= height && w <= width - x && h <= height - y).then_some((x, y, w, h))
        })
        .collect::<Option<Vec<_>>>();
    regions
        .filter(|regions| regions.iter().any(|&(_, _, w, h)| w != 0 && h != 0))
        .unwrap_or_else(|| vec![(0, 0, width, height)])
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::upload_regions;

    #[test]
    fn invalid_dirty_rect_uploads_the_entire_available_frame() {
        let valid = cef::Rect {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        };
        let invalid = cef::Rect {
            x: 9,
            ..valid.clone()
        };
        assert_eq!(
            upload_regions(std::slice::from_ref(&valid), 10, 10),
            [(1, 2, 3, 4)]
        );
        assert_eq!(upload_regions(&[valid, invalid], 10, 10), [(0, 0, 10, 10)]);
    }
}

impl Frame {
    /// Copy a complete CEF software frame (top-down, premultiplied BGRA).
    ///
    /// # Safety
    ///
    /// `buffer` must be the live OnPaint buffer for these dimensions.
    pub unsafe fn copy_bgra(
        buffer: *const u8,
        width: i32,
        height: i32,
        window: &SurfaceTarget,
        previous: Option<&Self>,
        dirty: &[cef::Rect],
    ) -> Result<Self> {
        let width = u32::try_from(width)?;
        let height = u32::try_from(height)?;
        let stride = width.checked_mul(4).context("BGRA row is too wide")?;
        let len = usize::try_from(u64::from(stride) * u64::from(height))?;
        ensure!(
            !buffer.is_null() && width != 0 && height != 0 && isize::try_from(len).is_ok(),
            "invalid CEF software frame"
        );
        // CEF supplies width * height * 4 readable bytes until OnPaint returns.
        let data = unsafe { std::slice::from_raw_parts(buffer, len) };

        #[cfg(target_os = "linux")]
        if let Some(gpu) = &window.gpu {
            let max_size = gpu.device.limits().max_texture_dimension_2d;
            ensure!(
                width <= max_size && height <= max_size,
                "frame exceeds the renderer limit"
            );
            let size = wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            };
            if let Some(
                prior @ Self(FrameStorage::Texture {
                    device,
                    software_texture: Some(texture),
                    ..
                }),
            ) = previous
                && *device == gpu.device
                && texture.size() == size
            {
                for (x, y, width, height) in upload_regions(dirty, width, height) {
                    if width == 0 || height == 0 {
                        continue;
                    }
                    gpu.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            origin: wgpu::Origin3d { x, y, z: 0 },
                            ..texture.as_image_copy()
                        },
                        data,
                        wgpu::TexelCopyBufferLayout {
                            offset: u64::from(y) * u64::from(stride) + u64::from(x) * 4,
                            bytes_per_row: Some(stride),
                            rows_per_image: None,
                        },
                        wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                return Ok(prior.clone());
            }
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("CEF software frame"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            gpu.queue.write_texture(
                texture.as_image_copy(),
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
                size,
            );
            return Ok(Self(FrameStorage::Texture {
                view: texture.create_view(&Default::default()),
                device: gpu.device.clone(),
                source_uv: [0.0, 0.0, 1.0, 1.0],
                software_texture: Some(texture),
            }));
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (window, previous, dirty);

        let image = image::RgbaImage::from_raw(width, height, data.to_vec())
            .context("invalid BGRA frame length")?;
        Ok(Self(FrameStorage::Image(Arc::new(RenderImage::new(
            smallvec::smallvec![image::Frame::new(image)],
        )))))
    }

    #[cfg(target_os = "linux")]
    /// Copy a callback-scoped dma-buf, then retain the view of the owned texture.
    ///
    /// # Safety
    ///
    /// Call inside the originating callback with the GPU used by CEF.
    pub unsafe fn copy_accelerated(
        info: &AcceleratedPaintInfo,
        target: &SurfaceTarget,
    ) -> Result<Self> {
        let gpu = target.gpu.as_ref().context("missing GPUI device")?;
        let texture = unsafe { import_accelerated(info, &gpu.device, &gpu.queue) }?;
        let visible = &info.extra.visible_rect;
        let right = visible
            .x
            .checked_add(visible.width)
            .context("visible rect overflows")?;
        let bottom = visible
            .y
            .checked_add(visible.height)
            .context("visible rect overflows")?;
        ensure!(
            visible.x >= 0
                && visible.y >= 0
                && visible.width > 0
                && visible.height > 0
                && u32::try_from(right)? <= texture.width()
                && u32::try_from(bottom)? <= texture.height(),
            "CEF visible rect lies outside the coded frame"
        );
        let width = texture.width() as f32;
        let height = texture.height() as f32;
        Ok(Self(FrameStorage::Texture {
            view: texture.create_view(&Default::default()),
            device: gpu.device.clone(),
            source_uv: [
                visible.x as f32 / width,
                visible.y as f32 / height,
                right as f32 / width,
                bottom as f32 / height,
            ],
            software_texture: None,
        }))
    }

    pub fn image(&self) -> Option<&Arc<RenderImage>> {
        match &self.0 {
            FrameStorage::Image(image) => Some(image),
            #[cfg(target_os = "linux")]
            FrameStorage::Texture { .. } => None,
        }
    }
}

/// Copy CEF's dma-buf (Vulkan), IOSurface (Metal), or shared D3D11 texture
/// (D3D12/Vulkan interop) into a wgpu-owned texture.
///
/// # Safety
///
/// Call inside the originating OnAcceleratedPaint callback. The device and
/// queue must match each other and the CEF producer's GPU and Vulkan driver.
pub unsafe fn import_accelerated(
    info: &AcceleratedPaintInfo,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<wgpu::Texture> {
    let source = cef::osr_texture_import::SharedTextureHandle::new(info);
    // The importer finishes the source read before returning.
    Ok(unsafe { source.import_texture(device, queue) }?)
}

/// The renderer pair supplied by GPUI, or a software image target.
///
/// ```compile_fail
/// let mut target = flecha_surface::SurfaceTarget::default();
/// target.gpu = None;
/// ```
#[derive(Clone, Default)]
pub struct SurfaceTarget {
    #[cfg(target_os = "linux")]
    gpu: Option<gpui::SurfaceDevice>,
}

impl SurfaceTarget {
    #[cfg(target_os = "linux")]
    pub fn adapter(&self) -> Option<&wgpu::Adapter> {
        self.gpu.as_ref().map(|gpu| &gpu.adapter)
    }

    #[cfg(target_os = "linux")]
    pub fn is_same_device(&self, other: &Self) -> bool {
        self.gpu.as_ref().map(|gpu| &gpu.device) == other.gpu.as_ref().map(|gpu| &gpu.device)
    }

    pub fn for_window(window: &gpui::Window) -> Self {
        #[cfg(target_os = "linux")]
        {
            Self {
                gpu: window.surface_device(),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = window;
            Self::default()
        }
    }
}

/// Paint the view followed by its optional popup. Popup bounds are in logical
/// pixels relative to the browser view, as supplied by CEF.
pub fn surface(view: Option<Frame>, popup: Option<(Bounds<Pixels>, Frame)>) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                if let Some(view) = view {
                    paint(view, bounds, window);
                }
                if let Some((relative, frame)) = popup {
                    let popup = Bounds::new(bounds.origin + relative.origin, relative.size);
                    paint(frame, popup, window);
                }
            });
        },
    )
    .size_full()
}

fn paint(frame: Frame, bounds: Bounds<Pixels>, window: &mut gpui::Window) {
    match frame.0 {
        #[cfg(target_os = "linux")]
        FrameStorage::Texture {
            view,
            device,
            source_uv,
            ..
        } => {
            if window
                .surface_device()
                .is_some_and(|gpu| gpu.device == device)
            {
                window.paint_texture_surface(bounds, view, source_uv);
            }
        }
        FrameStorage::Image(image) => {
            if let Err(error) =
                window.paint_image(bounds, bounds, Default::default(), image, 0, false)
            {
                eprintln!("CEF image paint: {error:#}");
            }
        }
    }
}
pub mod input;
