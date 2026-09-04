#![forbid(unsafe_code)]

use squallz_core::SFX_CLI_STUB_MARKER;

#[used]
static SFX_STUB_IDENTITY: [u8; 24] = SFX_CLI_STUB_MARKER;

fn main() {
    std::hint::black_box(&SFX_STUB_IDENTITY);
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    std::process::exit(squallz_sfx_runtime::run_current(&args));
}
