// Start mock ingester
// Start generating logs
// Start agent under flamegraph
// Kill agent

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use file_rotate::{FileRotate, RotationMode};

use memmap::MmapOptions;
use owning_ref::OwningHandle;

use structopt::StructOpt;

const CARGO_MANIFEST_DIR: &'static str = "CARGO_MANIFEST_DIR";

#[derive(Debug, StructOpt)]
#[structopt(name = "agent throughput bench")]
struct Opt {
    /// Dict file
    #[structopt(parse(from_os_str))]
    dict: PathBuf,

    /// Output directory
    #[structopt(parse(from_os_str), short)]
    out_dir: PathBuf,

    /// Number of files to retain during rotation
    #[structopt(long)]
    file_history: usize,

    /// Cut of Bytes before rotation
    #[structopt(long)]
    file_size: usize,
}

fn main() -> Result<(), std::io::Error> {
    let opt = Opt::from_args();
    println!("{:?}", opt);

    // Parse mmap into Vec<&str>
    let words = OwningHandle::new_with_fn(
        Box::new(unsafe { MmapOptions::new().map(&File::open(opt.dict)?)? }),
        |dict_arr_ptr| unsafe {
            dict_arr_ptr
                .as_ref()
                .unwrap()
                .split(|c| c == &b'\n')
                .map(|s| std::str::from_utf8(s).unwrap())
                .collect::<Vec<&str>>()
        },
    );

    let mut manifest_path = std::path::PathBuf::from(std::env::var(CARGO_MANIFEST_DIR).unwrap());
    manifest_path.pop();
    manifest_path.push("bin/Cargo.toml");
    println!("{:?}", manifest_path);
    let run = escargot::CargoBuild::new()
        .bin("logdna-agent")
        .current_release()
        .current_target()
        .no_default_features()
        .manifest_path(manifest_path)
        .run()
        .unwrap();

    println!("artifact={}", run.path().display());

    println!("word count: {}", words.len());

    let mut out_file: PathBuf = opt.out_dir.clone();
    out_file.push("test.log");

    fs::create_dir(opt.out_dir)?;

    let mut log = FileRotate::new(
        out_file,
        RotationMode::BytesSurpassed(opt.file_size),
        opt.file_history,
    );

    for word in words.iter() {
        writeln!(log, "{}", word)?;
    }

    Ok(())
}
