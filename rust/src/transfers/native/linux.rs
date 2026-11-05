//! Linux handle-relative filesystem primitives for the transfer executor.
use rustix::{
    fd::{AsFd, BorrowedFd, OwnedFd},
    fs::{self, AtFlags, FileType, Mode, OFlags, RenameFlags, ResolveFlags, Stat},
};
use std::{
    ffi::OsStr,
    fs::File,
    io,
    path::{Component, Path},
};
const RESOLVE: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
fn basename(name: &OsStr) -> io::Result<()> {
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected one native entry name",
        ));
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntryIdentity {
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
}
impl From<Stat> for EntryIdentity {
    fn from(s: Stat) -> Self {
        Self {
            device: s.st_dev as u64,
            inode: s.st_ino as u64,
            mode: s.st_mode as u32,
        }
    }
}

impl From<&std::fs::Metadata> for EntryIdentity {
    fn from(meta: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: meta.dev(),
            inode: meta.ino(),
            mode: meta.mode(),
        }
    }
}
/// Selected root may itself be a mount; descendants cannot cross mounts.
pub(crate) struct Root {
    fd: OwnedFd,
}
impl Root {
    pub(crate) fn open(path: &OsStr) -> io::Result<Self> {
        Ok(Self {
            fd: fs::open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )?,
        })
    }
    /// Second handle on the selected root, for callers that need "the root as a
    /// directory" without resolving an empty relative path.
    pub(crate) fn duplicate(&self) -> io::Result<OwnedFd> {
        rustix::io::dup(&self.fd).map_err(Into::into)
    }

    pub(crate) fn open_dir(&self, path: &OsStr) -> io::Result<OwnedFd> {
        fs::openat2(
            &self.fd,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE,
        )
        .map_err(Into::into)
    }
    pub(crate) fn open_regular(
        &self,
        parent: &impl AsFd,
        name: &OsStr,
        write: bool,
    ) -> io::Result<(File, EntryIdentity)> {
        basename(name)?;
        let f = fs::openat2(
            parent,
            name,
            (if write { OFlags::RDWR } else { OFlags::RDONLY })
                | OFlags::CLOEXEC
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK,
            Mode::empty(),
            RESOLVE,
        )?;
        let s = fs::fstat(&f)?;
        if FileType::from_raw_mode(s.st_mode) != FileType::RegularFile {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        let id = s.into();
        Ok((File::from(f), id))
    }
    pub(crate) fn entry_identity(
        &self,
        parent: &impl AsFd,
        name: &OsStr,
    ) -> io::Result<EntryIdentity> {
        basename(name)?;
        Ok(fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?.into())
    }

    /// One no-follow stat of a descendant entry, for immediate revalidation of a
    /// reviewed destination without opening (and therefore without needing read
    /// permission on) the file itself.
    pub(crate) fn entry_stat(&self, parent: &impl AsFd, name: &OsStr) -> io::Result<Stat> {
        basename(name)?;
        fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(Into::into)
    }
    pub(crate) fn create_staging(&self, parent: &impl AsFd, name: &OsStr) -> io::Result<Staging> {
        basename(name)?;
        fs::openat2(
            parent,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
            RESOLVE,
        )
        .map(|fd| Staging { fd })
        .map_err(Into::into)
    }
    pub(crate) fn mkdir(&self, parent: &impl AsFd, name: &OsStr) -> io::Result<OwnedFd> {
        basename(name)?;
        fs::mkdirat(parent, name, Mode::RWXU)?;
        fs::openat2(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
            RESOLVE,
        )
        .map_err(Into::into)
    }
    pub(crate) fn publish_noreplace(
        &self,
        parent: &impl AsFd,
        stage: &OsStr,
        dest: &OsStr,
    ) -> io::Result<()> {
        basename(stage)?;
        basename(dest)?;
        fs::renameat_with(parent, stage, parent, dest, RenameFlags::NOREPLACE).map_err(Into::into)
    }
    /// No-clobber rename between two directory handles: used for the same-filesystem
    /// Move fast path and for the displacement/restore window of an existing
    /// destination. The destination name is never replaced.
    pub(crate) fn rename_between(
        &self,
        from_parent: &impl AsFd,
        from_name: &OsStr,
        to_parent: &impl AsFd,
        to_name: &OsStr,
    ) -> io::Result<()> {
        basename(from_name)?;
        basename(to_name)?;
        fs::renameat_with(
            from_parent,
            from_name,
            to_parent,
            to_name,
            RenameFlags::NOREPLACE,
        )
        .map_err(Into::into)
    }

    /// Fresh identity check precedes unlink; name-based unlink cannot make the comparison atomic.
    pub(crate) fn remove_owned(
        &self,
        parent: &impl AsFd,
        name: &OsStr,
        id: EntryIdentity,
    ) -> io::Result<()> {
        let fresh = self.entry_identity(parent, name)?;
        if fresh.device != id.device
            || fresh.inode != id.inode
            || FileType::from_raw_mode(fresh.mode) != FileType::from_raw_mode(id.mode)
        {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "entry identity changed",
            ));
        }
        fs::unlinkat(parent, name, AtFlags::empty()).map_err(Into::into)
    }
    pub(crate) fn sync_dir(&self, dir: &impl AsFd) -> io::Result<()> {
        fs::fsync(dir).map_err(Into::into)
    }
}
impl AsFd for Root {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

pub(crate) struct Staging {
    fd: OwnedFd,
}
impl Staging {
    pub(crate) fn into_file(self) -> File {
        File::from(self.fd)
    }
}
