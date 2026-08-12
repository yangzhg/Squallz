#![forbid(unsafe_code)]
//! sqz: the Squallz command-line entry point. Shares squallz-core with the
//! GUI and squallz-i18n for language packs.
//!
//! This file only assembles the pieces: argument parsing, localizer setup,
//! Ctrl-C wiring and error rendering. The actual work lives in `commands/`.

mod args;
mod commands;
mod content_policy;
mod errors;
mod file_manager_presets;
mod progress;
mod prompt;
mod ui;

use std::any::Any;
use std::panic;
use std::sync::Arc;

use clap::Parser;
use squallz_core::api::ControlToken;
use squallz_core::{Engine, SFX_CLI_STUB_MARKER};
use squallz_i18n::Localizer;

use crate::args::Cli;
use crate::commands::Ctx;
use crate::errors::{
    error_kind, exit_code, localize_error, localize_update_error, update_error_kind,
    update_exit_code, CliError,
};

#[used]
static SFX_STUB_IDENTITY: [u8; 24] = SFX_CLI_STUB_MARKER;

fn main() {
    install_broken_pipe_panic_hook();
    match panic::catch_unwind(run_cli) {
        Ok(()) => {}
        Err(payload) if is_broken_pipe_payload(payload.as_ref()) => std::process::exit(0),
        Err(payload) => panic::resume_unwind(payload),
    }
}

fn run_cli() {
    if let Ok(path) = std::env::current_exe() {
        let args = std::env::args_os().skip(1).collect::<Vec<_>>();
        match squallz_sfx_runtime::probe(&path) {
            Ok(false) => {}
            Ok(true) | Err(_) => std::process::exit(squallz_sfx_runtime::run(&path, &args)),
        }
    }

    if let Some(code) = args::try_print_localized_help(std::env::args_os()) {
        std::process::exit(code);
    }

    let cli = Cli::parse();
    let json_errors = cli.cmd.json_requested();
    let output_style = cli.output_style;
    let color = cli.color;
    let accent = cli.accent;
    let (ctx, loc) = command_context(
        cli.lang.as_deref(),
        cli.quiet,
        cli.verbose,
        output_style,
        color,
        accent,
    );
    finish_command(
        commands::dispatch(cli.cmd, &ctx),
        &ctx,
        &loc,
        json_errors,
        output_style,
    );
}

fn command_context(
    lang: Option<&str>,
    quiet: bool,
    verbose: bool,
    output_style: args::OutputStyleArg,
    color: args::ColorArg,
    accent: args::AccentArg,
) -> (Ctx, Arc<Localizer>) {
    let loc = Arc::new(Localizer::load(lang));
    let ctl = ControlToken::new();
    let handler_ctl = Arc::clone(&ctl);
    let _ = ctrlc::set_handler(move || handler_ctl.cancel());
    (
        Ctx {
            engine: Engine::new(squallz_formats::registry()),
            loc: Arc::clone(&loc),
            ctl,
            quiet,
            verbose,
            output_style,
            color,
            accent,
        },
        loc,
    )
}

fn finish_command(
    result: Result<(), CliError>,
    ctx: &Ctx,
    loc: &Localizer,
    json_errors: bool,
    output_style: args::OutputStyleArg,
) {
    match result {
        Ok(()) => {}
        Err(CliError::Format(e)) => {
            let message = localize_error(loc, &e);
            let code = exit_code(&e);
            if json_errors {
                if print_json_error(error_kind(&e), &message, code).is_err() {
                    print_human_error(ctx, output_style, loc, &message);
                }
            } else {
                print_human_error(ctx, output_style, loc, &message);
            }
            std::process::exit(code);
        }
        Err(CliError::Update(e)) => {
            let message = localize_update_error(loc, &e);
            let code = update_exit_code(&e);
            if json_errors {
                if print_json_error(update_error_kind(&e), &message, code).is_err() {
                    print_human_error(ctx, output_style, loc, &message);
                }
            } else {
                print_human_error(ctx, output_style, loc, &message);
            }
            std::process::exit(code);
        }
        Err(CliError::Exit(code)) => std::process::exit(code),
    }
}

fn print_json_error(kind: &str, message: &str, code: i32) -> Result<(), serde_json::Error> {
    let value = serde_json::json!({
        "ok": false,
        "error": {
            "kind": kind,
            "message": message,
            "exit_code": code,
        }
    });
    let text = serde_json::to_string_pretty(&value)?;
    println!("{text}");
    Ok(())
}

fn print_human_error(
    ctx: &Ctx,
    output_style: args::OutputStyleArg,
    loc: &Localizer,
    message: &str,
) {
    let line = loc.format("cli.error_prefix", &[("message", message)]);
    if output_style.is_modern() {
        eprintln!(
            "{}",
            ctx.paint_stderr_tone(ui::Tone::Danger, &format!("✕ {line}"))
        );
    } else {
        eprintln!("{line}");
    }
}

fn install_broken_pipe_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if is_broken_pipe_payload(info.payload()) {
            return;
        }
        default_hook(info);
    }));
}

fn is_broken_pipe_payload(payload: &(dyn Any + Send)) -> bool {
    panic_payload_message(payload).is_some_and(|message| {
        message.contains("failed printing to stdout") && message.contains("Broken pipe")
    })
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> Option<&str> {
    if let Some(message) = payload.downcast_ref::<String>() {
        Some(message.as_str())
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        Some(*message)
    } else {
        None
    }
}
