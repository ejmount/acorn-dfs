use std::ffi::OsString;
use std::path::PathBuf as OsPath;

use acorn_dfs::new_map::Path;
use acorn_dfs::new_map::filesystem::DirEntry;
use acorn_dfs::new_map::sys_structures::FormatE;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The disk image to load
    image_path: OsString,
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Clone, Parser)]
pub enum Verb {
    Meta,
    #[command(id = "extract")]
    ExtractFile {
        #[arg(short, long)]
        #[arg(value_parser = Path::try_from_str)]
        path: Path,
        #[arg(short, long)]
        destination: OsPath,
    },
    ExtractAll {
        destination_folder: OsPath,
    },
    List {
        #[arg(short, long)]
        #[arg(value_parser = Path::try_from_str)]
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
        Verb::Meta => {
            println!("{}", disk.get_map_json());
        }
        Verb::List { prefix } => {
            let tree = disk.tree.unwrap();
            for k in tree.keys_by_prefix(prefix.unwrap_or_default()) {
                println!("{k}");
            }
        }
        Verb::ExtractFile { path, destination } => match disk.get_file(&path) {
            Ok((entry, contents)) => {
                write_file_plus_metadata(destination, &entry, contents).unwrap()
            }
            Err(e) => {
                panic!("Could not find file at {path} on the disk: {e}")
            }
        },
        Verb::ExtractAll {
            destination_folder: destination,
        } => {
            extract_disk(&disk, destination);
        }
    }
}

fn write_file_plus_metadata(
    destination: OsPath,
    entry: &DirEntry,
    contents: Vec<u8>,
) -> Result<(), std::io::Error> {
    dbg!(&destination);

    let mut folder = destination.clone();
    folder.pop();

    std::fs::create_dir_all(&folder).unwrap();

    std::fs::write(&destination, contents)?;
    let mut inf_path = destination.clone();
    inf_path.set_extension("inf");
    let inf_data = inf_data(entry);
    std::fs::write(inf_path, inf_data)
}

fn inf_data(dir: &DirEntry) -> String {
    use std::fmt::Write;
    let DirEntry {
        obj_name,
        load,
        exec,
        len,
        attrs,
        ..
    } = dir;
    let mut s = String::new();
    write!(s, "\"{obj_name}\" {load:X} {exec:X} {len} {}", attrs.bits()).unwrap();
    s
}

fn convert_path_to_os(p: Path) -> OsPath {
    let mut os_path = OsPath::new();
    for s in p.to_byte_segments() {
        let raw_str = str::from_utf8(&s).expect("ADFS name should be valid UTF8");
        let segment = raw_str.replace("/", ".").replace("!", "$");
        os_path.push(segment);
    }
    os_path
}

fn extract_disk(disk: &FormatE, destination: OsPath) {
    let tree = disk.tree.as_ref().unwrap();
    let m = disk.get_map_json();
    eprintln!("{m}");

    for path in tree.keys() {
        let Ok((entry, contents)) = disk.get_file(path) else {
            continue;
        };

        let disk_path = convert_path_to_os(path.clone());

        let dest_file = destination.clone().join(disk_path);

        write_file_plus_metadata(dest_file, &entry, contents).unwrap();
    }
}
