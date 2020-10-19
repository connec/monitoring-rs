use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::channel;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

fn main() -> Result<(), Box<dyn Error>> {
    let container_log_directory = fs::canonicalize(
        env::var("CONTAINER_LOG_DIRECTORY").unwrap_or_else(|_| "/var/log/containers".to_string()),
    )?;

    let mut files = HashMap::new();
    for entry in fs::read_dir(&container_log_directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            continue;
        }

        let path = fs::canonicalize(entry.path())?;
        open_file(&mut files, path)?;
    }

    let (tx, rx) = channel();
    let mut watcher: RecommendedWatcher = Watcher::new_raw(tx)?;
    watcher.watch(container_log_directory, RecursiveMode::NonRecursive)?;

    let mut stdout = io::stdout();

    for event in rx {
        // let's `dbg` the events as we handle them
        if let Some(path) = event.path {
            let file = if let Some(file) = files.get_mut(&path) {
                file
            } else if let Some(file) = open_file(&mut files, path)? {
                file
            } else {
                continue;
            };
            io::copy(file, &mut stdout)?;
        }
    }

    Ok(())
}

fn open_file(files: &mut HashMap<PathBuf, File>, path: PathBuf) -> io::Result<Option<&mut File>> {
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    file.seek(SeekFrom::End(0))?;

    // We expect the key not to be set – https://github.com/rust-lang/rust/issues/65225 would
    // resolve this mismatch.
    let file = files.entry(path).or_insert(file);

    Ok(Some(file))
}
