//! Release builds on Windows are GUI-subsystem programs, so they start without a console and
//! `--help`, `--export` messages and errors typed in a terminal would print nothing. Attaching
//! to the parent's console, if there is one, makes them appear there. Before an interactive
//! window opens the console is released again: a process attached to a console is killed when
//! that console closes, and the window should outlive the terminal it was started from.

pub fn attach_parent() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        // Fails harmlessly without a parent console (started from Explorer) or when already
        // attached. Handles redirected to a file or pipe are left as they are.
        win32::AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

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
        // declared `safe`.
        pub safe fn AttachConsole(process_id: u32) -> i32;
        pub safe fn FreeConsole() -> i32;
    }
}
