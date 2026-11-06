//! Windows operations relative to held directory handles; descendant reparses fail closed.
use super::{
    EntryIdentity, EntryInfo, EntryKind, NamespaceDurability, RemovalOutcome, require_same_entry,
};
use std::{
    ffi::OsStr,
    fs::{File, Metadata, OpenOptions},
    io,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsHandle, AsRawHandle, FromRawHandle},
    },
    path::{Component, Path},
    ptr,
};
use windows_sys::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FILE_RENAME_INFO, GetFileInformationByHandle,
        GetFileType, SetFileInformationByHandle,
    },
};

pub(crate) use std::os::windows::io::AsHandle as AsDirectory;
pub(crate) type Directory = File;
pub(crate) struct Staging(File);
impl Staging {
    pub(crate) fn into_file(self) -> File {
        self.0
    }
}

const READ_ATTRIBUTES: u32 = 0x80;
const WRITE_ATTRIBUTES: u32 = 0x100;
const DELETE: u32 = 0x10000;
const SYNCHRONIZE: u32 = 0x100000;
const GENERIC_READ: u32 = 0x80000000;
const GENERIC_WRITE: u32 = 0x40000000;
const DIRECTORY: u32 = 0x10;
const REPARSE: u32 = 0x400;
const FILE_OPEN: u32 = 1;
const FILE_CREATE: u32 = 2;
const DIRECTORY_FILE: u32 = 1;
const NON_DIRECTORY_FILE: u32 = 0x40;
const SHARE_ALL: u32 = 7;

#[repr(C)]
struct UnicodeString {
    len: u16,
    max: u16,
    buffer: *mut u16,
}
#[repr(C)]
struct ObjectAttributes {
    len: u32,
    root: HANDLE,
    name: *mut UnicodeString,
    attrs: u32,
    security: *mut std::ffi::c_void,
    qos: *mut std::ffi::c_void,
}
#[repr(C)]
union IoStatusValue {
    status: i32,
    pointer: *mut std::ffi::c_void,
}
#[repr(C)]
struct IoStatusBlock {
    value: IoStatusValue,
    information: usize,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        handle: *mut HANDLE,
        access: u32,
        attrs: *const ObjectAttributes,
        status: *mut IoStatusBlock,
        alloc: *const i64,
        file_attrs: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const std::ffi::c_void,
        ea_len: u32,
    ) -> i32;
    fn NtSetInformationFile(
        handle: HANDLE,
        status: *mut IoStatusBlock,
        information: *const std::ffi::c_void,
        length: u32,
        class: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}
