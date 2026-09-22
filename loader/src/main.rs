//! Collection-specific visuals over the shared engine chainloader.
#![no_std]
#![no_main]
use psx_chainloader::paint;
use psx_chainloader::runtime::{self, Presentation};
struct LoadingScreen;
impl Presentation for LoadingScreen {
    fn setup(&mut self) {
        paint::setup(320, 255);
    }
    fn begin(&mut self) {
        paint::rect(0, 0, 320, 240, 0);
        paint::show();
        paint::rect(80, 114, 160, 12, 0x0030_3030);
    }
    fn update(&mut self, done: u32, total: u32) {
        let width = (done * 160 / total.max(1)) as i16;
        paint::rect(80, 114, width.clamp(2, 160), 12, paint::WHITE);
    }
    fn finish(&mut self) {}
    fn diagnostic_mode(&mut self) {}
    fn no_scratchpad(&mut self) {
        paint::text(80, 90, 2, "NOSPAD", paint::YELLOW);
    }
}
/// Enter the copied high-RAM blob with a trusted packed executable.
/// # Safety
/// All shared `runtime::run` extent, hardware ownership and stack requirements apply.
#[link_section = ".text.loader_entry"]
#[no_mangle]
pub unsafe extern "C" fn loader_entry(
    exe_lba: u32,
    lba_offset: u32,
    track_base: u32,
    checksum: u32,
) -> ! {
    unsafe {
        runtime::run(
            exe_lba,
            lba_offset,
            track_base,
            checksum,
            loader_entry as *const () as u32,
            LoadingScreen,
        )
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    runtime::panic(&mut LoadingScreen)
}
