//! Console output for the Windows release build.
//!
//! Release builds on Windows are GUI-subsystem programs, so they start without a console and
//! `--help`, `--export` messages and errors typed in a terminal would print nothing. Attaching
//! to the parent's console (if there is one) makes them appear there. Before an interactive
//! window opens the console is released again: a process attached to a console is killed when
//! that console closes, and the window should outlive the terminal it was started from.
//!
//! This and `reveal` are the places on Windows that use `unsafe` (to declare a few Win32
//! functions), as `macos` and the macOS menu bar are on macOS (AppKit through objc2);
//! everything else keeps `unsafe_code` denied.

/// Attaches to the console of the process that started us, if it has one.
pub fn attach_parent() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        // Fails harmlessly when started from Explorer (no parent console) or when already
        // attached. Handles redirected to a file or pipe are left as they are.
        win32::AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Releases the console attached by [`attach_parent`], so closing the terminal doesn't close
/// the window.
pub fn detach() {
    #[cfg(all(windows, not(debug_assertions)))]
    win32::FreeConsole();
}

#[cfg(all(windows, not(debug_assertions)))]
#[allow(unsafe_code)]
mod win32 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        // Both take plain integers (or nothing) and touch no memory of ours, so they are
        // declared `safe`; the return value (success flag) is not needed.
        pub safe fn AttachConsole(process_id: u32) -> i32;
        pub safe fn FreeConsole() -> i32;
    }
}
