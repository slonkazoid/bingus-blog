// TODO: make this bearable

use std::{
    fs::{self, Metadata},
    io::{self, Result},
    path::Path,
    process::{Child, Command},
    sync::Mutex,
};

fn compress_file(path: &Path, metadata: Metadata, handles: &Mutex<Vec<Child>>) -> Result<()> {
    let compressed_file = format!("{}.gz", path.to_str().unwrap());
    if match fs::metadata(compressed_file) {
        Ok(existing_metadata) => metadata.modified()? > existing_metadata.modified()?,
        Err(err) => match err.kind() {
            io::ErrorKind::NotFound => true,
            _ => return Err(err),
        },
    } {
        let mut handles_guard = handles.lock().unwrap();
        handles_guard.push(Command::new("gzip").arg("-kf5").arg(path).spawn()?);
    }
    Ok(())
}

fn compress_recursively(path: &Path, handles: &Mutex<Vec<Child>>) -> Result<()> {
    let metadata = fs::metadata(path)?;

    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            compress_recursively(&entry?.path(), handles)?
        }
        Ok(())
    } else if match path.extension() {
        Some(ext) => ext == "gz",
        None => false,
    } || metadata.is_symlink()
    {
        Ok(())
    } else {
        compress_file(path, metadata, handles)
    }
}

pub fn compress_epicly<P: AsRef<Path>>(path: P) -> Result<u64> {
    let mut i = 0;

    let handles = Mutex::new(Vec::new());

    compress_recursively(AsRef::<Path>::as_ref(&path), &handles)?;

    let handles = handles.into_inner().unwrap();

    for mut handle in handles {
        assert!(handle.wait().unwrap().success());
        i += 1;
    }

    Ok(i)
}
