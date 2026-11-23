//! What stopped a folder's deletion halfway: Windows deletes no file a program has loaded or
//! holds open, and git says only "Invalid argument" (#445). Elsewhere an open file is deleted
//! like any other, so nothing is found.

use std::path::{Path, PathBuf};

/// A file left in a folder that programs hold open.
#[derive(Debug)]
pub(crate) struct InUse {
    /// Relative to the folder.
    pub file: PathBuf,
    pub programs: Vec<Program>,
}

/// A running program, as Windows names it.
#[derive(Debug)]
pub(crate) struct Program {
    pub name: String,
    pub id: u32,
}

impl InUse {
    /// "file is in use by Name (process 12): close it", for after "left behind: folder.".
    pub fn explain(&self) -> String {
        let names: Vec<String> = self
            .programs
            .iter()
            .map(|p| {
                let name = if p.name.is_empty() {
                    "a program"
                } else {
                    &p.name
                };
                format!("{name} (process {})", p.id)
            })
            .collect();
        let (list, them) = match names.split_last() {
            Some((last, [])) => (last.clone(), "it"),
            Some((last, rest)) => (format!("{} and {last}", rest.join(", ")), "them"),
            None => (String::new(), "it"),
        };
        format!(
            "{} is in use by {list}: close {them}, then delete the folder yourself.",
            self.file.display()
        )
    }
}

/// The file git stopped at deleting `folder`, if programs hold it open. Git walks the folder
/// depth first in the order Windows lists it, and stops at the first entry it can't delete:
/// that is the first entry left, as `read_dir` lists it. A folder that is some program's
/// working directory is not found: the Restart Manager knows only of open files.
#[cfg(windows)]
pub(crate) fn find(folder: &Path) -> Option<InUse> {
    let mut path = folder.to_path_buf();
    loop {
        let first = std::fs::read_dir(&path).ok()?.next()?.ok()?;
        path = first.path();
        if !first.file_type().ok()?.is_dir() {
            break;
        }
    }
    let programs = restart_manager::holders(&path)?;
    if programs.is_empty() {
        return None;
    }
    let file = path.strip_prefix(folder).unwrap_or(&path).to_path_buf();
    Some(InUse { file, programs })
}

#[cfg(not(windows))]
pub(crate) fn find(_folder: &Path) -> Option<InUse> {
    None
}

/// The Restart Manager, which installers ask what holds a file, declared by hand as the app's
/// Windows calls are.
#[cfg(windows)]
#[allow(unsafe_code)]
mod restart_manager {
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;

    use super::Program;

    const ERROR_SUCCESS: u32 = 0;
    const ERROR_MORE_DATA: u32 = 234;
    /// `CCH_RM_SESSION_KEY`, with the terminating null.
    const SESSION_KEY: usize = 33;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct UniqueProcess {
        process_id: u32,
        start_time: [u32; 2],
    }

    /// `RM_PROCESS_INFO`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcessInfo {
        process: UniqueProcess,
        app_name: [u16; 256],
        service_short_name: [u16; 64],
        application_type: i32,
        app_status: u32,
        ts_session_id: u32,
        restartable: i32,
    }

    #[link(name = "rstrtmgr")]
    unsafe extern "system" {
        fn RmStartSession(session: *mut u32, flags: u32, key: *mut u16) -> u32;
        fn RmRegisterResources(
            session: u32,
            files: u32,
            file_names: *const *const u16,
            applications: u32,
            processes: *const UniqueProcess,
            services: u32,
            service_names: *const *const u16,
        ) -> u32;
        fn RmGetList(
            session: u32,
            needed: *mut u32,
            count: *mut u32,
            processes: *mut ProcessInfo,
            reboot_reasons: *mut u32,
        ) -> u32;
        fn RmEndSession(session: u32) -> u32;
    }

    /// The programs holding `file` open, or `None` when Windows can't tell.
    pub fn holders(file: &Path) -> Option<Vec<Program>> {
        let mut session = 0;
        let mut key = [0u16; SESSION_KEY];
        // SAFETY: both point to buffers of the sizes the call writes.
        if unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) } != ERROR_SUCCESS {
            return None;
        }
        let programs = listed(session, file);
        // SAFETY: the session was started above and is ended once.
        unsafe { RmEndSession(session) };
        programs
    }

    fn listed(session: u32, file: &Path) -> Option<Vec<Program>> {
        let name: Vec<u16> = file.as_os_str().encode_wide().chain([0]).collect();
        let names = [name.as_ptr()];
        // SAFETY: one null-terminated UTF-16 name that outlives the call, which copies it.
        let registered = unsafe {
            RmRegisterResources(
                session,
                1,
                names.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        if registered != ERROR_SUCCESS {
            return None;
        }
        let mut infos: Vec<ProcessInfo> = Vec::new();
        // The list may grow between asking its size and reading it: ask again, a few times.
        for _ in 0..4 {
            let mut needed = 0;
            let mut count = u32::try_from(infos.len()).ok()?;
            let mut reasons = 0;
            // SAFETY: `infos` holds `count` entries for the call to fill.
            let result = unsafe {
                RmGetList(
                    session,
                    &mut needed,
                    &mut count,
                    infos.as_mut_ptr(),
                    &mut reasons,
                )
            };
            match result {
                ERROR_SUCCESS => {
                    infos.truncate(count as usize);
                    return Some(infos.iter().map(program).collect());
                }
                ERROR_MORE_DATA => {
                    // SAFETY: `ProcessInfo` is plain data, for which all zeroes is valid.
                    let empty: ProcessInfo = unsafe { std::mem::zeroed() };
                    infos = vec![empty; needed as usize];
                }
                _ => return None,
            }
        }
        None
    }

    fn program(info: &ProcessInfo) -> Program {
        let len = info
            .app_name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.app_name.len());
        Program {
            name: String::from_utf16_lossy(&info.app_name[..len]),
            id: info.process.process_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{InUse, Program};

    fn in_use(programs: &[(&str, u32)]) -> InUse {
        InUse {
            file: PathBuf::from("node_modules").join("x.node"),
            programs: programs
                .iter()
                .map(|&(name, id)| Program {
                    name: name.into(),
                    id,
                })
                .collect(),
        }
    }

    #[test]
    fn one_program_is_named_with_its_process() {
        let text = in_use(&[("Node.js JavaScript Runtime", 13940)]).explain();
        assert!(
            text.ends_with(
                " is in use by Node.js JavaScript Runtime (process 13940): close it, then delete the folder yourself."
            ),
            "{text}"
        );
    }

    #[test]
    fn several_programs_are_listed_and_closed_together() {
        let text = in_use(&[("A", 1), ("", 2), ("C", 3)]).explain();
        assert!(
            text.contains(
                "in use by A (process 1), a program (process 2) and C (process 3): close them,"
            ),
            "{text}"
        );
    }
}
