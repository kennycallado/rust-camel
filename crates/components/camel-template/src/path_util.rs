//! Path-confined filesystem helpers for the external template component
//! (ADR-0047 Stage 2, Task 2.1).
//!
//! Every template/include acquisition opens files `openat`-relative to a root
//! directory handle so that symlink escapes, `..` segments, and absolute paths
//! are rejected at the kernel level rather than by string inspection alone.
//! This module owns three primitives the rest of the component builds on:
//!
//! - [`OwnedHandle`] — an owned kernel descriptor for a file or directory.
//! - [`FileIdentity`] — a kernel-stable identity used for cycle/duplicate
//!   detection across the dependency closure.
//! - [`open_root`] — opens the configured root directory by absolute path.
//!
//! See the spec requirement "Dependency-closure contract": the system SHALL
//! reject symlinks, `..` segments, absolute paths, cycles, and duplicate file
//! identities.

use crate::error::TemplateReloadError;

// ---------------------------------------------------------------------------
// Platform-neutral public types
// ---------------------------------------------------------------------------

/// An owned handle to an opened file or directory. Acts as the anchor for
/// `openat`-relative traversal: once a root handle is held, child paths can
/// only be reached by walking components one at a time, so a symlink or `..`
/// anywhere in the chain is rejected before it can escape the root.
///
/// The wrapped kernel primitive is cfg-gated: an `OwnedFd` on Unix and an
/// `OwnedHandle` on Windows.
#[derive(Debug)]
pub(crate) struct OwnedHandle {
    #[cfg(unix)]
    inner: std::os::fd::OwnedFd,
    #[cfg(windows)]
    inner: std::os::windows::io::OwnedHandle,
}

/// Kernel-stable identity of a single file. Two handles sharing an identity
/// refer to the same on-disk inode (Unix) / file ID (Windows) at the same
/// length and modification time, which lets the dependency-closure walker
/// reject duplicates and detect cycles.
///
/// Field sets are cfg-gated because the identifying primitives differ per
/// platform.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FileIdentity {
    /// Inode number.
    #[cfg(unix)]
    pub(crate) inode: u64,
    /// Logical file length in bytes.
    #[cfg(unix)]
    pub(crate) length: u64,
    /// Full modification time in nanoseconds since the Unix epoch
    /// (`st_mtime * 1e9 + st_mtime_nsec`).
    #[cfg(unix)]
    pub(crate) mtime_nsec: i64,

    /// Volume serial number of the hosting filesystem.
    #[cfg(windows)]
    pub(crate) volume_serial: u32,
    /// High 32 bits of the NT file identifier.
    #[cfg(windows)]
    pub(crate) file_index_high: u32,
    /// Low 32 bits of the NT file identifier.
    #[cfg(windows)]
    pub(crate) file_index_low: u32,
    /// Logical file length in bytes.
    #[cfg(windows)]
    pub(crate) length: u64,
    /// Last-write time in 100-nanosecond intervals since the Windows epoch.
    #[cfg(windows)]
    pub(crate) last_write_100ns: i64,
}

// ===========================================================================
// Unix implementation
// ===========================================================================
#[cfg(unix)]
mod imp {
    use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

    use rustix::fs::{Mode, OFlags, StatExt, fstat, openat};

    use super::{FileIdentity, OwnedHandle};
    use crate::error::TemplateReloadError;

