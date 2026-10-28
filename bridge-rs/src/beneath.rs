//! Files under a folder a sandbox writes in, reached only through folder descriptors (security audit 2026-10-27,
//! H1). A sandboxed agent can swap any folder of its shared folder, its `/envoi` or a linked volume for a symbolic
//! link at any moment; a path checked once and then used again by its name lets that link lead the bridge (this
//! account) to the person's own files: read them, write them, change their modes. So every name here is resolved from
//! an open folder, one name at a time, never through a link:
//!
//! - **opening**: Linux's `openat2` with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS` (one call),
//!   else (an older kernel, macOS, a seccomp filter) `openat(O_NOFOLLOW)` name by name;
//! - **making folders**: `mkdirat`, then the new folder opened without following a link;
//! - **writing**: a temporary file made with `openat(O_CREAT | O_EXCL | O_NOFOLLOW)` in the folder's descriptor, then
//!   `renameat` in that same folder;
//! - **modes**: `fchmod` on a descriptor; a name's mode through a descriptor of it opened without following a link
//!   (Linux: `O_PATH`, then its `/proc/self/fd` entry, as glibc's `fchmodat(AT_SYMLINK_NOFOLLOW)` does);
//! - **removing**: `unlinkat` in the folder's descriptor, a folder emptied the same way, never followed.
//!
//! The root itself is a path the bridge chose (a volume's mountpoint, a folder of its data): resolved normally. What
//! a sandbox's user owns and this account can't open is done as the sandboxes' « root » (`podman unshare`) by this
//! same binary (`bipbridge fs-as-root …`, `as_root`), never by a shell command working on paths.
//!
//! Works on Linux and macOS (the per-name walk); `openat2` and `O_PATH` are Linux only.

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How deep a tree is followed at most (a folder nested deeper is an error, never a stack overflow).
pub const MAX_DEPTH: usize = 64;

/// What a name is, as `fstatat(AT_SYMLINK_NOFOLLOW)` sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Link,
    /// A pipe, a socket, a device.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub kind: Kind,
    /// Permission bits with setuid/setgid/sticky (`& 0o7777`).
    pub mode: u32,
    pub size: u64,
    pub uid: u32,
    pub mtime: i64,
    pub mtime_nsec: i64,
    pub dev: u64,
    pub ino: u64,
}

impl Stat {
    #[allow(clippy::unnecessary_cast)] // the field types differ between Linux and macOS
    fn of(st: &libc::stat) -> Stat {
        let format = st.st_mode as u32 & libc::S_IFMT as u32;
        let kind = if format == libc::S_IFDIR as u32 {
            Kind::Dir
        } else if format == libc::S_IFREG as u32 {
            Kind::File
        } else if format == libc::S_IFLNK as u32 {
            Kind::Link
        } else {
            Kind::Other
        };
        Stat {
            kind,
            mode: st.st_mode as u32 & 0o7777,
            size: st.st_size as u64,
            uid: st.st_uid,
            mtime: st.st_mtime as i64,
            mtime_nsec: st.st_mtime_nsec as i64,
            dev: st.st_dev as u64,
            ino: st.st_ino as u64,
        }
    }

    /// Its modification time.
    pub fn modified(&self) -> SystemTime {
        let nanos = self.mtime_nsec.clamp(0, 999_999_999) as u32;
        if self.mtime >= 0 {
            UNIX_EPOCH + Duration::new(self.mtime as u64, nanos)
        } else {
            UNIX_EPOCH - Duration::from_secs(self.mtime.unsigned_abs())
        }
    }
}

fn cvt(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// One plain name: not empty, no `/`, not `.` nor `..`.
fn name_of(name: &OsStr) -> io::Result<CString> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a plain name"));
    }
    CString::new(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "not a plain name"))
}

/// A relative path's names, each plain (no root, no `..`; `.` dropped).
fn names_of(relative: &Path) -> io::Result<Vec<OsString>> {
    let mut out = Vec::new();
    for part in relative.components() {
        match part {
            Component::Normal(name) => out.push(name.to_os_string()),
            Component::CurDir => {}
            _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a plain relative path")),
        }
    }
    Ok(out)
}

