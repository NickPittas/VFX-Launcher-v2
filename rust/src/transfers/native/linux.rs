//! Linux handle-relative primitives behind the shared transfer executor.
use super::{
    EntryIdentity, EntryInfo, EntryKind, NamespaceDurability, RemovalOutcome, require_same_entry,
};
use rustix::{
    fd::{BorrowedFd, OwnedFd},
    fs::{self, AtFlags, FileType, Mode, OFlags, RenameFlags, ResolveFlags, Stat, Timestamps},
    io::Errno,
    time::Timespec,
};
use std::{
    ffi::OsStr,
    fs::{File, Metadata},
    io,
    path::{Component, Path},
};

pub(crate) use rustix::fd::AsFd as AsDirectory;
pub(crate) type Directory = OwnedFd;

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

fn info(stat: Stat) -> io::Result<EntryInfo> {
    let kind = match FileType::from_raw_mode(stat.st_mode) {
        FileType::RegularFile => EntryKind::Regular,
        FileType::Directory => EntryKind::Directory,
        _ => EntryKind::Other,
    };
    Ok(EntryInfo {
        identity: EntryIdentity {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
            kind,
        },
        size: u64::try_from(stat.st_size).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "file size cannot be represented",
            )
        })?,
        modified_ns: i64::try_from(
            i128::from(stat.st_mtime) * 1_000_000_000 + i128::from(stat.st_mtime_nsec),
        )
        .ok(),
    })
}

pub(crate) fn handle_info(handle: &impl AsDirectory) -> io::Result<EntryInfo> {
    info(fs::fstat(handle)?)
}

pub(crate) fn preserve_metadata(target: &File, source: &Metadata) -> Vec<String> {
    use std::os::unix::fs::MetadataExt;
    let mut warnings = Vec::new();
    if let Err(error) = fs::fchmod(target, Mode::from_raw_mode(source.mode())) {
        warnings.push(format!(
            "copied, but the source permissions could not be preserved: {error}"
        ));
    }
    let times = Timestamps {
        last_access: Timespec {
            tv_sec: source.atime(),
            tv_nsec: source.atime_nsec(),
        },
        last_modification: Timespec {
            tv_sec: source.mtime(),
            tv_nsec: source.mtime_nsec(),
        },
    };
    if let Err(error) = fs::futimens(target, &times) {
        warnings.push(format!(
            "copied, but the source modification time could not be preserved: {error}"
        ));
    }
    warnings
}

pub(crate) fn no_clobber_unavailable(error: &io::Error) -> bool {
    matches!(
        Errno::from_io_error(error),
        Some(Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP)
    )
}

/// The selected root may be a mount; descendants cannot cross mounts or links.
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
    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            fd: self.duplicate()?,
        })
    }
    pub(crate) fn duplicate(&self) -> io::Result<Directory> {
        rustix::io::dup(&self.fd).map_err(Into::into)
    }
    pub(crate) fn open_dir(&self, path: &OsStr) -> io::Result<Directory> {
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
        parent: &impl AsDirectory,
        name: &OsStr,
        write: bool,
    ) -> io::Result<(File, EntryIdentity)> {
        basename(name)?;
        let fd = fs::openat2(
            parent,
            name,
            (if write { OFlags::RDWR } else { OFlags::RDONLY })
                | OFlags::CLOEXEC
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK,
            Mode::empty(),
            RESOLVE,
        )?;
        let observed = handle_info(&fd)?;
        if observed.identity.kind != EntryKind::Regular {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok((File::from(fd), observed.identity))
    }
    pub(crate) fn entry_identity(
        &self,
        parent: &impl AsDirectory,
        name: &OsStr,
    ) -> io::Result<EntryIdentity> {
        Ok(self.entry_stat(parent, name)?.identity)
    }
    pub(crate) fn entry_stat(
        &self,
        parent: &impl AsDirectory,
        name: &OsStr,
    ) -> io::Result<EntryInfo> {
        basename(name)?;
        info(fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?)
    }
    pub(crate) fn create_staging(
        &self,
        parent: &impl AsDirectory,
        name: &OsStr,
    ) -> io::Result<Staging> {
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
    pub(crate) fn mkdir(&self, parent: &impl AsDirectory, name: &OsStr) -> io::Result<Directory> {
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
        parent: &impl AsDirectory,
        file: &File,
        stage: &OsStr,
        dest: &OsStr,
        expected: EntryIdentity,
    ) -> io::Result<()> {
        basename(stage)?;
        basename(dest)?;
        require_same_entry(handle_info(file)?.identity, expected)?;
        require_same_entry(self.entry_identity(parent, stage)?, expected)?;
        fs::renameat_with(parent, stage, parent, dest, RenameFlags::NOREPLACE).map_err(Into::into)
    }
    pub(crate) fn rename_between(
        &self,
        from_parent: &impl AsDirectory,
        from_name: &OsStr,
        to_parent: &impl AsDirectory,
        to_name: &OsStr,
        expected: EntryIdentity,
    ) -> io::Result<()> {
        basename(from_name)?;
        basename(to_name)?;
        require_same_entry(self.entry_identity(from_parent, from_name)?, expected)?;
        fs::renameat_with(
            from_parent,
            from_name,
            to_parent,
            to_name,
            RenameFlags::NOREPLACE,
        )
        .map_err(Into::into)
    }
    /// Fresh identity/type proof precedes unlink; comparison and unlink are not atomic.
    pub(crate) fn remove_owned(
        &self,
        parent: &impl AsDirectory,
        name: &OsStr,
        expected: EntryIdentity,
    ) -> io::Result<RemovalOutcome> {
        require_same_entry(self.entry_identity(parent, name)?, expected)?;
        fs::unlinkat(parent, name, AtFlags::empty())?;
        Ok(RemovalOutcome::Removed)
    }
    pub(crate) fn sync_dir(&self, dir: &impl AsDirectory) -> io::Result<NamespaceDurability> {
        fs::fsync(dir)?;
        Ok(NamespaceDurability::Confirmed)
    }
}

impl AsDirectory for Root {
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
