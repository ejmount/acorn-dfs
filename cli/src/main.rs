use std::ffi::{OsStr, OsString};
use std::ops::Deref;
use std::path::PathBuf as OsPath;

use acorn_dfs::new_map::filesystem::DirEntry;
use acorn_dfs::new_map::sys_structures::{Format, NewMapDisk};
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
    /// Output the disc record of the given image
    Meta,
    /// Extract a single file from the given path to a given host file
    #[command(id = "extract")]
    ExtractFile {
        #[arg(short, long)]
        #[arg(value_parser = Path::try_from_str)]
        path: Path,
        #[arg(short, long)]
        destination: OsPath,
    },
    /// Extract all files to the given folder
    ExtractAll { destination_folder: OsPath },
    /// List files on the disk image that begin with the given prefix. (Leave
    /// empty for all files.)
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

    let maybe_disk = NewMapDisk::parse(&contents, Format::E);

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
            for k in disk.entries(prefix).unwrap() {
                println!("{k}");
            }
        }
        Verb::ExtractFile { path, destination } => {
            match write_file_plus_metadata(&disk, destination, &path) {
                Ok(_) => {}
                Err(e) => panic!("Could not extract file at {path}: {e}"),
            }
        }
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

fn write_file_plus_metadata<'a>(
    disk: &'a NewMapDisk,
    destination: OsPath,
    path: &Path,
) -> Result<(), Error<'a>> {
    let mut folder = destination.clone();

    // Assume that destination is pointing at a filename, remove the leaf name to
    // get the bottom-most containing folder.
    folder.pop();

    std::fs::create_dir_all(&folder).unwrap();

    let mut f = std::fs::File::create(&destination).map_err(IoError::StdError)?;

    let FaultValue(entry, _) = disk.get_file(path, &mut f)?;

    let mut inf_path = destination.clone();
    inf_path.set_extension("inf");
    let inf_data = inf_data(&entry);
    std::fs::write(inf_path, inf_data).map_err(|e| IoError::StdError(e).into())
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

fn extract_disk(disk: &mut NewMapDisk, destination: OsPath) {
    let keys: Vec<_> = disk.entries(None).unwrap();

    for path in keys {
        let disk_path = convert_path_to_os(path.clone());

        let dest_file = destination.clone().join(disk_path);

        write_file_plus_metadata(disk, dest_file, &path).unwrap();
    }
}