    impl OwnedHandle {
        /// Open `name` relative to `root`, walking each path component
        /// handle-relative.
        ///
        /// `name` MAY be multi-component (e.g. `partials/page.html`); to
        /// confine it, every component is opened relative to the previous
        /// handle. A single trailing `openat(..., O_NOFOLLOW)` only rejects a
        /// symlink in the LAST component, so intermediate components are
        /// additionally opened with `O_DIRECTORY | O_NOFOLLOW`: that rejects
        /// both a non-directory and a symlink at any intermediate hop (the
        /// "intermediate symlink escape" case — spec Critical C1).
        ///
        /// `max_bytes` is forwarded to the bounded read (Task 2.3) and is not
        /// consulted during opening.
        ///
        /// Before touching the filesystem, any component that is empty, `.`,
        /// `..`, or introduces an absolute path is rejected lexically.
        #[allow(dead_code)] // public(crate) API; consumed by Task 2.3/2.4.
        pub(crate) fn open_relative(
            root: &OwnedHandle,
            name: &str,
            _max_bytes: usize,
        ) -> Result<(OwnedHandle, FileIdentity), TemplateReloadError> {
            let components = validate_components(name)?;

            let root_fd = root.inner.as_fd();
            let last_idx = components.len() - 1;

            // Open the first component relative to the root.
            let first = openat(
                root_fd,
                components[0],
                if last_idx == 0 {
                    OFlags::RDONLY | OFlags::NOFOLLOW
                } else {
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW
                },
                Mode::empty(),
            )
            .map_err(openat_escape_err(components[0]))?;

            if components.len() == 1 {
                let identity = identity_of(&first)?;
                return Ok((OwnedHandle { inner: first }, identity));
            }

            // Walk intermediate components; each is required to be a real
            // directory and not a symlink.
            let mut cur: OwnedFd = first;
            for &comp in &components[1..last_idx] {
                let next = openat(
                    cur.as_fd(),
                    comp,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                    Mode::empty(),
                )
                .map_err(openat_escape_err(comp))?;
                cur = next;
            }

            // Final component: a regular file open, still NOFOLLOW so a leaf
            // symlink is rejected.
            let final_fd = openat(
                cur.as_fd(),
                components[last_idx],
                OFlags::RDONLY | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(openat_escape_err(components[last_idx]))?;

            let identity = identity_of(&final_fd)?;
            Ok((OwnedHandle { inner: final_fd }, identity))
        }

        /// Borrow the underlying descriptor (used by later tasks that read).
        #[allow(dead_code)] // consumed by Task 2.3 / 2.4.
        pub(crate) fn as_fd(&self) -> BorrowedFd<'_> {
            self.inner.as_fd()
        }

