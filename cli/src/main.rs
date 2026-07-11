use std::ffi::{OsStr, OsString};
use std::ops::Deref;
use std::path::PathBuf as OsPath;

use acorn_dfs::new_map::filesystem::DirEntry;
use acorn_dfs::new_map::sys_structures::FormatE;
use acorn_dfs::new_map::{FaultValue, Path};
use clap::Parser;
use mmap_io::MemoryMappedFile;

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

enum DataSource {
    Mmap(MemoryMappedFile),
    Vec(Vec<u8>),
}

impl Deref for DataSource {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Mmap(memory_mapped_file) => memory_mapped_file
                .as_slice_bytes(0, memory_mapped_file.len())
                .unwrap(),
            Self::Vec(items) => &items[..],
        }
    }
}

fn main() -> Result<(), std::io::Error> {
    let args = Args::parse();

    let src = read_file(&args.image_path)?;

    let contents = src;

    let maybe_disk = FormatE::parse(&contents);

    let mut disk = match maybe_disk {
        Ok(disk) => disk,
        Err(e) => unimplemented!("Parse failed: {e:}"),
    };

    //disk.expand_tree().expect("Explode");
    if !disk.faults.is_empty() {
        panic!("Explode");
    }

    match args.verb {
        Verb::Meta => {
            println!("{}", disk.get_map_json());
        }
        Verb::List { prefix } => {
            for k in disk.entries(prefix) {
                println!("{k}");
            }
        }
        Verb::ExtractFile { path, destination } => match disk.get_file(&path) {
            Ok(FaultValue(Ok((entry, contents)), _)) => {
                write_file_plus_metadata(destination, &entry, contents).unwrap()
            }
            Ok(FaultValue(Err(e), _)) => {
                panic!("Could not find file at {path} on the disk: {e}")
            }
            Err(e) => {
                panic!("Parse error trying to extract {path} on the disk: {e}")
            }
        },
        Verb::ExtractAll {
            destination_folder: destination,
        } => {
            extract_disk(&mut disk, destination);
        }
    };
    Ok(())
}

fn read_file(path: &OsStr) -> Result<DataSource, std::io::Error> {
    let src = match MemoryMappedFile::open_ro(path) {
        Ok(f) => DataSource::Mmap(f),
        Err(mmap_io::MmapIoError::Io(error)) => return Err(error),
        Err(_) => std::fs::read(path).map(DataSource::Vec)?,
    };

    Ok(src)
}

fn write_file_plus_metadata(
    destination: OsPath,
    entry: &DirEntry,
    contents: Vec<u8>,
) -> Result<(), std::io::Error> {
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

fn extract_disk(disk: &mut FormatE, destination: OsPath) {
    let keys: Vec<_> = disk.entries(None).collect();

    for path in keys {
        let Ok(FaultValue(Ok((entry, contents)), _)) = disk.get_file(&path) else {
            continue;
        };

        let disk_path = convert_path_to_os(path.clone());

        let dest_file = destination.clone().join(disk_path);

        write_file_plus_metadata(dest_file, &entry, contents).unwrap();
    }
}
