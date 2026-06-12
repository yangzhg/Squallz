#![forbid(unsafe_code)]
//! sqz: the Squallz command-line entry point. Shares squallz-core with the
//! GUI and squallz-i18n for language packs.
//!
//! This file only assembles the pieces: argument parsing, localizer setup,
//! Ctrl-C wiring and error rendering. The actual work lives in `commands/`.

mod args;
mod commands;
mod errors;
mod progress;
mod prompt;
mod ui;

use std::any::Any;
use std::panic;
use std::sync::Arc;

use clap::Parser;
use squallz_core::api::ControlToken;
use squallz_core::Engine;
use squallz_i18n::Localizer;

use crate::args::Cli;
use crate::commands::Ctx;
use crate::errors::{error_kind, exit_code, localize_error, CliError};

fn main() {
    install_broken_pipe_panic_hook();
    match panic::catch_unwind(run_cli) {
        Ok(()) => {}
        Err(payload) if is_broken_pipe_payload(payload.as_ref()) => std::process::exit(0),
        Err(payload) => panic::resume_unwind(payload),
    }
}

fn run_cli() {
    if let Some(code) = args::try_print_localized_help(std::env::args_os()) {
        std::process::exit(code);
    }

    let cli = Cli::parse();
    let json_errors = cli.cmd.json_requested();
    let output_style = cli.output_style;
    let color = cli.color;
    let accent = cli.accent;
    let loc = Arc::new(Localizer::load(cli.lang.as_deref()));
    let ctl = ControlToken::new();

    // Ctrl-C cancels gracefully: workers notice at the next chunk boundary
    // and unwind with FormatError::Cancelled (exit code 5). Failing to
    // install the handler must not break the CLI itself.
    let handler_ctl = Arc::clone(&ctl);
    let _ = ctrlc::set_handler(move || handler_ctl.cancel());

    let ctx = Ctx {
        engine: Engine::new(squallz_formats::registry()),
        loc: Arc::clone(&loc),
        ctl,
        quiet: cli.quiet,
        verbose: cli.verbose,
        output_style,
        color,
        accent,
    };
    match commands::dispatch(cli.cmd, &ctx) {
        Ok(()) => {}
        Err(CliError::Format(e)) => {
            let message = localize_error(&loc, &e);
            let code = exit_code(&e);
            if json_errors {
                if print_json_error(error_kind(&e), &message, code).is_err() {
                    print_human_error(&ctx, output_style, &loc, &message);
                }
            } else {
                print_human_error(&ctx, output_style, &loc, &message);
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
