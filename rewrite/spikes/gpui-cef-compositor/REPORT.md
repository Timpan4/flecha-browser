# Spike 1: GPUI and CEF composition

## Result

The Linux demo builds and composites real CEF dma-bufs with GPUI on headless
Weston and an RTX 5080. Pixel checks cover advancing frames, resize, the initial
Wayland scale transition from 1× to 2×, and the GPUI overlay. An injected
invalid dma-buf was rejected by the importer and the demo recovered by creating
a software OSR browser. Xvfb uses software OSR because its X server cannot
provide the DRI3 device that CEF's X11 allocator requires.

## Toolkit and engine choices

- **Bite GPUI.** The [dma-buf arm](https://github.com/bite-gpui/bite-gpui/pull/10)
  uses Apache-2.0 GPUI and renderer crates in a repository with Apache and GPL
  components. Its October 2 merge predates the November 22 Zed pin by 51 days.
  It depends on the fork's split `gpui_engine` and `gpui_authoring` crates.
  [gpui_interop](https://github.com/bite-gpui/gpui_interop) is Apache-2.0, and its
  0.2.1 release targets Bite 1.25.203 and registry wgpu 30.0.1. Its producer-owned
  dma-buf API would still need a copy for CEF's callback-scoped buffers.
- **gpui-pre.** The patched GPUI, wgpu, and Windows crates declare Apache-2.0.
  They start at Zed `08cbb88`, 366 commits behind the pin. The fork adds Linux
  dma-buf import in wgpu and Windows shared D3D11 surfaces. Its own dma-buf
  lifetime and renderer cache would need adapting for CEF. [Fork and
  license](https://github.com/vegidio/gpui-pre) · [Zed
  comparison](https://github.com/zed-industries/zed/compare/08cbb88...f126950f9d3b62666f97437ae5d4ccac2272c4db)
- **Selected Zed patch.** [zed.patch](patches/zed.patch) keeps the exact Zed
  revision. It exposes the Linux window's wgpu adapter, device, and queue,
  requests the dma-buf feature when available, and adds a texture view and
  source rectangle to its surface primitive. The wgpu renderer reuses its
  surface pipeline and clips and draws in GPUI scene order. The native Metal
  and D3D11 renderers use their existing code.
- **Shared wgpu.** [cef-rs.patch](patches/cef-rs.patch) keeps the exact cef-rs
  revision from the Flecha fork. It changes cef-rs's wgpu dependency to Zed's
  `cddb2f5d80dc0009b2ef6767635140cacf90a007` and adapts the four native import
  calls to the pin's explicit `cleared` parameter. The imported producer
  textures already contain pixels. The lockfile resolves one wgpu package.

Flecha matches the Wayland CEF GBM render node to GPUI's Vulkan adapter. On X11
it checks the DRM device returned by DRI3 before enabling shared textures.
cef-rs duplicates the descriptor and imports it without asking Vulkan to
re-export the dma-buf. This is the NVIDIA-sensitive distinction documented in
Flecha's [Chromium patch](../../../chromium/patches/0079-nvidia-vulkan-dmabuf-import.patch).
The import, copy, release of external queue ownership, and GPU wait complete
during the paint callback. The GPUI scene sees a view of the copy, cropped to
CEF's visible rectangle. Software frames use the same Linux scene path with
an upload from the callback's BGRA buffer.

The demo uses a fixed fixture and an in-memory browser context with an
isolated CEF root directory. Fallback closes the old browser before creating
its replacement. That resets the fixture and counter while retaining the
last drawable frame until the replacement paints. The spike pumps CEF at
GPUI frame cadence. Idle scheduling and general navigation state are separate
work.

## Build and reproduce

From `rewrite/` on the workstation:

```sh
bash spikes/gpui-cef-compositor/prepare.sh
CARGO_NET_GIT_FETCH_WITH_CLI=true \
  flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run cargo build -j 14 --locked

VK_DRIVER_FILES=/usr/share/vulkan/icd.d/nvidia_icd.json \
  bash spikes/gpui-cef-compositor/verify-linux.sh wayland 1 fallback
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/nvidia_icd.json \
  bash spikes/gpui-cef-compositor/verify-linux.sh wayland 2 auto
bash spikes/gpui-cef-compositor/verify-linux.sh x11 1 auto input
bash spikes/gpui-cef-compositor/verify-linux.sh x11 2 software
```

`prepare.sh` downloads the source archives from `pins.toml`, applies the two
patches, and checks the beta archive against its Linux minimal SHA-1
`057a0b871bf26d26d654d999dba595b92fe0f067`. The early dependency build used
cef-rs's default, which selected the promoted stable archive of the same CEF
commit. The runs below linked and loaded the verified beta archive through
`CEF_PATH` and `run-linux.sh`. The launcher uses the explicit runtime
directory because Cargo 1.100 changes `OUT_DIR` and cef-rs copies
`libcef.so` and resources into `target/debug/build`. The demo checks that
libcef exposes CEF API version 15800. [CEF build
index](https://cef-builds.spotifycdn.com/index.json) · [cef-rs build-dir
issue](https://github.com/tauri-apps/cef-rs/pull/522)

`cargo fmt --check`, the locked workspace build, and
`cargo clippy --locked -j 14 --workspace --all-targets -- -D warnings` passed.
`cargo test --locked -j 14 --workspace` compiled the test targets and found
no Rust unit or doc tests. The display harness is the behavior check.
`cargo tree --locked -i wgpu` showed CEF and GPUI using the same Git package.

## Headless verification

Host was CachyOS with NVIDIA driver 620.73.04. Weston 15.0.1 used its Vulkan
renderer on a private Wayland socket and Xvfb allocated its own X11 display.
The Weston frames used NVIDIA modifier `0x300000000606014`.

The verifier first waits for the expected size to be visible, then takes
three compositor captures per phase. It decodes the counter and the page's
`innerWidth × devicePixelRatio` and `innerHeight × devicePixelRatio`
from the pixels. It checks the full overlay rectangle and neighboring page
pixels. Early Xvfb captures taken during swapchain resize could be blank
before the new image presented. The stabilized samples cover the new geometry.
These samples do not establish that every intervening frame was visible.

| Display and mode | Initial page pixels | Resized page pixels | Capture verdict |
| --- | --- | --- | --- |
| Weston 1×, accelerated | 800 × 480 | 640 × 360 | Counter, resize and overlay passed |
| Weston 1×, rejected dma-buf | 640 × 360 | 640 × 360 | New software browser advanced the counter and retained the overlay |
| Weston 2×, accelerated | 1600 × 960 | 1280 × 720 | Counter, resize and overlay passed. Log records the live initial 1× to 2× update |
| Xvfb 1×, automatic software | 800 × 480 | 640 × 360 | Counter, resize and overlay passed |
| Xvfb 2×, explicit software | 1600 × 960 | 1280 × 720 | Counter, resize and overlay passed |

The sequential run checked 27 frames. After resize it read counters
`43,45,47` (Weston 1×), `98,103,109` (Weston 2×), `76,79,83` (Xvfb 1×)
and `32,34,36` (Xvfb 2×). After the injected import error the replacement
browser showed `6,8,9`. Both Xvfb resize cases needed a second readiness
capture before the sampled frames.

### Input and outstanding checks

- **Select menu.** Xvfb delivered native mouse events. The app received
  `PET_POPUP`, the screenshot showed the menu over the page, and clicking
  the second item changed the field to `Two` and hid the popup.
- **Basic keyboard input.** After a native X11 click focused the text input,
  the native `x` key produced the fixture's expected text input marker.
- **IME and file drag.** The fixture exposes a text field and file input.
  The demo forwards basic key and pointer events. GPUI's IME composition
  and file drag events still need explicit bridges to CEF and a real-session
  check.
- **Clipboard and screen reader.** On Xvfb, Ctrl+A and Ctrl+C established a
  CLIPBOARD owner after typing, but the second X11 client received no
  SelectionNotify before the probe was stopped. The opt-in
  `verify-linux.sh x11 1 auto clipboard` reproduces the request. The Wayland
  test used Weston's fake seat and isolated the session bus. The owner's
  native Wayland clipboard peer and Orca checks remain open. The page's
  accessibility tree needs a Flecha bridge. [CEF Wayland clipboard
  issue](https://github.com/chromiumembedded/cef/issues/4341)
- **Other display configurations.** Moving a window between differently
  scaled real outputs and X11 with working DRI3 remain open.
- **Performance.** The captures establish pixel correctness. CPU, GPU and
  frame-time comparisons against the shipping browser remain open. The
  shipping benchmark drives the Chromium fork's shell and cannot drive this
  OSR demo as written.

### Other targets

The shared importer calls cef-rs's IOSurface/Metal and shared D3D11
(D3D12/Vulkan interop) paths on their respective targets. The demo uses
GPUI's image path for software frames on those platforms. Native Metal
color-texture composition, the D3D11 renderer surface path, and macOS
helper packaging remain open. This host has only the
`x86_64-unknown-linux-gnu` Rust target and lacks the macOS and Windows SDKs
needed by GPUI and the CEF wrapper. macOS and Windows were neither built nor
run.
