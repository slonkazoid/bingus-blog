use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

// i will kill you rust stdlib
pub trait Append<T>
where
    Self: Into<OsString>,
    T: From<OsString>,
{
    fn append(self, ext: impl AsRef<OsStr>) -> T {
        let mut buffer: OsString = self.into();
        buffer.push(ext.as_ref());
        T::from(buffer)
    }
}

impl Append<PathBuf> for PathBuf {}
impl Append<PathBuf> for &Path {}
