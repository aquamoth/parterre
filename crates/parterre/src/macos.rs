//! What parterre does with macOS itself, through AppKit: folders opened from Finder (dropped on
//! the Dock icon, *Open With*, #336, and the *Revision Graph* service, #337), the clipboard for
//! the menu bar's Paste, *Install Command Line Tool…* (#341), the input method's indicator
//! (#342), and the About panel (#351). The menu bar is `app::menu_bar::macos`.

// AppKit's API is Objective-C, called through objc2: every call is `unsafe` to Rust.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Mutex;

use eframe::egui;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationVersion, NSAboutPanelOptionCredits, NSAboutPanelOptionVersion,
    NSApplication, NSApplicationWillFinishLaunchingNotification,
    NSAttributedStringAppKitDocumentFormats, NSPasteboard, NSPasteboardTypeFileURL,
    NSPasteboardTypeString,
};
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSAttributedString, NSData, NSDictionary,
    NSNotification, NSNotificationCenter, NSObjectProtocol, NSString, NSURL,
};

/// Folders Finder asked parterre to open, not taken yet.
static OPENED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

thread_local! {
    /// The handler of Finder's requests, alive while parterre runs.
    static HANDLER: RefCell<Option<Retained<Opener>>> = const { RefCell::new(None) };
    /// For a repaint once a folder comes in.
    static CONTEXT: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// Apple event codes (four-character codes) of the open-documents event.
const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and Opener has no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ParterreOpener"]
    struct Opener;

    impl Opener {
        /// As NSApplication's own handlers are in: from here on Finder's requests are parterre's.
        #[unsafe(method(willFinishLaunching:))]
        fn will_finish_launching(&self, _notification: &NSNotification) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let manager = NSAppleEventManager::sharedAppleEventManager();
            // SAFETY: the selector is this class's, taking two descriptors; the codes are
            // FourCharCodes. Sent directly, as objc2-foundation has it only with all of
            // CoreServices.
            unsafe {
                let _: () = msg_send![
                    &*manager,
                    setEventHandler: self,
                    andSelector: sel!(handleOpenDocuments:withReplyEvent:),
                    forEventClass: CORE_EVENT_CLASS,
                    andEventID: OPEN_DOCUMENTS
                ];
            }
            let app = NSApplication::sharedApplication(mtm);
            // SAFETY: this object answers the service's message, revisionGraph:userData:error:.
            unsafe { app.setServicesProvider(Some(self)) };
        }

        /// A folder dropped on the Dock icon, or opened with *Open With*.
        #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
        fn handle_open_documents(
            &self,
            event: &NSAppleEventDescriptor,
            _reply: &NSAppleEventDescriptor,
        ) {
            // SAFETY: the keyword is a FourCharCode.
            let list: Option<Retained<NSAppleEventDescriptor>> =
                unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
            let Some(list) = list else {
                return;
            };
            let paths = (1..=list.numberOfItems())
                .filter_map(|i| list.descriptorAtIndex(i))
                .filter_map(|d| d.fileURLValue())
                .filter_map(|url| url.path())
                .map(|p| PathBuf::from(p.to_string()));
            opened(paths);
        }

        /// The *Revision Graph* service, on folders chosen in Finder (`NSServices` in
        /// Info.plist).
        #[unsafe(method(revisionGraph:userData:error:))]
        fn revision_graph(
            &self,
            pasteboard: &NSPasteboard,
            _user_data: Option<&NSString>,
            _error: *mut *mut NSString,
        ) {
            let items = pasteboard.pasteboardItems().map(|items| items.to_vec());
            let paths = items.unwrap_or_default().into_iter().filter_map(|item| {
                // SAFETY: an AppKit constant.
                let url = item.stringForType(unsafe { NSPasteboardTypeFileURL })?;
                let url = NSURL::URLWithString(&url)?;
                Some(PathBuf::from(url.path()?.to_string()))
            });
            opened(paths);
        }
    }

    unsafe impl NSObjectProtocol for Opener {}
);

impl Opener {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}

/// Queues `paths` for the window, and wakes it.
fn opened(paths: impl Iterator<Item = PathBuf>) {
    let mut queued = OPENED.lock().unwrap_or_else(|e| e.into_inner());
    queued.extend(paths);
    drop(queued);
    CONTEXT.with_borrow(|ctx| {
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    });
}

/// Takes Finder's requests to open folders from the start: call before the window is made.
/// AppKit asks before parterre's first frame when it starts parterre to open a folder.
pub fn listen() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let opener = Opener::new(mtm);
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: the selector is Opener's, taking the notification; the observer lives on in
    // HANDLER.
    unsafe {
        center.addObserver_selector_name_object(
            &opener,
            sel!(willFinishLaunching:),
            Some(NSApplicationWillFinishLaunchingNotification),
            None,
        );
    }
    HANDLER.with_borrow_mut(|h| *h = Some(opener));
}

