use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::thread;
use std::time::Duration;

#[derive(Debug)]
struct DirTree {
    file_name: OsString,
    children: Option<Vec<DirTree>>,
}

impl DirTree {
    fn read_recursive<P: AsRef<Path>>(path: P) -> io::Result<Vec<Self>> {
        fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                let path = entry.path();

                let mut metadata = entry.metadata()?;
                if !metadata.is_file() && !metadata.is_dir() {
                    // Entry is a symlink
                    metadata = fs::metadata(&path)?;
                }

                let children = if metadata.is_file() {
                    None
                } else {
                    Some(Self::read_recursive(&path)?)
                };

                Ok(DirTree {
                    file_name: entry.file_name(),
                    children,
                })
            })
            .collect()
    }
}

impl fmt::Display for DirTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn write_tree(f: &mut fmt::Formatter<'_>, node: &DirTree, depth: usize) -> fmt::Result {
            let indentation = "  ".repeat(depth);

            if depth > 0 {
                writeln!(f)?;
            }

            write!(f, "{}{}", indentation, node.file_name.to_string_lossy())?;

            if let Some(ref children) = node.children {
                write!(f, "/")?;
                for child in children {
                    write_tree(f, child, depth + 1)?;
                }
            }

            Ok(())
        };

        write_tree(f, self, 0)
    }
}

fn main() {
    loop {
        match DirTree::read_recursive("/var/log") {
            Ok(entries) => println!(
                "{}",
                DirTree {
                    file_name: "/var/log".into(),
                    children: Some(entries),
                }
            ),
            Err(error) => {
                eprintln!("Warning: Failed to read /var/log due to: {:?}", error);
                eprintln!("Warning: Will try again in 30s");
            }
        }
        thread::sleep(Duration::from_secs(30));
    }
}
