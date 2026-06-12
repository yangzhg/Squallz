use std::env;
use std::fs::File;
use std::path::PathBuf;

use sevenz_rust2::{ArchiveReader, ArchiveWriter, Password};

fn exit_error(message: String) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn main() {
    let mut args = env::args_os().skip(1);
    let Some(output) = args.next().map(PathBuf::from) else {
        eprintln!("usage: create_solid_7z <output.7z> <source-dir>");
        std::process::exit(2);
    };
    let Some(source) = args.next().map(PathBuf::from) else {
        eprintln!("usage: create_solid_7z <output.7z> <source-dir>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: create_solid_7z <output.7z> <source-dir>");
        std::process::exit(2);
    }

    let mut writer = match ArchiveWriter::create(&output) {
        Ok(writer) => writer,
        Err(e) => exit_error(format!("cannot create {}: {e}", output.display())),
    };
    match writer.push_source_path(&source, |_| true) {
        Ok(_) => {}
        Err(e) => exit_error(format!(
            "cannot add {} as solid 7z source: {e}",
            source.display()
        )),
    };
    match writer.finish() {
        Ok(_) => {}
        Err(e) => exit_error(format!("cannot finish {}: {e}", output.display())),
    };

    let output_file = match File::open(&output) {
        Ok(file) => file,
        Err(e) => exit_error(format!("cannot reopen {}: {e}", output.display())),
    };
    let reader = match ArchiveReader::new(output_file, Password::empty()) {
        Ok(reader) => reader,
        Err(e) => exit_error(format!("cannot read generated {}: {e}", output.display())),
    };
    if !reader.archive().is_solid {
        eprintln!("generated archive is not solid: {}", output.display());
        std::process::exit(1);
    }
    println!(
        "solid=true files={} blocks={}",
        reader.archive().files.len(),
        reader.archive().blocks.len()
    );
}