/// Whether an error means « a link (or something else than expected) was there »: the walk refused it.
pub fn refused(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENOTDIR) | Some(libc::EXDEV))
        || err.kind() == io::ErrorKind::InvalidInput
}

const DIR_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// An open folder: every name below it is reached through it.
#[derive(Debug)]
pub struct Dir {
    fd: OwnedFd,
}

impl Dir {
    /// A folder by its path (a root the bridge chose: resolved normally).
    pub fn open(path: &Path) -> io::Result<Dir> {
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::other("a NUL in the path"))?;
        // SAFETY: a NUL-terminated path; the descriptor is owned right after.
        let fd = cvt(unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) })?;
        // SAFETY: just opened, owned by nobody else.
        Ok(Dir { fd: unsafe { OwnedFd::from_raw_fd(fd) } })
    }

    /// A folder by its path, made first (with its parents) when missing.
    pub fn open_or_make(path: &Path) -> io::Result<Dir> {
        match Dir::open(path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                std::fs::create_dir_all(path)?;
                Dir::open(path)
            }
            other => other,
        }
    }

    /// A folder by its path when that path still leads to it as it was checked: `path` is a real path (no link on
    /// the way, `canonicalize`d when it was checked) and the folder opened has that same real path. A folder on the
    /// way that a sandbox swapped for a link since is refused (ELOOP), never followed elsewhere.
    pub fn open_exact(path: &Path) -> io::Result<Dir> {
        let dir = Dir::open(path)?;
        if dir.real_path()? != path {
            return Err(io::Error::from_raw_os_error(libc::ELOOP));
        }
        Ok(dir)
    }

    /// Where this folder is now, as the system resolves it (Linux `/proc/self/fd`, macOS `F_GETPATH`).
    pub fn real_path(&self) -> io::Result<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_link(format!("/proc/self/fd/{}", self.raw()))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let mut buf = vec![0u8; libc::PATH_MAX as usize + 1];
            // SAFETY: a valid descriptor and a buffer of at least PATH_MAX bytes, as F_GETPATH requires.
            cvt(unsafe { libc::fcntl(self.raw(), libc::F_GETPATH, buf.as_mut_ptr()) })?;
            let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            buf.truncate(end);
            Ok(PathBuf::from(OsString::from_vec(buf)))
        }
    }

    fn raw(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Another descriptor of the same folder.
    pub fn try_clone(&self) -> io::Result<Dir> {
        Ok(Dir { fd: self.fd.try_clone()? })
    }

    /// `relative` (names only) opened under this folder with `flags`, never through a link nor out of it.
    fn open_beneath(&self, relative: &Path, flags: libc::c_int) -> io::Result<OwnedFd> {
        let names = names_of(relative)?;
        if names.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "an empty path"));
        }
        #[cfg(target_os = "linux")]
        {
            let joined: PathBuf = names.iter().collect();
            if let Some(found) = openat2(self.raw(), &joined, flags) {
                return found;
            }
        }
        let mut current: Option<OwnedFd> = None;
        for (i, name) in names.iter().enumerate() {
            let c = name_of(name)?;
            let last = i + 1 == names.len();
            let wanted = if last { flags | libc::O_NOFOLLOW | libc::O_CLOEXEC } else { DIR_FLAGS };
            let at = current.as_ref().map(|f| f.as_raw_fd()).unwrap_or(self.raw());
            // SAFETY: a valid folder descriptor and a NUL-terminated plain name; owned right after.
            let fd = cvt(unsafe { libc::openat(at, c.as_ptr(), wanted, 0o600 as libc::c_uint) })?;
            // SAFETY: just opened, owned by nobody else.
            current = Some(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        current.ok_or_else(|| io::Error::other("an empty path"))
    }

    /// The folder at `relative` under this one (itself for an empty path).
    pub fn sub(&self, relative: &Path) -> io::Result<Dir> {
        if names_of(relative)?.is_empty() {
            return self.try_clone();
        }
        Ok(Dir { fd: self.open_beneath(relative, libc::O_RDONLY | libc::O_DIRECTORY)? })
    }

    /// A regular file under this folder, opened for reading (never a link, a pipe or a device).
    pub fn open_file(&self, relative: &Path) -> io::Result<File> {
        let fd = self.open_beneath(relative, libc::O_RDONLY | libc::O_NONBLOCK)?;
        let file = File::from(fd);
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
        }
        Ok(file)
    }

    /// The folder at `relative`, each missing folder on the way made (`mkdirat`); `mode`: set on each folder on the
    /// way that this account owns (`fchmod`), this folder itself left as it is.
    pub fn make_dirs(&self, relative: &Path, mode: Option<u32>) -> io::Result<Dir> {
        let mut current = self.try_clone()?;
        for name in names_of(relative)? {
            let next = match current.sub_one(&name) {
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    match current.mkdir(&name, mode.unwrap_or(0o755)) {
                        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                        other => other?,
                    }
                    current.sub_one(&name)?
                }
                other => other?,
            };
            if let Some(mode) = mode {
                next.chmod_if_owned(mode);
            }
            current = next;
        }
        Ok(current)
    }

    /// The folder `name` right under this one.
    pub fn sub_one(&self, name: &OsStr) -> io::Result<Dir> {
        let c = name_of(name)?;
        // SAFETY: a valid folder descriptor and a NUL-terminated plain name; owned right after.
        let fd = cvt(unsafe { libc::openat(self.raw(), c.as_ptr(), DIR_FLAGS) })?;
        // SAFETY: just opened, owned by nobody else.
        Ok(Dir { fd: unsafe { OwnedFd::from_raw_fd(fd) } })
    }

    /// `relative` split into its folder (opened, never through a link) and its last name.
    pub fn parent_of(&self, relative: &Path) -> io::Result<(Dir, OsString)> {
        let mut names = names_of(relative)?;
        let Some(last) = names.pop() else { return Err(io::Error::new(io::ErrorKind::InvalidInput, "an empty path")) };
        let parent: PathBuf = names.iter().collect();
        Ok((self.sub(&parent)?, last))
    }

    /// This folder's own state.
    pub fn stat_self(&self) -> io::Result<Stat> {
        // SAFETY: an all-zero `stat` is valid; fstat fills it.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: a valid descriptor and a valid out pointer.
        cvt(unsafe { libc::fstat(self.raw(), &mut st) })?;
        Ok(Stat::of(&st))
    }

    /// What `name` is, the link itself for a link.
    pub fn stat(&self, name: &OsStr) -> io::Result<Stat> {
        let c = name_of(name)?;
        // SAFETY: an all-zero `stat` is valid; fstatat fills it.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: a valid folder descriptor, a NUL-terminated name and a valid out pointer.
        cvt(unsafe { libc::fstatat(self.raw(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) })?;
        Ok(Stat::of(&st))
    }

    /// The names in this folder with what each is (`.` and `..` left out, a name gone meanwhile too), by name.
    pub fn entries(&self) -> io::Result<BTreeMap<OsString, Stat>> {
        let fd = self.fd.try_clone()?;
        // SAFETY: fdopendir takes the descriptor over (closed by closedir below).
        let dir = unsafe { libc::fdopendir(fd.as_raw_fd()) };
        if dir.is_null() {
            return Err(io::Error::last_os_error());
        }
        // now the DIR's
        std::mem::forget(fd);
        // SAFETY: a valid DIR; rewound in case another descriptor of the same folder read it before.
        unsafe { libc::rewinddir(dir) };
        let mut names = Vec::new();
        loop {
            // SAFETY: a valid DIR; the entry is read before the next call.
            let entry = unsafe { libc::readdir(dir) };
            if entry.is_null() {
                break;
            }
            // SAFETY: d_name is NUL-terminated within the entry.
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
            let bytes = name.to_bytes();
            if bytes != b"." && bytes != b".." {
                names.push(OsString::from_vec(bytes.to_vec()));
            }
        }
        // SAFETY: a valid DIR, closed once.
        unsafe { libc::closedir(dir) };
        let mut out = BTreeMap::new();
        for name in names {
            match self.stat(&name) {
                Ok(st) => {
                    out.insert(name, st);
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        Ok(out)
    }

    /// A new folder `name` (its mode set after, past the umask).
    pub fn mkdir(&self, name: &OsStr, mode: u32) -> io::Result<()> {
        let c = name_of(name)?;
        // SAFETY: a valid folder descriptor and a NUL-terminated plain name.
        cvt(unsafe { libc::mkdirat(self.raw(), c.as_ptr(), 0o700) })?;
        self.sub_one(name)?.chmod(mode)
    }

    /// This folder's mode (`fchmod`).
    pub fn chmod(&self, mode: u32) -> io::Result<()> {
        // SAFETY: a valid descriptor.
        cvt(unsafe { libc::fchmod(self.raw(), mode as libc::mode_t) }).map(|_| ())
    }

    /// This folder's mode when this account owns it (else left alone: a sandbox's user's).
    pub fn chmod_if_owned(&self, mode: u32) {
        // SAFETY: getuid never fails.
        let uid = unsafe { libc::getuid() };
        if let Ok(st) = self.stat_self() {
            if st.uid == uid && st.mode != mode {
                let _ = self.chmod(mode);
            }
        }
    }

    /// `name`'s mode, `name` a folder or a regular file and never followed if it is a link: Err when it is
    /// something else (a link swapped in meanwhile is refused).
    pub fn chmod_at(&self, name: &OsStr, mode: u32) -> io::Result<()> {
        let c = name_of(name)?;
        #[cfg(target_os = "linux")]
        {
            // SAFETY: a valid folder descriptor and a NUL-terminated plain name; owned right after.
            let fd = unsafe { libc::openat(self.raw(), c.as_ptr(), libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
            if fd >= 0 {
                // SAFETY: just opened, owned by nobody else.
                let fd = unsafe { OwnedFd::from_raw_fd(fd) };
                // SAFETY: an all-zero `stat` is valid; fstat fills it (O_PATH descriptors allow it).
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                // SAFETY: a valid descriptor and a valid out pointer.
                cvt(unsafe { libc::fstat(fd.as_raw_fd(), &mut st) })?;
                if !matches!(Stat::of(&st).kind, Kind::Dir | Kind::File) {
                    return Err(io::Error::from_raw_os_error(libc::ELOOP));
                }
                let proc = CString::new(format!("/proc/self/fd/{}", fd.as_raw_fd())).map_err(io::Error::other)?;
                // SAFETY: a NUL-terminated path to this process's own descriptor (the inode itself).
                if unsafe { libc::chmod(proc.as_ptr(), mode as libc::mode_t) } == 0 {
                    return Ok(());
                }
                let err = io::Error::last_os_error();
                if err.kind() != io::ErrorKind::NotFound {
                    return Err(err); // else no /proc: by an ordinary descriptor below
                }
            }
        }
        // SAFETY: a valid folder descriptor and a NUL-terminated plain name; owned right after.
        let fd = cvt(unsafe {
            libc::openat(self.raw(), c.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        })?;
        // SAFETY: just opened, owned by nobody else.
        let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        let meta = file.metadata()?;
        if !meta.is_dir() && !meta.is_file() {
            return Err(io::Error::from_raw_os_error(libc::ELOOP));
        }
        // SAFETY: a valid descriptor.
        cvt(unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) }).map(|_| ())
    }

    /// A new file, empty, under a temporary name (`.bip-<random>.part`) no one else had: (its name, the file).
    pub fn create_temp(&self) -> io::Result<(OsString, File)> {
        for _ in 0..8 {
            let name = OsString::from(format!(".bip-{}.part", crate::py::token_urlsafe(9)));
            let c = name_of(&name)?;
            let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            // SAFETY: a valid folder descriptor and a NUL-terminated plain name; owned right after.
            let fd = unsafe { libc::openat(self.raw(), c.as_ptr(), flags, 0o600 as libc::c_uint) };
            if fd >= 0 {
                // SAFETY: just opened, owned by nobody else.
                return Ok((name, File::from(unsafe { OwnedFd::from_raw_fd(fd) })));
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::AlreadyExists {
                return Err(err);
            }
        }
        Err(io::Error::other("no free temporary name"))
    }

    /// `name` in this folder written whole: a temporary file filled, given `mode` (and `modified`), then renamed over
    /// `name` in this same folder. A folder named so stays (Err).
    pub fn write_file(
        &self,
        name: &OsStr,
        mode: u32,
        modified: Option<SystemTime>,
        fill: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> io::Result<()> {
        name_of(name)?;
        if self.stat(name).is_ok_and(|st| st.kind == Kind::Dir) {
            return Err(io::Error::other("a folder has this name"));
        }
        let (tmp, mut out) = self.create_temp()?;
        let result = (|| {
            fill(&mut out)?;
            out.flush()?;
            // SAFETY: a valid descriptor.
            cvt(unsafe { libc::fchmod(out.as_raw_fd(), mode as libc::mode_t) })?;
            if let Some(time) = modified {
                out.set_modified(time)?;
            }
            drop(out);
            self.rename(&tmp, self, name)
        })();
        if result.is_err() {
            let _ = self.unlink(&tmp);
        }
        result
    }

    /// `from` in this folder renamed `to` in `target` (`renameat`).
    pub fn rename(&self, from: &OsStr, target: &Dir, to: &OsStr) -> io::Result<()> {
        let (a, b) = (name_of(from)?, name_of(to)?);
        // SAFETY: valid folder descriptors and NUL-terminated plain names.
        cvt(unsafe { libc::renameat(self.raw(), a.as_ptr(), target.raw(), b.as_ptr()) }).map(|_| ())
    }

    /// `name` (a file or a link: the link itself) removed.
    pub fn unlink(&self, name: &OsStr) -> io::Result<()> {
        let c = name_of(name)?;
        // SAFETY: a valid folder descriptor and a NUL-terminated plain name.
        cvt(unsafe { libc::unlinkat(self.raw(), c.as_ptr(), 0) }).map(|_| ())
    }

    /// The empty folder `name` removed.
    pub fn rmdir(&self, name: &OsStr) -> io::Result<()> {
        let c = name_of(name)?;
        // SAFETY: a valid folder descriptor and a NUL-terminated plain name.
        cvt(unsafe { libc::unlinkat(self.raw(), c.as_ptr(), libc::AT_REMOVEDIR) }).map(|_| ())
    }

    /// `name` removed whatever it is, a folder with all it holds (each folder opened without following a link, each
    /// name removed in its folder's descriptor): the entries removed.
    pub fn remove(&self, name: &OsStr) -> io::Result<usize> {
        self.remove_at(name, 0)
    }

    fn remove_at(&self, name: &OsStr, depth: usize) -> io::Result<usize> {
        let st = self.stat(name)?;
        if st.kind != Kind::Dir {
            self.unlink(name)?;
            return Ok(1);
        }
        if depth > MAX_DEPTH {
            return Err(io::Error::other("folders nested too deep"));
        }
        let sub = self.sub_one(name)?;
        let mut removed = 0;
        for (child, _) in sub.entries()? {
            removed += sub.remove_at(&child, depth + 1)?;
        }
        self.rmdir(name)?;
        Ok(removed + 1)
    }

    /// Where the link `name` leads.
    pub fn read_link(&self, name: &OsStr) -> io::Result<PathBuf> {
        let c = name_of(name)?;
        let mut buf = vec![0u8; 4096];
        // SAFETY: a valid folder descriptor, a NUL-terminated name and a buffer of the given size.
        let n = unsafe { libc::readlinkat(self.raw(), c.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        buf.truncate(n as usize);
        Ok(PathBuf::from(OsString::from_vec(buf)))
    }

    /// A link `name` leading to `to`.
    pub fn symlink(&self, to: &Path, name: &OsStr) -> io::Result<()> {
        let c = name_of(name)?;
        let target = CString::new(to.as_os_str().as_bytes()).map_err(io::Error::other)?;
        // SAFETY: NUL-terminated strings and a valid folder descriptor.
        cvt(unsafe { libc::symlinkat(target.as_ptr(), self.raw(), c.as_ptr()) }).map(|_| ())
    }
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

/// `openat2` beneath `dir`, no link followed on the way: None when the kernel doesn't have it (or a filter refuses
/// it): the caller walks name by name.
#[cfg(target_os = "linux")]
fn openat2(dir: RawFd, relative: &Path, flags: libc::c_int) -> Option<io::Result<OwnedFd>> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static MISSING: AtomicBool = AtomicBool::new(false);
    if MISSING.load(Ordering::Relaxed) {
        return None;
    }
    let c = CString::new(relative.as_os_str().as_bytes()).ok()?;
    let how = OpenHow {
        flags: (flags | libc::O_NOFOLLOW | libc::O_CLOEXEC) as u64,
        mode: 0,
        resolve: libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS,
    };
    // SAFETY: a valid folder descriptor, a NUL-terminated path, and `how` of the size given.
    let fd = unsafe {
        libc::syscall(libc::SYS_openat2, dir, c.as_ptr(), &how as *const OpenHow, std::mem::size_of::<OpenHow>())
    };
    if fd < 0 {
        let err = io::Error::last_os_error();
        return match err.raw_os_error() {
            Some(libc::ENOSYS) | Some(libc::EPERM) | Some(libc::E2BIG) => {
                MISSING.store(true, Ordering::Relaxed);
                None
            }
            // a way out of the folder tried (EXDEV) or a retry asked (EAGAIN): the name-by-name walk decides, and it
            // never takes `..` nor a link either
            Some(libc::EAGAIN) | Some(libc::EXDEV) => None,
            _ => Some(Err(err)),
        };
    }
    // SAFETY: just opened, owned by nobody else.
    Some(Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) }))
}

/// `root/relative`, a regular file, opened for reading without following any link on the way.
pub fn open_file(root: &Path, relative: &Path) -> io::Result<File> {
    Dir::open(root)?.open_file(relative)
}

/// `open_file` under a root that is itself checked (`Dir::open_exact`): for a root inside a folder a sandbox writes
/// in (an archive at `/workspace/notes`), whose own path a link swapped in could lead elsewhere.
pub fn open_file_exact(root: &Path, relative: &Path) -> io::Result<File> {
    Dir::open_exact(root)?.open_file(relative)
}

// --- as the sandboxes' « root » ------------------------------------------------------------------------------------

/// The command that runs `bipbridge fs-as-root <args>` as the sandboxes' « root » (`podman unshare`): this account
/// mapped to root in the sandboxes' user namespace, so the files a sandbox's user made can be read and changed. The
/// work is done by this same binary, through folder descriptors, never by a shell on paths.
pub fn as_root(podman: &str, args: &[&OsStr]) -> io::Result<std::process::Output> {
    let exe = std::env::current_exe()?;
    // replaced by an update while running: the new binary at the same place
    let exe = match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(plain) => PathBuf::from(plain),
        None => exe,
    };
    std::process::Command::new(podman)
        .arg("unshare")
        .arg(exe)
        .arg(CLI)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
}

/// The hidden subcommand.
pub const CLI: &str = "fs-as-root";

/// `bipbridge fs-as-root mirror <src> <dst>` | `open-modes <root>` | `read <root> <relative> <limit>` (as the
/// sandboxes' « root », from `as_root`).
pub fn cli(argv: &[String]) -> i32 {
    let arg = |i: usize| argv.get(i).map(PathBuf::from);
    let result: io::Result<()> = match argv.first().map(String::as_str) {
        Some("mirror") => match (arg(1), arg(2)) {
            (Some(src), Some(dst)) => crate::shares::sync_tree(&src, &dst).map(|(c, r)| println!("{c} {r}")),
            _ => Err(io::Error::other("usage: fs-as-root mirror SRC DST")),
        },
        Some("open-modes") => match arg(1) {
            Some(root) => {
                let tended = crate::sandbox_adopt::open_modes_as(&root, None);
                println!("{} {}", tended.opened, tended.removed_links);
                Ok(())
            }
            None => Err(io::Error::other("usage: fs-as-root open-modes ROOT")),
        },
        Some("read") => match (arg(1), arg(2), argv.get(3).and_then(|l| l.parse::<u64>().ok())) {
            (Some(root), Some(relative), Some(limit)) => open_file_exact(&root, &relative).and_then(|file| {
                let mut out = io::stdout().lock();
                io::copy(&mut file.take(limit), &mut out)?;
                out.flush()
            }),
            _ => Err(io::Error::other("usage: fs-as-root read ROOT RELATIVE LIMIT")),
        },
        _ => Err(io::Error::other("usage: fs-as-root mirror|open-modes|read …")),
    };
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

/// `a` and `b` swapped in one step (Linux `renameat2(RENAME_EXCHANGE)`, macOS `renamex_np(RENAME_SWAP)`): what a
/// sandbox does to win a race, for the tests.
#[cfg(test)]
pub(crate) fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    let (a, b) = (CString::new(a.as_os_str().as_bytes())?, CString::new(b.as_os_str().as_bytes())?);
    #[cfg(target_os = "linux")]
    // SAFETY: NUL-terminated paths.
    let r = unsafe { libc::renameat2(libc::AT_FDCWD, a.as_ptr(), libc::AT_FDCWD, b.as_ptr(), libc::RENAME_EXCHANGE) };
    #[cfg(target_os = "macos")]
    // SAFETY: NUL-terminated paths.
    let r = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) };
    cvt(r).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn names_only_and_never_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("share");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f.txt"), "ok").unwrap();
        std::fs::write(dir.path().join("secret"), "key").unwrap();
        std::os::unix::fs::symlink(dir.path(), root.join("up")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), root.join("s")).unwrap();
        let share = Dir::open(&root).unwrap();
        let mut text = String::new();
        share.open_file(Path::new("a/b/f.txt")).unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "ok");
        for bad in ["up/secret", "s", "../secret", "/a/b/f.txt/..", "a/../../secret", "a/b"] {
            assert!(share.open_file(Path::new(bad)).is_err(), "{bad}");
        }
        assert!(share.sub(Path::new("up")).is_err());
        assert!(share.make_dirs(Path::new("up/x"), None).is_err());
        assert!(!dir.path().join("x").exists());
        assert!(share.parent_of(Path::new("up/new.txt")).is_err());
        assert!(share.chmod_at(OsStr::new("s"), 0o777).is_err());
        assert_ne!(mode(&dir.path().join("secret")), 0o777);
        // removing a link removes the link, never what it leads to
        assert_eq!(share.remove(OsStr::new("up")).unwrap(), 1);
        assert!(dir.path().join("secret").exists() && root.join("a/b/f.txt").exists());
    }

    /// Linux: `openat2` itself refuses a link anywhere on the way and a way out (the per-name walk is only its
    /// fallback), and a mode is set through an `O_PATH` descriptor, never on what a link leads to.
    #[cfg(target_os = "linux")]
    #[test]
    fn openat2_refuses_links_and_ways_out() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("share");
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/f.txt"), "ok").unwrap();
        std::fs::write(dir.path().join("secret"), "key").unwrap();
        std::os::unix::fs::symlink(dir.path(), root.join("up")).unwrap();
        std::os::unix::fs::symlink("a", root.join("rel")).unwrap();
        let share = Dir::open(&root).unwrap();
        let flags = libc::O_RDONLY;
        let Some(found) = openat2(share.raw(), Path::new("a/f.txt"), flags) else {
            eprintln!("no openat2 on this kernel: the per-name walk is what runs");
            return;
        };
        assert!(found.is_ok());
        for bad in ["up/secret", "rel/f.txt"] {
            let err = openat2(share.raw(), Path::new(bad), flags).expect("openat2 answered").unwrap_err();
            assert!(refused(&err), "{bad}: {err}");
        }
        // a way out: EXDEV from openat2 (then the walk, which never takes `..`), never the file
        assert!(!matches!(openat2(share.raw(), Path::new("../secret"), flags), Some(Ok(_))));
        assert!(share.open_file(Path::new("../secret")).is_err());
        std::os::unix::fs::symlink(dir.path().join("secret"), root.join("s")).unwrap();
        std::fs::set_permissions(dir.path().join("secret"), std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(share.chmod_at(OsStr::new("s"), 0o666).is_err());
        assert_eq!(mode(&dir.path().join("secret")), 0o600);
        share.chmod_at(OsStr::new("a"), 0o751).unwrap();
        assert_eq!(mode(&root.join("a")), 0o751);
        assert_eq!(share.sub(Path::new("a")).unwrap().real_path().unwrap(), root.join("a"));
    }

    #[test]
    fn written_through_a_temporary_file_and_removed_whole() {
        let dir = tempfile::tempdir().unwrap();
        let share = Dir::open(dir.path()).unwrap();
        let sub = share.make_dirs(Path::new("x/y"), Some(0o777)).unwrap();
        assert_eq!(mode(&dir.path().join("x/y")), 0o777);
        sub.write_file(OsStr::new("f.md"), 0o666, None, |out| out.write_all(b"hello")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("x/y/f.md")).unwrap(), "hello");
        assert_eq!(mode(&dir.path().join("x/y/f.md")), 0o666);
        assert_eq!(sub.entries().unwrap().len(), 1); // no temporary file left
        assert!(share.write_file(OsStr::new("x"), 0o666, None, |_| Ok(())).is_err()); // a folder
        assert_eq!(share.remove(OsStr::new("x")).unwrap(), 3);
        assert!(!dir.path().join("x").exists());
    }

    /// A thread swaps a folder of the share for a link to « home » as fast as it can while the bridge reads, writes,
    /// changes modes and removes through it: nothing outside the share is ever reached.
    #[test]
    fn a_folder_swapped_for_a_link_never_leads_out() {
        let dir = tempfile::tempdir().unwrap();
        let (root, home) = (dir.path().join("share"), dir.path().join("home"));
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(root.join("d/f.txt"), "share").unwrap();
        std::fs::write(home.join("f.txt"), "home secret").unwrap();
        std::fs::set_permissions(home.join("f.txt"), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let swapper = {
            let (root, home, stop) = (root.clone(), home.clone(), stop.clone());
            std::thread::spawn(move || {
                let (real, link) = (root.join("d"), root.join("d.link"));
                std::os::unix::fs::symlink(&home, &link).unwrap();
                while !stop.load(Ordering::Relaxed) {
                    // `d` is the folder, then the link to home, then the folder again: swapped in one step each
                    exchange(&real, &link).unwrap();
                    std::thread::yield_now();
                    exchange(&real, &link).unwrap();
                }
            })
        };
        let share = Dir::open(&root).unwrap();
        let (mut read_ok, mut wrote_ok) = (0, 0);
        for i in 0..4000 {
            if let Ok(mut file) = share.open_file(Path::new("d/f.txt")) {
                let mut text = String::new();
                file.read_to_string(&mut text).unwrap();
                assert_eq!(text, "share", "read through the link");
                read_ok += 1;
            }
            if let Ok(sub) = share.sub(Path::new("d")) {
                sub.chmod_if_owned(0o777);
                let name = OsString::from(format!("w{}.txt", i % 3));
                if sub.write_file(&name, 0o666, None, |out| out.write_all(b"from the agent")).is_ok() {
                    wrote_ok += 1;
                }
                let _ = sub.chmod_at(OsStr::new("f.txt"), 0o666);
                let _ = sub.remove(&name);
            }
            if let Ok((parent, name)) = share.parent_of(Path::new("d/f.txt")) {
                let _ = parent.chmod_at(&name, 0o666);
            }
        }
        stop.store(true, Ordering::Relaxed);
        swapper.join().unwrap();
        assert!(read_ok > 0 && wrote_ok > 0, "the race never let anything through: {read_ok} {wrote_ok}");
        let names: Vec<_> = std::fs::read_dir(&home).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, [OsString::from("f.txt")], "nothing written in home");
        assert_eq!(std::fs::read_to_string(home.join("f.txt")).unwrap(), "home secret");
        assert_eq!(mode(&home.join("f.txt")), 0o600, "home's file mode untouched");
        assert_eq!(mode(&home), 0o700, "home's mode untouched");
    }
}