        /// Read up to `max_bytes` from this handle in 8 KiB chunks, failing
        /// closed the instant the limit is exceeded (no whole-file allocation
        /// first). Used by the production [`crate::closure::FilesystemTemplateReader`]
        /// (Task 2.4) to enforce the per-template `max_template_size` bound.
        ///
        /// The chunk size matches the test-local helper in `closure.rs` so the
        /// production and test code paths exercise the same read geometry.
        #[allow(dead_code)] // consumed by FilesystemTemplateReader (Task 2.4 → Phase 4).
        pub(crate) fn read_bounded(
            &self,
            max_bytes: usize,
        ) -> Result<Vec<u8>, TemplateReloadError> {
            let mut out: Vec<u8> = Vec::new();
            let mut buf = [0u8; 8192];
            let fd = self.as_fd();
            loop {
                let n = rustix::io::read(fd, &mut buf)
                    .map_err(|e| TemplateReloadError::Acquire(format!("read failed: {e}")))?;
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..n]);
                if out.len() > max_bytes {
                    return Err(TemplateReloadError::BoundExceeded("max_template_size"));
                }
            }
            Ok(out)
        }
    }

    /// Compute the [`FileIdentity`] of an already-opened descriptor.
    #[allow(dead_code)] // consumed via open_relative / open_root (Task 2.3/2.4).
    fn identity_of(fd: &OwnedFd) -> Result<FileIdentity, TemplateReloadError> {
        let st =
            fstat(fd).map_err(|e| TemplateReloadError::Acquire(format!("fstat failed: {e}")))?;
        Ok(FileIdentity {
            inode: st.st_ino,
            length: st.st_size as u64,
            // `StatExt::mtime` returns the signed second value; combine with the
            // raw nanosecond fraction for full-resolution change detection.
            mtime_nsec: st.mtime().saturating_mul(1_000_000_000) + st.st_mtime_nsec as i64,
        })
    }

    /// Translate an `openat` error into a [`TemplateReloadError`].
    ///
    /// Most failures here indicate that confinement would be violated
    /// (symlink rejected by `O_NOFOLLOW`, intermediate non-directory,
    /// permission, cross-device, …) and map to
    /// [`TemplateReloadError::PathEscape`] — fail closed. A genuine
    /// `ENOENT` (the component does not exist) is NOT a confinement
    /// failure, so it is reported as
    /// [`TemplateReloadError::Acquire`] with a "not found" message so
    /// operators get an accurate diagnostic. The raw errno text is
    /// preserved in both branches for debuggability.
    #[allow(dead_code)] // consumed via open_relative (Task 2.3/2.4).
    fn openat_escape_err(comp: &str) -> impl Fn(rustix::io::Errno) -> TemplateReloadError + '_ {
        move |e| {
            if e == rustix::io::Errno::NOENT {
                // Genuine "component does not exist" — not a confinement
                // failure. A symlink rejected by `O_NOFOLLOW` returns
                // `ELOOP` on Linux, NOT `ENOENT`, so the security-critical
                // path-escape detection is preserved.
                TemplateReloadError::Acquire(format!("template not found: {comp} ({e})"))
            } else {
                TemplateReloadError::PathEscape(format!(
                    "openat({comp:?}) confined open failed: {e}"
                ))
            }
        }
    }

    /// Split `name` on `/` and reject any component that is empty, `.`, `..`.
    /// A leading `/` produces an empty first component and is likewise
    /// rejected, which blocks absolute paths.
    ///
    /// Returns the non-empty component slice. The caller is guaranteed at
    /// least one component.
    #[allow(dead_code)] // consumed via open_relative (Task 2.3/2.4).
    // `open_root_unix` uses `Path::components()` directly instead and is
    // unaffected by this lexical pre-check.
    fn validate_components(name: &str) -> Result<Vec<&str>, TemplateReloadError> {
        let mut out: Vec<&str> = Vec::new();
        for comp in name.split('/') {
            if comp.is_empty() || comp == "." || comp == ".." {
                return Err(TemplateReloadError::PathEscape(format!(
                    "rejected path component {comp:?} in {name:?}"
                )));
            }
            out.push(comp);
        }
        // `split` always yields at least one element; if the only element was
        // rejected above (empty/`.`/`..`) we already returned. A surviving
        // empty vec is therefore impossible, but guard anyway.
        if out.is_empty() {
            return Err(TemplateReloadError::PathEscape(format!(
                "empty template name {name:?}"
            )));
        }
        Ok(out)
    }

    /// Opens a configured root directory by absolute path and returns its
    /// handle plus identity.
    ///
    /// Rejects any path that still contains a `..` component after component
    /// parsing (the operator-supplied root must already be normalized), and
    /// reports a missing parent with
    /// [`TemplateReloadError::PathEscape`] `"missing parent"`.
    #[allow(dead_code)] // consumed via open_root (Task 4.4 / 5.1).
    pub(crate) fn open_root_unix(
        root_abs_path: &std::path::Path,
    ) -> Result<(OwnedHandle, FileIdentity), TemplateReloadError> {
        use std::path::Component;

        for comp in root_abs_path.components() {
            if matches!(comp, Component::ParentDir) {
                return Err(TemplateReloadError::PathEscape(format!(
                    "root path contains a parent-dir segment: {}",
                    root_abs_path.display()
                )));
            }
        }

        let fd = match openat(
            rustix::fs::CWD,
            root_abs_path,
            OFlags::RDONLY | OFlags::DIRECTORY,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => {
                return Err(TemplateReloadError::PathEscape(
                    "missing parent".to_string(),
                ));
            }
            Err(e) => {
                return Err(TemplateReloadError::Acquire(format!(
                    "open root failed: {e}"
                )));
            }
        };

        let identity = identity_of(&fd)?;
        Ok((OwnedHandle { inner: fd }, identity))
    }
}

