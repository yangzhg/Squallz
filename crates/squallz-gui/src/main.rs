#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use squallz_core::SFX_GUI_STUB_MARKER;

#[used]
static SFX_STUB_IDENTITY: [u8; 24] = SFX_GUI_STUB_MARKER;

fn main() {
    squallz_gui::run();
}