fn raw(h: &impl AsDirectory) -> HANDLE {
    h.as_handle().as_raw_handle() as HANDLE
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn basename(name: &OsStr) -> io::Result<Vec<u16>> {
    let v: Vec<_> = name.encode_wide().collect();
    // Only ASCII is interpreted; unpaired native UTF-16 units are preserved.
    let ascii: String = v
        .iter()
        .take_while(|c| **c != 46)
        .map(|c| char::from_u32(u32::from(*c)).unwrap_or('\u{FFFD}'))
        .collect::<String>()
        .to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&ascii.as_str())
        || (ascii.len() == 4
            && (ascii.starts_with("COM") || ascii.starts_with("LPT"))
            && matches!(ascii.as_bytes()[3], b'1'..=b'9'));
    if v.is_empty()
        || v.len() > 255
        || v == [46]
        || v == [46, 46]
        || reserved
        || v.iter()
            .any(|c| *c < 32 || [47, 92, 58, 60, 62, 34, 124, 63, 42].contains(c))
        || matches!(v.last(), Some(32 | 46))
    {
        return Err(invalid("invalid Windows entry basename"));
    }
    Ok(v)
}
fn open_child(
    parent: &impl AsDirectory,
    name: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
    attrs: u32,
) -> io::Result<File> {
    let parent_info = handle_info(parent)?;
    if parent_info.identity.kind != EntryKind::Directory {
        return Err(invalid("parent is not a directory"));
    }
    let mut n = basename(name)?;
    let mut us = UnicodeString {
        len: (n.len() * 2) as u16,
        max: (n.len() * 2) as u16,
        buffer: n.as_mut_ptr(),
    };
    let oa = ObjectAttributes {
        len: std::mem::size_of::<ObjectAttributes>() as u32,
        root: raw(parent),
        name: &mut us,
        attrs: 0x40,
        security: ptr::null_mut(),
        qos: ptr::null_mut(),
    };
    let mut handle = ptr::null_mut();
    let mut ios = IoStatusBlock {
        value: IoStatusValue { status: 0 },
        information: 0,
    };
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &oa,
            &mut ios,
            ptr::null(),
            attrs,
            SHARE_ALL,
            disposition,
            options | 0x20 | 0x00200000,
            ptr::null(),
            0,
        )
    };
    if status < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    let file = unsafe { File::from_raw_handle(handle as _) };
    let child = handle_info(&file)?;
    if child.identity.kind == EntryKind::Other
        || child.identity.device != parent_info.identity.device
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "reparse, special entry or cross-volume descendant refused",
        ));
    }
    Ok(file)
}
pub(crate) fn handle_info(handle: &impl AsDirectory) -> io::Result<EntryInfo> {
    if unsafe { GetFileType(raw(handle)) } != 1 {
        return Err(invalid("handle is not a disk file or directory"));
    }
    let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    if unsafe { GetFileInformationByHandle(raw(handle), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let kind = if info.dwFileAttributes & REPARSE != 0 {
        EntryKind::Other
    } else if info.dwFileAttributes & DIRECTORY != 0 {
        EntryKind::Directory
    } else {
        EntryKind::Regular
    };
    let inode = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    if inode == 0 {
        return Err(io::Error::other(
            "filesystem did not provide a usable file identity",
        ));
    }
    let ticks = (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
        | u64::from(info.ftLastWriteTime.dwLowDateTime);
    Ok(EntryInfo {
        identity: EntryIdentity {
            device: u64::from(info.dwVolumeSerialNumber),
            inode,
            kind,
        },
        size: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
        modified_ns: i64::try_from((i128::from(ticks) - 116444736000000000) * 100).ok(),
    })
}
pub(crate) struct Root {
    file: File,
}
impl Root {
    pub(crate) fn open(path: &OsStr) -> io::Result<Self> {
        if !Path::new(path).is_absolute() {
            return Err(invalid("selected root must be absolute"));
        }
        // The selected root may resolve a user-selected link; descendants never do.
        let file = OpenOptions::new()
            .access_mode(READ_ATTRIBUTES | 1 | 0x20 | SYNCHRONIZE)
            .share_mode(SHARE_ALL)
            .custom_flags(0x02000000)
            .open(path)?;
        if handle_info(&file)?.identity.kind != EntryKind::Directory {
            return Err(invalid("selected root is not a directory"));
        }
        Ok(Self { file })
    }
    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
        })
    }
    pub(crate) fn duplicate(&self) -> io::Result<Directory> {
        self.file.try_clone()
    }
    pub(crate) fn open_dir(&self, path: &OsStr) -> io::Result<Directory> {
        let mut current = self.duplicate()?;
        for part in Path::new(path).components() {
            let Component::Normal(name) = part else {
                return Err(invalid("expected a relative directory path"));
            };
            current = open_child(
                &current,
                name,
                READ_ATTRIBUTES | 1 | 0x20,
                FILE_OPEN,
                DIRECTORY_FILE,
                0,
            )?;
            if handle_info(&current)?.identity.kind != EntryKind::Directory {
                return Err(invalid("entry is not a directory"));
            }
        }
        Ok(current)
    }
    pub(crate) fn open_regular(
        &self,
        parent: &impl AsDirectory,
        name: &OsStr,
        write: bool,
    ) -> io::Result<(File, EntryIdentity)> {
        let file = open_child(
            parent,
            name,
            READ_ATTRIBUTES | if write { GENERIC_WRITE } else { GENERIC_READ },
            FILE_OPEN,
            NON_DIRECTORY_FILE,
            0,
        )?;
        let identity = handle_info(&file)?.identity;
        if identity.kind != EntryKind::Regular {
            return Err(invalid("entry is not a regular file"));
        }
        Ok((file, identity))
    }
    pub(crate) fn entry_stat(
        &self,
        parent: &impl AsDirectory,
        name: &OsStr,
    ) -> io::Result<EntryInfo> {
        handle_info(&open_child(parent, name, READ_ATTRIBUTES, FILE_OPEN, 0, 0)?)
    }
    pub(crate) fn entry_identity(
        &self,
        parent: &impl AsDirectory,
        name: &OsStr,
    ) -> io::Result<EntryIdentity> {
        Ok(self.entry_stat(parent, name)?.identity)
    }
    pub(crate) fn create_staging(
        &self,
        parent: &impl AsDirectory,
        name: &OsStr,
    ) -> io::Result<Staging> {
        Ok(Staging(open_child(
            parent,
            name,
            GENERIC_WRITE | READ_ATTRIBUTES | WRITE_ATTRIBUTES | DELETE,
            FILE_CREATE,
            NON_DIRECTORY_FILE,
            0x80,
        )?))
    }
    pub(crate) fn mkdir(&self, parent: &impl AsDirectory, name: &OsStr) -> io::Result<Directory> {
        open_child(
            parent,
            name,
            READ_ATTRIBUTES | 1 | 0x20,
            FILE_CREATE,
            DIRECTORY_FILE,
            DIRECTORY,
        )
    }
    pub(crate) fn publish_noreplace(
        &self,
        parent: &impl AsDirectory,
        file: &File,
        stage: &OsStr,
        destination: &OsStr,
        expected: EntryIdentity,
    ) -> io::Result<()> {
        require_same_entry(handle_info(file)?.identity, expected)?;
        require_same_entry(self.entry_identity(parent, stage)?, expected)?;
        rename(file, parent, destination)
    }
    pub(crate) fn rename_between(
        &self,
        from: &impl AsDirectory,
        source: &OsStr,
        to: &impl AsDirectory,
        destination: &OsStr,
        expected: EntryIdentity,
    ) -> io::Result<()> {
        let file = open_child(
            from,
            source,
            DELETE | READ_ATTRIBUTES,
            FILE_OPEN,
            NON_DIRECTORY_FILE,
            0,
        )?;
        require_same_entry(handle_info(&file)?.identity, expected)?;
        rename(&file, to, destination)
    }
    pub(crate) fn remove_owned(
        &self,
        parent: &impl AsDirectory,
        name: &OsStr,
        expected: EntryIdentity,
    ) -> io::Result<RemovalOutcome> {
        let file = open_child(
            parent,
            name,
            DELETE | READ_ATTRIBUTES,
            FILE_OPEN,
            NON_DIRECTORY_FILE,
            0,
        )?;
        require_same_entry(handle_info(&file)?.identity, expected)?;
        let mut flags = 1u32 | 2u32; // DELETE | POSIX_SEMANTICS, without clearing read-only attributes.
        let ok = unsafe {
            SetFileInformationByHandle(raw(&file), 21, &mut flags as *mut _ as *mut _, 4)
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if !matches!(error.raw_os_error(), Some(1 | 50 | 87)) {
                return Err(error);
            }
            let mut delete = 1u8;
            if unsafe {
                SetFileInformationByHandle(raw(&file), 4, &mut delete as *mut _ as *mut _, 1)
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        drop(file);
        match self.entry_stat(parent, name) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(RemovalOutcome::Removed),
            Err(e) if e.raw_os_error() == Some(303) => Ok(RemovalOutcome::Pending),
            Err(e) => Err(e),
            Ok(_) => Ok(RemovalOutcome::Pending),
        }
    }
    pub(crate) fn sync_dir(&self, directory: &impl AsDirectory) -> io::Result<NamespaceDurability> {
        if handle_info(directory)?.identity.kind != EntryKind::Directory {
            return Err(invalid("namespace barrier requires a directory"));
        }
        // Windows completion semantics: flushed files and completed rename, no power-loss namespace guarantee.
        Ok(NamespaceDurability::Unconfirmed)
    }
}
impl AsHandle for Root {
    fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        self.file.as_handle()
    }
}
fn rename(file: &File, parent: &impl AsDirectory, name: &OsStr) -> io::Result<()> {
    if handle_info(parent)?.identity.kind != EntryKind::Directory {
        return Err(invalid("rename target parent is not a directory"));
    }
    let name = basename(name)?;
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let size = std::mem::size_of::<FILE_RENAME_INFO>() + name.len() * 2;
    let mut aligned = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
    let info = aligned.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        // Zeroed union means ReplaceIfExists = FALSE; never fall back to a replacing path rename.
        (*info).RootDirectory = raw(parent);
        (*info).FileNameLength = (name.len() * 2) as u32;
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            info.cast::<u8>().add(offset).cast::<u16>(),
            name.len(),
        );
        // The Win32 wrapper rejects a non-null RootDirectory on supported hosts.
        // Use the native handle-relative operation, never a path-based fallback.
        let mut ios = IoStatusBlock {
            value: IoStatusValue { status: 0 },
            information: 0,
        };
        let status = NtSetInformationFile(raw(file), &mut ios, info.cast(), size as u32, 10);
        if status < 0 {
            return Err(io::Error::from_raw_os_error(
                RtlNtStatusToDosError(status) as i32
            ));
        }
    }
    Ok(())
}
pub(crate) fn preserve_metadata(target: &File, source: &Metadata) -> Vec<String> {
    let mut warnings = Vec::new();
    let supported = 0x1 | 0x2 | 0x4 | 0x20 | 0x2000; // read-only, hidden, system, archive, not-content-indexed.
    let attributes = source.file_attributes() & supported;
    let mut info = FILE_BASIC_INFO {
        CreationTime: 0,
        LastAccessTime: source.last_access_time() as i64,
        LastWriteTime: source.last_write_time() as i64,
        ChangeTime: 0,
        FileAttributes: if attributes == 0 { 0x80 } else { attributes },
    };
    if unsafe {
        SetFileInformationByHandle(
            raw(target),
            0,
            &mut info as *mut _ as *mut _,
            std::mem::size_of_val(&info) as u32,
        )
    } == 0
    {
        warnings.push(format!(
            "Could not preserve Windows times and attributes: {}",
            io::Error::last_os_error()
        ));
    }
    warnings.push(
        "Windows permissions, alternate data streams and unsupported attributes are not copied"
            .into(),
    );
    warnings
}
pub(crate) fn no_clobber_unavailable(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(1 | 50 | 87))
}