/// The folders Finder asked parterre to open since the last call. `ctx` is woken when more
/// come.
pub fn take_opened(ctx: &egui::Context) -> Vec<PathBuf> {
    CONTEXT.with_borrow_mut(|c| {
        if c.is_none() {
            *c = Some(ctx.clone());
        }
    });
    std::mem::take(&mut *OPENED.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The text on the clipboard.
pub fn clipboard_text() -> Option<String> {
    let pasteboard = NSPasteboard::generalPasteboard();
    // SAFETY: an AppKit constant.
    let text = pasteboard.stringForType(unsafe { NSPasteboardTypeString })?;
    Some(text.to_string())
}

/// The input method's indicator, the bubble macOS shows at the text cursor (#342): only while a
/// text field has the keyboard. Otherwise the window's input context is deactivated, which
/// AppKit keeps active for any view taking text, as winit's does whether or not a field has the
/// keyboard, and which shows the indicator where the last text field was.
#[derive(Debug, Default)]
pub struct InputMethod {
    active: Option<bool>,
}

impl InputMethod {
    pub fn update(&mut self, frame: &eframe::Frame, text_focus: bool) {
        if self.active == Some(text_focus) {
            return;
        }
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = frame.window_handle() else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        self.active = Some(text_focus);
        // SAFETY: the handle's view is the window's NSView, alive while the window is.
        let view: &objc2_app_kit::NSView = unsafe { handle.ns_view.cast().as_ref() };
        if let Some(context) = view.inputContext() {
            if text_focus {
                context.activate();
            } else {
                context.discardMarkedText();
                context.deactivate();
            }
        }
    }
}

pub mod cli {
    //! *Install Command Line Tool…* (#341): `/usr/local/bin/parterre`, a link to the program in
    //! parterre.app, so that a new copy of the app keeps it working. Asks for an administrator's
    //! password, through macOS's own prompt, when `/usr/local/bin` can't be written.

    use std::path::Path;
    use std::sync::mpsc::{Receiver, channel};

    /// Where the link goes.
    pub const LINK: &str = "/usr/local/bin/parterre";

    /// Starts installing the link to the running program; the answer comes on the receiver,
    /// what to say in the status bar, or why it failed.
    pub fn install() -> Receiver<Result<String, String>> {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(install_now());
        });
        rx
    }

    fn install_now() -> Result<String, String> {
        let program = std::env::current_exe()
            .map_err(|e| format!("Could not tell where parterre is: {e}"))?;
        let program = program.canonicalize().unwrap_or(program);
        let link = Path::new(LINK);
        match std::fs::symlink_metadata(link) {
            Ok(meta) if !meta.file_type().is_symlink() => {
                return Err(format!(
                    "{LINK} is there already, and isn't a link: remove it first"
                ));
            }
            Ok(_) if std::fs::read_link(link).ok().as_deref() == Some(program.as_path()) => {
                return Ok(format!("{LINK} is in place: type parterre in a terminal"));
            }
            _ => {}
        }
        if let Err(e) = link_as_user(&program, link) {
            if e.kind() != std::io::ErrorKind::PermissionDenied
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(format!("Could not make {LINK}: {e}"));
            }
            link_as_administrator(&program)?;
        }
        Ok(format!("Installed {LINK}: type parterre in a terminal"))
    }

    fn link_as_user(program: &Path, link: &Path) -> std::io::Result<()> {
        let parent = link.parent().unwrap_or(Path::new("/"));
        if !parent.is_dir() {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        let _ = std::fs::remove_file(link);
        std::os::unix::fs::symlink(program, link)
    }

    /// The same, as an administrator: macOS asks for the password.
    fn link_as_administrator(program: &Path) -> Result<(), String> {
        const SCRIPT: &str = r#"on run argv
    do shell script "mkdir -p /usr/local/bin && ln -sfn " & quoted form of item 1 of argv & " /usr/local/bin/parterre" with prompt "parterre wants to put its command line tool in /usr/local/bin." with administrator privileges
end run"#;
        let output = std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(SCRIPT)
            .arg(program)
            .output()
            .map_err(|e| format!("Could not ask for a password: {e}"))?;
        if output.status.success() {
            return Ok(());
        }
        let message = String::from_utf8_lossy(&output.stderr);
        // -128: the user cancelled.
        if message.contains("-128") {
            Err("Not installed: cancelled".to_owned())
        } else {
            Err(format!("Could not make {LINK}: {}", message.trim()))
        }
    }
}

/// *About parterre*: macOS's own About panel (#351). AppKit takes the name, the icon and the
/// copyright from the app bundle (Info.plist's `NSHumanReadableCopyright`, which
/// `packaging/macos/build-dmg.sh` fills in from NOTICE); parterre gives the version and the
/// credits under it.
pub fn about_panel() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let (version, commit) = crate::about::version_and_commit(crate::VERSION);
    let notices = crate::about::third_party_notices()
        .and_then(NSURL::from_file_path)
        .and_then(|url| url.absoluteString())
        .map(|url| url.to_string());
    let html = crate::about::credits_html(notices.as_deref());
    let data = NSData::with_bytes(html.as_bytes());
    // SAFETY: HTML in UTF-8 (its meta tag says so), and no document attributes asked for.
    let credits = unsafe {
        NSAttributedString::initWithHTML_documentAttributes(
            NSAttributedString::alloc(),
            &data,
            None,
        )
    };
    let mut keys = Vec::new();
    let mut values: Vec<Retained<AnyObject>> = Vec::new();
    // SAFETY: AppKit's constants.
    unsafe {
        keys.push(NSAboutPanelOptionApplicationVersion);
        values.push(Retained::into_super(Retained::into_super(
            NSString::from_str(version),
        )));
        // Without one, the panel shows CFBundleVersion, which is the version again.
        keys.push(NSAboutPanelOptionVersion);
        values.push(Retained::into_super(Retained::into_super(
            NSString::from_str(commit),
        )));
        if let Some(credits) = credits {
            keys.push(NSAboutPanelOptionCredits);
            values.push(Retained::into_super(Retained::into_super(credits)));
        }
    }
    let options = NSDictionary::from_retained_objects(&keys, &values);
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: the keys are AppKit's, each with the type it documents.
    unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
}
