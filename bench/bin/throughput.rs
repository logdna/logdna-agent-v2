// Start mock ingester
// Start generating logs
// Start agent under flamegraph
// Kill agent

use memmap::MmapOptions;
use std::fs::{self, File};
use std::path::PathBuf;
use structopt::StructOpt;

use file_rotate::{FileRotate, RotationMode};
use std::io::Write;

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

    let file = File::open(opt.dict)?;
    let dict_arr = unsafe { MmapOptions::new().map(&file)? };

    let words = dict_arr
        .split(|c| c == &b'\n')
        .map(|s| std::str::from_utf8(&s).unwrap())
        .collect::<Vec<&str>>();
    println!("word count: {}", words.len());

    let mut out_file: PathBuf = opt.out_dir.clone();
    out_file.push("test.log");

    fs::create_dir(opt.out_dir)?;

    let mut log = FileRotate::new(
        out_file,
        RotationMode::BytesSurpassed(opt.file_size),
        opt.file_history,
    );

    for word in words {
        writeln!(log, "{}", word)?;
    }

    Ok(())
}