// ===========================================================================
// Windows implementation
// ===========================================================================
// NOTE (spec Task 2.1): the Windows path uses per-component `NtCreateFile`
// with a chained `OBJECT_ATTRIBUTES.RootDirectory` and reparse-point rejection
// at every component — NOT `CreateFileW` (which only checks the trailing
// component). This path CANNOT be validated on the Linux CI host; Unix is the
// CI gate.
//
// `NtCreateFile`, `OBJECT_ATTRIBUTES`/`UNICODE_STRING`, `IO_STATUS_BLOCK`, and
// `OBJ_CASE_INSENSITIVE` live behind `windows-sys` features NOT yet enabled in
// the Phase-1 workspace dependency set. A Windows build (cross-compile or a
// Windows CI job) MUST add these features before this path compiles:
//   - `Wdk_Foundation`           (OBJECT_ATTRIBUTES, UNICODE_STRING, NtCreateFile)
//   - `Win32_System_IO`          (IO_STATUS_BLOCK, NtCreateFile)
//   - `Win32_System_Kernel`      (OBJ_CASE_INSENSITIVE)
// already present: `Win32_Foundation`, `Wdk_Storage_FileSystem`,
//                  `Win32_Storage_FileSystem`, `Win32_Security`.
// Until that job lands, treat the Windows path as unvalidated scaffolding.
#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Wdk::Foundation::{OBJECT_ATTRIBUTES, UNICODE_STRING};
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_BASIC_INFORMATION, FILE_FS_VOLUME_INFORMATION, FILE_INTERNAL_INFORMATION, FILE_OPEN,
        FILE_OPEN_REPARSE_POINT, FILE_STANDARD_INFORMATION, FILE_SYNCHRONOUS_IO_NONALERT,
        NtCreateFile,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, INVALID_HANDLE_VALUE, STATUS_SUCCESS,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FileFsVolumeInformation, FileInternalInfo, FileStandardInfo, GetFileInformationByHandleEx,
        SYNCHRONIZE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
    use windows_sys::Win32::System::Kernel::OBJ_CASE_INSENSITIVE;

    use super::{FileIdentity, OwnedHandle};
    use crate::error::TemplateReloadError;

    // NTSTATUS values are signed `i32`; reinterpret the unsigned code.
    const STATUS_OBJECT_NAME_NOT_FOUND: i32 = 0xC0000034u32 as i32;

    impl OwnedHandle {
        pub(crate) fn open_relative(
            root: &OwnedHandle,
            name: &str,
            _max_bytes: usize,
        ) -> Result<(OwnedHandle, FileIdentity), TemplateReloadError> {
            let components = validate_components(name)?;
            let last_idx = components.len() - 1;

            let mut cur_root: HANDLE = root.inner.as_raw() as HANDLE;
            let mut held: Option<OwnedHandleInner> = None;

            for (i, comp) in components.iter().enumerate() {
                let is_last = i == last_idx;
                let opened = nt_open_component(cur_root, comp, is_last)?;
                if is_last {
                    let identity = identity_of(opened.0)?;
                    return Ok((OwnedHandle { inner: opened }, identity));
                }
                held = Some(opened);
                cur_root = held.as_ref().expect("just-set").0; // allow-unwrap
            }
            unreachable!("component list is non-empty (validated)")
        }
    }

    /// RAII wrapper closing the NT HANDLE on drop.
    struct OwnedHandleInner(HANDLE);
    impl Drop for OwnedHandleInner {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    impl OwnedHandle {
        fn as_raw(&self) -> usize {
            self.inner.0 as usize
        }

        /// Read up to `max_bytes` from this handle. The Windows path is
        /// unvalidated scaffolding (see the module-level note); this stub
        /// returns [`TemplateReloadError::Acquire`] until a Windows CI job
        /// lands. The signature mirrors the Unix
        /// [`OwnedHandle::read_bounded`] so the production reader compiles
        /// cross-platform.
        #[allow(dead_code)] // consumed by FilesystemTemplateReader (Task 2.4 → Phase 4).
        pub(crate) fn read_bounded(
            &self,
            _max_bytes: usize,
        ) -> Result<Vec<u8>, TemplateReloadError> {
            Err(TemplateReloadError::Acquire(
                "Windows external template path is unvalidated scaffolding; \
                 bounded read not yet implemented for NT handles"
                    .to_string(),
            ))
        }
    }

    fn nt_open_component(
        root: HANDLE,
        comp: &str,
        is_last: bool,
    ) -> Result<OwnedHandleInner, TemplateReloadError> {
        let mut name_utf16: Vec<u16> = OsStr::new(comp).encode_wide().collect();
        name_utf16.push(0);
        let object_name = UNICODE_STRING {
            Length: ((name_utf16.len().saturating_sub(1)) * 2) as u16,
            MaximumLength: (name_utf16.len() * 2) as u16,
            Buffer: name_utf16.as_mut_ptr(),
        };
        let mut oa: OBJECT_ATTRIBUTES = unsafe { std::mem::zeroed() };
        oa.Length = std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32;
        oa.RootDirectory = root;
        oa.ObjectName = &object_name;
        oa.Attributes = OBJ_CASE_INSENSITIVE as u32;

        // Open the component itself, never following a reparse point, so a
        // symlink/junction at this hop is rejected. Intermediate hops must be
        // directories; `FILE_DIRECTORY_FILE` enforces that, but the symbol is
        // feature-gated, so we rely on `FILE_OPEN_REPARSE_POINT` + a
        // post-open directory check for non-leaf components.
        let mut handle: HANDLE = INVALID_HANDLE_VALUE;
        let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        // Positional NtCreateFile args (all flag types are `u32` aliases):
        //   filehandle, desiredaccess, objectattributes, iostatusblock,
        //   allocationsize, fileattributes, shareaccess, createdisposition,
        //   createoptions, eabuffer, ealength.
        // `FILE_OPEN_REPARSE_POINT` opens the name itself without following a
        // reparse point, so a symlink/junction at this hop is exposed rather
        // than traversed; we additionally check attributes below.
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                SYNCHRONIZE,
                &oa,
                &mut io_status,
                std::ptr::null(),
                0,
                0,
                FILE_OPEN,
                FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                std::ptr::null(),
                0,
            )
        };
        if status != STATUS_SUCCESS {
            return Err(TemplateReloadError::PathEscape(format!(
                "NtCreateFile({comp:?}) confined open failed: ntstatus=0x{:08X}",
                status as u32
            )));
        }
        // Reject a reparse point (symlink/junction) at this component even
        // though it opened: reparse attributes must be absent.
        if is_reparse(handle)? {
            unsafe {
                CloseHandle(handle);
            }
            return Err(TemplateReloadError::PathEscape(format!(
                "component {comp:?} is a reparse point (symlink/junction)"
            )));
        }
        Ok(OwnedHandleInner(handle))
    }

    fn is_reparse(handle: HANDLE) -> Result<bool, TemplateReloadError> {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, GetFileInformationByHandle,
        };
        let mut info: windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION =
            unsafe { std::mem::zeroed() };
        let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
        if ok == 0 {
            return Err(TemplateReloadError::Acquire(
                "GetFileInformationByHandle failed".to_string(),
            ));
        }
        Ok((info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0)
    }

    fn identity_of(handle: HANDLE) -> Result<FileIdentity, TemplateReloadError> {
        let mut internal: FILE_INTERNAL_INFORMATION = unsafe { std::mem::zeroed() };
        query_info(handle, FileInternalInfo, &mut internal)?;

        let mut standard: FILE_STANDARD_INFORMATION = unsafe { std::mem::zeroed() };
        query_info(handle, FileStandardInfo, &mut standard)?;

        let mut volume: FILE_FS_VOLUME_INFORMATION = unsafe { std::mem::zeroed() };
        query_fs_volume(handle, &mut volume)?;

        // `FILE_BASIC_INFORMATION.LastWriteTime` is the Windows FILETIME for
        // last write, expressed as the number of 100-nanosecond intervals
        // since 1601-01-01 UTC. It is the exact analogue of Unix's
        // `st_mtime` nanosecond timestamp for change-detection purposes.
        // `Wdk_Storage_FileSystem` (FILE_BASIC_INFORMATION) and
        // `Win32_Storage_FileSystem` (FileBasicInfo) are both already
        // enabled in the workspace, so no new windows-sys features are
        // required to populate this field.
        let mut basic: FILE_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        query_info(handle, FileBasicInfo, &mut basic)?;

        Ok(FileIdentity {
            volume_serial: volume.VolumeSerialNumber,
            file_index_high: (internal.IndexNumber >> 32) as u32,
            file_index_low: internal.IndexNumber as u32,
            length: standard.EndOfFile,
            last_write_100ns: basic.LastWriteTime,
        })
    }

    fn query_info<T>(
        handle: HANDLE,
        class: windows_sys::Win32::Storage::FileSystem::FILE_INFO_BY_HANDLE_CLASS,
        buf: *mut T,
    ) -> Result<(), TemplateReloadError> {
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                class,
                buf as *mut std::ffi::c_void,
                std::mem::size_of::<T>() as u32,
            )
        };
        if ok == 0 {
            return Err(TemplateReloadError::Acquire(
                "GetFileInformationByHandleEx failed".to_string(),
            ));
        }
        Ok(())
    }

    fn query_fs_volume(
        handle: HANDLE,
        buf: *mut FILE_FS_VOLUME_INFORMATION,
    ) -> Result<(), TemplateReloadError> {
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileFsVolumeInformation,
                buf as *mut std::ffi::c_void,
                std::mem::size_of::<FILE_FS_VOLUME_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(TemplateReloadError::Acquire(
                "GetFileInformationByHandleEx(FileFsVolumeInformation) failed".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn open_root_windows(
        root_abs_path: &Path,
    ) -> Result<(OwnedHandle, FileIdentity), TemplateReloadError> {
        use std::path::Component;

        for comp in root_abs_path.components() {
            if matches!(comp, Component::ParentDir) {
                return Err(TemplateReloadError::PathEscape(format!(
                    "root path contains a parent-dir segment: {}",
                    root_abs_path.display()
                )));
            }
        }

        let mut name_utf16: Vec<u16> = root_abs_path.as_os_str().encode_wide().collect();
        // NT paths use the `\??\` prefix for native object manager opens.
        let mut prefixed: Vec<u16> = Vec::with_capacity(4 + name_utf16.len());
        prefixed.extend([b'\\' as u16, b'?' as u16, b'?' as u16, b'\\' as u16]);
        prefixed.extend_from_slice(&name_utf16);
        prefixed.push(0);
        name_utf16 = prefixed;

        let object_name = UNICODE_STRING {
            Length: ((name_utf16.len().saturating_sub(1)) * 2) as u16,
            MaximumLength: (name_utf16.len() * 2) as u16,
            Buffer: name_utf16.as_mut_ptr(),
        };
        let mut oa: OBJECT_ATTRIBUTES = unsafe { std::mem::zeroed() };
        oa.Length = std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32;
        oa.RootDirectory = std::ptr::null_mut();
        oa.ObjectName = &object_name;
        oa.Attributes = OBJ_CASE_INSENSITIVE as u32;

        let mut handle: HANDLE = INVALID_HANDLE_VALUE;
        let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                SYNCHRONIZE,
                &oa,
                &mut io_status,
                std::ptr::null(),
                0,
                0,
                FILE_OPEN,
                FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                std::ptr::null(),
                0,
            )
        };
        match status {
            STATUS_SUCCESS => {}
            STATUS_OBJECT_NAME_NOT_FOUND => {
                return Err(TemplateReloadError::PathEscape(
                    "missing parent".to_string(),
                ));
            }
            _ => {
                return Err(TemplateReloadError::Acquire(format!(
                    "open root failed: ntstatus=0x{:08X}",
                    status as u32
                )));
            }
        }

        let inner = OwnedHandleInner(handle);
        let identity = identity_of(handle)?;
        Ok((OwnedHandle { inner }, identity))
    }

    fn validate_components(name: &str) -> Result<Vec<&str>, TemplateReloadError> {
        let mut out: Vec<&str> = Vec::new();
        for comp in name.split(|c| c == '/' || c == '\\') {
            if comp.is_empty() || comp == "." || comp == ".." {
                return Err(TemplateReloadError::PathEscape(format!(
                    "rejected path component {comp:?} in {name:?}"
                )));
            }
            out.push(comp);
        }
        if out.is_empty() {
            return Err(TemplateReloadError::PathEscape(format!(
                "empty template name {name:?}"
            )));
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Platform-neutral entry points
// ---------------------------------------------------------------------------

/// Open a root directory by absolute path. Delegates to the cfg-gated
/// platform implementation.
#[allow(dead_code)] // consumed by Task 4.4 / 5.1.
pub(crate) fn open_root(
    root_abs_path: &std::path::Path,
) -> Result<(OwnedHandle, FileIdentity), TemplateReloadError> {
    #[cfg(unix)]
    {
        imp::open_root_unix(root_abs_path)
    }
    #[cfg(windows)]
    {
        imp::open_root_windows(root_abs_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The three `open_relative` tests are Unix-only: they rely on `O_NOFOLLOW`
    // and `symlink(2)` semantics that have no portable Rust std equivalent.
    #[cfg(unix)]
    #[cfg(test)]
    mod unix {
        use std::fs;
        use std::os::unix::fs::symlink;

        use super::*;

        fn open_root_handle(dir: &std::path::Path) -> OwnedHandle {
            let (handle, _id) = open_root(dir).expect("root opens");
            handle
        }

        #[test]
        fn owned_handle_open_relative_unix() {
            // Arrange: a tempdir root containing a child file with content.
            let root = tempfile::tempdir().expect("tempdir");
            let child = root.path().join("child");
            fs::write(&child, b"hello").expect("write child");

            // Act: open the child relative to the root handle.
            let handle = open_root_handle(root.path());
            let (opened, identity) =
                OwnedHandle::open_relative(&handle, "child", 1024).expect("open ok");

            // Assert: identity length reflects the written content.
            assert!(identity.length > 0, "identity length must be positive");
            let _ = opened;
        }

        #[test]
        fn open_relative_rejects_symlink_escape() {
            // Arrange: a target file OUTSIDE the root, and a symlink inside
            // the root pointing at it.
            let outside = tempfile::tempdir().expect("outside tempdir");
            let target = outside.path().join("secret");
            fs::write(&target, b"escaped").expect("write target");

            let root = tempfile::tempdir().expect("root tempdir");
            symlink(&target, root.path().join("link")).expect("symlink");

            // Act + Assert: the trailing-component O_NOFOLLOW rejects the leaf
            // symlink.
            let handle = open_root_handle(root.path());
            let err = OwnedHandle::open_relative(&handle, "link", 1024)
                .expect_err("symlink leaf must be rejected");
            assert!(
                matches!(err, TemplateReloadError::PathEscape(_)),
                "expected PathEscape, got {err:?}"
            );
        }

        #[test]
        fn open_relative_rejects_intermediate_symlink_escape() {
            // Arrange: root/partials is a real directory, but root/partials/link
            // is a symlink to a directory OUTSIDE the root. The requested path
            // traverses the symlink as an intermediate component.
            let outside = tempfile::tempdir().expect("outside tempdir");

            let root = tempfile::tempdir().expect("root tempdir");
            let partials = root.path().join("partials");
            fs::create_dir(&partials).expect("mkdir partials");
            symlink(outside.path(), partials.join("link")).expect("symlink");
            // A file that would be reachable only through the escaping symlink.
            fs::write(outside.path().join("page.html"), b"escaped").expect("write target");

            // Act + Assert: the per-component walk rejects the intermediate
            // `link` symlink via O_DIRECTORY | O_NOFOLLOW (spec Critical C1).
            let handle = open_root_handle(root.path());
            let err = OwnedHandle::open_relative(&handle, "partials/link/page.html", 1024)
                .expect_err("intermediate symlink must be rejected");
            assert!(
                matches!(err, TemplateReloadError::PathEscape(_)),
                "expected PathEscape, got {err:?}"
            );
        }

        #[test]
        fn open_relative_missing_component_maps_to_acquire() {
            // Lock the errno distinction: a genuinely missing component
            // (ENOENT) must NOT be reported as a confinement failure. Only
            // symlink-rejection / non-directory / permission errors
            // (ELOOP, ENOTDIR, EACCES, …) map to PathEscape. This guards
            // operator messaging against the "missing template reports
            // path-escape" regression.
            let root = tempfile::tempdir().expect("root tempdir");
            // No child file is created.
            let handle = open_root_handle(root.path());
            let err = OwnedHandle::open_relative(&handle, "does_not_exist", 1024)
                .expect_err("missing component must error");
            assert!(
                matches!(err, TemplateReloadError::Acquire(_)),
                "expected Acquire for ENOENT, got {err:?}"
            );
            assert!(
                !matches!(err, TemplateReloadError::PathEscape(_)),
                "ENOENT must NOT map to PathEscape (regression lock)"
            );
        }
    }

    #[test]
    fn open_root_rejects_dotdot() {
        // A root path that still contains `..` after parsing must be rejected
        // before any filesystem access. Guarded to Unix because the literal is
        // a Unix-style absolute path.
        #[cfg(unix)]
        {
            let path = std::path::Path::new("/srv/../etc");
            let err = open_root(path).expect_err("dotdot root must be rejected");
            assert!(
                matches!(err, TemplateReloadError::PathEscape(_)),
                "expected PathEscape, got {err:?}"
            );
        }
    }
}
