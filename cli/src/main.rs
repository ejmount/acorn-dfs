use std::ffi::OsString;
use std::path::PathBuf as OsPath;

use acorn_dfs::new_map::Path;
use acorn_dfs::new_map::sys_structures::FormatE;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    /// The image to load
    image_path: OsString,
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Clone, Parser)]
pub enum Verb {
    #[command(id = "extract")]
    ExtractFile {
        #[arg(short, long)]
        #[arg(value_parser = Path::from_str)]
        path: Path,
        #[arg(short, long)]
        destination: OsPath,
    },
    List {
        #[arg(short, long)]
        #[arg(value_parser = Path::from_str)]
        prefix: Option<Path>,
    },
}

fn main() {
    let args = Args::parse();

    let contents = match std::fs::read(&args.image_path) {
        Ok(contents) => contents,
        Err(err) => panic!("Could not read {:?}: {}", args.image_path, err),
    };

    let maybe_disk = FormatE::parse(&contents);

    let mut disk = match maybe_disk {
        Ok(disk) => disk,
        Err(e) => unimplemented!("Parse failed: {e:}"),
    };

    disk.expand_tree().expect("Explode");
    if !disk.faults.is_empty() {
        panic!("Explode");
    }

    match args.verb {
        Verb::List { prefix } => {
            let tree = disk.tree.unwrap();
            for k in tree.keys_by_prefix(prefix.unwrap_or_default()) {
                println!("{k}");
            }
        }
        Verb::ExtractFile { path, destination } => match disk.get_file(&path) {
            Ok(contents) => std::fs::write(destination, contents).unwrap(),
            Err(e) => {
                panic!("Could not find file at {path} on the disk: {e}")
            }
        },
    }
}
