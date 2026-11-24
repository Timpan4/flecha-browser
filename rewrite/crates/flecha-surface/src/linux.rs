use std::{fs::File, os::unix::fs::MetadataExt as _, path::PathBuf};

use anyhow::{Context as _, Result, ensure};
use ash::vk;
use raw_window_handle::{HasDisplayHandle as _, RawDisplayHandle};
use x11rb::{connection::Connection as _, protocol::dri3::ConnectionExt as _};

/// The Ozone platform that matches the window's display connection.
pub fn ozone_platform(window: &gpui::Window) -> &'static str {
    match window.display_handle().map(|display| display.as_raw()) {
        Ok(RawDisplayHandle::Wayland(_)) => "wayland",
        _ => "x11",
    }
}

/// The DRM render node of GPUI's Vulkan adapter. Chromium allocates its GBM buffers there
/// when given `--render-node-override`, so CEF and GPUI use the same GPU. On X11,
/// Chromium opens its GBM device through DRI3 instead, so that device must match too.
pub fn render_node(adapter: &wgpu::Adapter, ozone: &str) -> Result<PathBuf> {
    ensure!(
        adapter
            .features()
            .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF),
        "GPUI adapter does not support Vulkan dma-buf import"
    );

    // The guard keeps the Vulkan adapter and instance alive during this read.
    let adapter = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() }
        .context("GPUI adapter is not Vulkan")?;
    ensure!(
        adapter
            .physical_device_capabilities()
            .supports_extension(ash::ext::physical_device_drm::NAME),
        "GPUI adapter lacks VK_EXT_physical_device_drm"
    );
    let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
    unsafe {
        adapter
            .shared_instance()
            .raw_instance()
            .get_physical_device_properties2(adapter.raw_physical_device(), &mut properties);
    }
    ensure!(
        drm.has_render != vk::FALSE,
        "GPUI adapter has no DRM render node"
    );
    let render_id = (
        u32::try_from(drm.render_major)?,
        u32::try_from(drm.render_minor)?,
    );
    let path = PathBuf::from(format!("/dev/dri/renderD{}", render_id.1));
    ensure!(
        device_id(path.metadata()?.rdev()) == render_id,
        "GPUI render node identity changed"
    );

    if ozone == "x11" {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let device = connection
            .dri3_open(root, 0)?
            .reply()
            .context("the X server has no DRI3 device")?
            .device_fd;
        let id = device_id(File::from(device).metadata()?.rdev());
        let primary = (drm.has_primary != vk::FALSE)
            .then(|| -> Result<_> {
                Ok((
                    u32::try_from(drm.primary_major)?,
                    u32::try_from(drm.primary_minor)?,
                ))
            })
            .transpose()?;
        ensure!(
            id == render_id || Some(id) == primary,
            "X11 GBM and GPUI use different GPUs"
        );
    }
    Ok(path)
}

fn device_id(raw: u64) -> (u32, u32) {
    (libc::major(raw), libc::minor(raw))
}
