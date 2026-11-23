//! The boot splash, for `scene-core` (SCENES.md).
//!
//! As the disc's first program, the launcher plays the SDK's "Built with PSoXide" splash once at
//! boot, from the SDK's boot-only memory (sdk/psoxide-boot.ld, linked by the Makefile when
//! LAUNCHER_FEATURES has `scene-core`): psx_splash's code and data are stored after `.data` in the
//! EXE, loaded with it in the same read, moved above `.bss` by psx-rt's `__boot_entry` and run from
//! there. Nothing of it stays resident. The three games play none. Without the feature `splash`
//! does nothing and the launcher is the default build, byte for byte.

/// Play the splash once, before the launcher sets up its own display: on a GPU of its own, so
/// the launcher's `gpu::init` and uploads (its BASIC font sits where the splash puts the same
/// font) start from a clean screen. The splash waits on the VBlank counter, so it goes in first,
/// after the pad's first poll (the analog handshake runs before interrupts are on). The pad is
/// read with the launcher's own `PadReader`, so the splash pulls in no second pad driver, and
/// this function is boot-only too (`.boot_only`), so its setup leaves no code in `main`.
#[cfg(feature = "scene-core")]
#[inline(never)]
#[link_section = ".boot_only.launcher_splash"]
#[allow(deprecated)] // `PadReader::poll`, as `main` reads the pad
pub fn splash() {
    use psx_gpu::display::{DisplayConfig, DoubleBuffer, Resolution, VideoMode};
    use psx_gpu::Gpu;

    // SAFETY: nothing else has the GPU or its DMA yet; `main` sets the GPU up again after this
    // returns.
    let dma = unsafe { psx_io::periph::GpuDma::steal() };
    let mut gpu = Gpu::new(dma, DisplayConfig::new(VideoMode::Ntsc, Resolution::R320X240));
    let mut buffers = DoubleBuffer::new(Resolution::R320X240);
    gpu.set_draw_area((0, 0), (319, 239));
    gpu.set_draw_offset((0, 0));
    let mut pad = psx_pad::PadReader::port1();
    let _ = pad.poll();
    psx_rt::interrupts::install_vblank_counter();
    psx_splash::show(
        &mut gpu,
        &mut buffers,
        "Built with PSoXide",
        psx_display::Brightness::DEFAULT,
        || pad.poll().buttons,
    );
}

/// Without `scene-core` the launcher plays no splash.
#[cfg(not(feature = "scene-core"))]
#[inline(always)]
pub fn splash() {}
