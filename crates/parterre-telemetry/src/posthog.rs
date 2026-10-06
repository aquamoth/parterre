//! PostHog, through its official SDK (`posthog-rs`): all of parterre's PostHog code (#225).
//! Changing services means replacing this module.
//!
//! The SDK batches events on a thread of its own and sends them every few seconds. Every event
//! carries the install ID as its `distinct_id` and is personless (`$process_person_profile:
//! false`), PostHog's anonymous events, and PostHog's standard properties, added in
//! `before_send` as its mobile SDKs add them. Only the channel and git's version get parterre's
//! own names. Every usage event, the lifecycle and feature events (#264) alike, also carries
//! the properties registered last (`feature.rs`), as PostHog's super properties, added in the
//! usage client's `before_send`.
//!
//! The session (`$session_id`, #262) goes on each usage event as it is made, the feature events
//! included, not in `before_send`: nothing else this client sends, such as a crash report
//! (`$exception`), carries it, as a session would tie it to the install ID's events.
//!
//! Crash reports (#263) go through a client of their own, the SDK's global one, whose panic
//! capture sends a `$exception` the moment a panic happens. They carry the standard properties
//! but never the install ID, the session or the registered properties: the SDK makes them
//! personless with a random ID, and their `before_send` is their own. Home folders in them
//! become `~` there, PostHog's way to keep personal data out of exceptions.

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use posthog_rs::{Client, ClientOptionsBuilder, ErrorTrackingOptionsBuilder, Event};
use serde_json::Value;

use crate::feature::{self, Feature};
use crate::{Channel, Context, Lifecycle, app_version, crash};

/// The parterre project's token. Public by design: it can only send events (#275).
const TOKEN: &str = "phc_rdXXhGJLnbhdtA6tJ5bAXiNzntnob2krZCevNBz5GSES";

/// PostHog Cloud EU's ingestion host (Frankfurt).
pub(crate) const HOST: &str = "https://eu.i.posthog.com";

/// The app, as `$app_name`.
const APP_NAME: &str = "parterre";

/// How long one request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// What the usage statistics thread is told.
enum Command {
    /// The window closes, in session `.0`: say so, send what is queued, and tell the sender once
    /// done.
    Close(String, mpsc::Sender<()>),
    /// A feature was used, in session `.1`.
    Record(Feature, String),
}

/// The usage statistics' own thread, which talks to PostHog. Dropped, it sends what it has
/// queued and nothing more.
#[derive(Debug)]
pub(crate) struct Sender {
    commands: mpsc::Sender<Command>,
}

impl Sender {
    /// Starts the thread, which sends `events` at once, in `session`, to `host`.
    pub(crate) fn start(
        host: &str,
        context: Context,
        session: String,
        events: Vec<Lifecycle>,
    ) -> Sender {
        let (commands, received) = mpsc::channel();
        let host = host.to_owned();
        let spawned = std::thread::Builder::new()
            .name("usage statistics".into())
            .spawn(move || run(&host, &context, (&session, events), &received));
        // Without a thread, nothing is sent: commands go nowhere.
        drop(spawned);
        Sender { commands }
    }

    /// Where features go, each in its session: to the thread, without waiting.
    pub(crate) fn sink(&self) -> impl Fn(Feature, String) + Send + 'static {
        let commands = self.commands.clone();
        move |feature, session| {
            let _ = commands.send(Command::Record(feature, session));
        }
    }

    /// `Application Backgrounded`, in `session`, then what is queued is sent, waiting at most
    /// `wait`.
    pub(crate) fn close(&self, session: String, wait: Duration) {
        let (done, finished) = mpsc::channel();
        if self.commands.send(Command::Close(session, done)).is_ok() {
            let _ = finished.recv_timeout(wait);
        }
    }
}

/// Sends the launch's `events`, in their session, then does as told.
fn run(
    host: &str,
    context: &Context,
    (session, events): (&str, Vec<Lifecycle>),
    commands: &Receiver<Command>,
) {
    let standard = Standard::of(context);
    let client = client(host, standard, crate::CLOSE);
    for lifecycle in &events {
        client.capture(event(lifecycle, &context.install_id, session));
    }
    loop {
        match commands.recv() {
            Ok(Command::Record(feature, session)) => {
                client.capture(feature_event(feature, &context.install_id, &session));
            }
            Ok(Command::Close(session, done)) => {
                let backgrounded = event(&Lifecycle::Backgrounded, &context.install_id, &session);
                client.capture(backgrounded);
                client.shutdown();
                let _ = done.send(());
                return;
            }
            // Stopped: usage statistics unticked.
            Err(_) => {
                client.shutdown();
                return;
            }
        }
    }
}

/// A client for `host`, with the standard properties on every event, which gives up sending
/// on shutdown after `close`.
fn client(host: &str, standard: Standard, close: Duration) -> Client {
    let mut options = ClientOptionsBuilder::default();
    options
        .api_key(TOKEN.to_owned())
        .host(host)
        // A desktop app: `$os` is the user's, not a server's.
        .is_server(false)
        .request_timeout_seconds(REQUEST_TIMEOUT.as_secs())
        .shutdown_timeout_ms(u64::try_from(close.as_millis()).unwrap_or(u64::MAX))
        .before_send(move |mut event| {
            standard.apply(&mut event);
            if let Some(properties) = feature::registered() {
                apply(&properties, &mut event);
            }
            Some(event)
        });
    // GeoIP is left on (`disable_geoip` false): the project derives the place, then drops the
    // IP. Options that don't build give a client that sends nothing.
    match options.build() {
        Ok(options) => posthog_rs::client(options),
        Err(_) => posthog_rs::client(""),
    }
}

/// Turns on the SDK's panic capture for the rest of the process: from now on a panic is sent to
/// `host` as a `$exception` the moment it happens, waiting at most the SDK's 2 seconds. The
/// panic hook installed before it is still called, after it. False if it couldn't be turned
/// on, as when it already is.
pub(crate) fn capture_panics(
    host: &str,
    version: &str,
    git_version: fn() -> Option<String>,
    homes: Vec<String>,
) -> bool {
    let mut standard = Standard::crash(version);
    // Asked on a thread of its own, so that the start doesn't wait for git; a panic before it
    // has answered goes without its version.
    let git = Arc::new(OnceLock::new());
    let asked = Arc::clone(&git);
    let _ = std::thread::Builder::new()
        .name("crash reports".into())
        .spawn(move || asked.set(git_version()));
    let Ok(error_tracking) = ErrorTrackingOptionsBuilder::default()
        .capture_panics(true)
        .build()
    else {
        return false;
    };
    let mut options = ClientOptionsBuilder::default();
    options
        .api_key(TOKEN.to_owned())
        .host(host)
        .is_server(false)
        .request_timeout_seconds(REQUEST_TIMEOUT.as_secs())
        .error_tracking(error_tracking)
        .before_send(move |mut event| {
            if standard.git_version.is_none() {
                standard.git_version = git.get().cloned().flatten();
            }
            crash_report(&mut event, &standard, &homes);
            Some(event)
        });
    let Ok(options) = options.build() else {
        return false;
    };
    posthog_rs::init_global(options).is_ok()
}

/// A panic, as it is sent: with the standard properties, never anything that ties it to this
/// installation, and the `homes` scrubbed from every text in it ([`crash::scrub`]): the panic's
/// message and file, the stack frames' files and `$debug_images[].code_file`.
fn crash_report(event: &mut Event, standard: &Standard, homes: &[String]) {
    standard.apply(event);
    // Sessions belong to the usage statistics, which carry the install ID (#262).
    let _ = event.remove_prop("$session_id");
    let keys: Vec<String> = event.properties().keys().cloned().collect();
    for key in keys {
        if let Some(mut value) = event.remove_prop(&key) {
            scrub(&mut value, homes);
            let _ = event.insert_prop(key, value);
        }
    }
}

/// Every text in `value`, through [`crash::scrub`].
fn scrub(value: &mut Value, homes: &[String]) {
    match value {
        Value::String(text) => *text = crash::scrub(text, homes),
        Value::Array(values) => values.iter_mut().for_each(|v| scrub(v, homes)),
        Value::Object(map) => map.values_mut().for_each(|v| scrub(v, homes)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// The event for `lifecycle`, from this installation, in `session`.
fn event(lifecycle: &Lifecycle, install_id: &str, session: &str) -> Event {
    let mut event = Event::new(lifecycle.name(), install_id);
    let _ = event.insert_prop("$session_id", session);
    if let Lifecycle::Updated { previous_version } = lifecycle {
        let _ = event.insert_prop("previous_version", previous_version);
    }
    event
}

/// The event for `feature`, from this installation, in `session`: its fixed name, and which
/// feature.
fn feature_event(feature: Feature, install_id: &str, session: &str) -> Event {
    let mut event = Event::new(feature.event(), install_id);
    let _ = event.insert_prop("$session_id", session);
    let (key, value) = feature.property();
    let _ = event.insert_prop(key, value);
    event
}

/// The registered `properties`, on `event`.
fn apply(properties: &feature::Properties, event: &mut Event) {
    for (key, value) in properties.pairs() {
        let _ = match value {
            feature::Value::Name(name) => event.insert_prop(key, name),
            feature::Value::Number(number) => event.insert_prop(key, number),
            feature::Value::Count(count) => event.insert_prop(key, count),
            feature::Value::Flag(flag) => event.insert_prop(key, flag),
        };
    }
}

/// PostHog's standard properties, and parterre's own two, as on every event. `$os` and
/// `$os_version` are the SDK's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Standard {
    app_version: String,
    channel: &'static str,
    git_version: Option<String>,
    /// BCP 47, `sv-SE`.
    locale: Option<String>,
    /// IANA, `Europe/Stockholm`.
    timezone: Option<String>,
    /// In points (logical pixels).
    screen: Option<[u32; 2]>,
}

impl Standard {
    /// This launch's, asking git and the system.
    fn of(context: &Context) -> Standard {
        Standard {
            app_version: app_version(&context.version),
            channel: Channel::current().name(),
            git_version: (context.git_version)(),
            locale: sys_locale::get_locale(),
            timezone: iana_time_zone::get_timezone().ok(),
            screen: context
                .screen
                .filter(|s| s.iter().all(|x| x.is_finite() && *x > 0.0))
                .map(|s| s.map(|x| x.round() as u32)),
        }
    }

    /// A crash report's, known at start: not the screen's size, and git's version only later.
    fn crash(version: &str) -> Standard {
        Standard {
            app_version: app_version(version),
            channel: Channel::current().name(),
            git_version: None,
            locale: sys_locale::get_locale(),
            timezone: iana_time_zone::get_timezone().ok(),
            screen: None,
        }
    }

    fn apply(&self, event: &mut Event) {
        // Personless: no person profile, but still counted by its distinct_id.
        let _ = event.insert_prop("$process_person_profile", false);
        let _ = event.insert_prop("$app_name", APP_NAME);
        let _ = event.insert_prop("$app_version", &self.app_version);
        let _ = event.insert_prop("channel", self.channel);
        let optional = [
            ("git_version", &self.git_version),
            ("$locale", &self.locale),
            ("$timezone", &self.timezone),
        ];
        for (key, value) in optional {
            if let Some(value) = value {
                let _ = event.insert_prop(key, value);
            }
        }
        if let Some([width, height]) = self.screen {
            let _ = event.insert_prop("$screen_width", width);
            let _ = event.insert_prop("$screen_height", height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Menu, Screen};
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    const ID: &str = "6f1c2b7e-3a4d-4e8f-9b0a-1c2d3e4f5a6b";
    const SESSION: &str = "0199b0a4-5c00-7000-8000-000000000000";
    const NEXT_SESSION: &str = "0199b0a4-5c01-7000-8000-000000000000";

    fn standard() -> Standard {
        Standard {
            app_version: "0.6.0".into(),
            channel: "deb",
            git_version: Some("2.43.0".into()),
            locale: Some("sv-SE".into()),
            timezone: Some("Europe/Stockholm".into()),
            screen: Some([1920, 1080]),
        }
    }

    fn context() -> Context {
        Context {
            install_id: ID.into(),
            version: "0.6.0 (a1b2c3d)".into(),
            screen: Some([1920.0, 1080.0]),
            git_version: || Some("2.43.0".into()),
        }
    }

    #[test]
    fn the_host_is_posthogs_eu_ingestion() {
        assert_eq!(HOST, posthog_rs::EU_INGESTION_ENDPOINT);
    }

    #[test]
    fn events_are_the_install_ids_and_personless() {
        let mut e = event(&Lifecycle::Opened, ID, SESSION);
        standard().apply(&mut e);
        assert_eq!(e.event_name(), "Application Opened");
        assert_eq!(e.distinct_id(), ID);
        assert_eq!(e.properties()["$process_person_profile"], json!(false));
    }

    #[test]
    fn every_event_carries_the_standard_properties() {
        let mut e = event(&Lifecycle::Installed, ID, SESSION);
        standard().apply(&mut e);
        let expected = json!({
            "$session_id": SESSION,
            "$process_person_profile": false,
            "$app_name": "parterre",
            "$app_version": "0.6.0",
            "channel": "deb",
            "git_version": "2.43.0",
            "$locale": "sv-SE",
            "$timezone": "Europe/Stockholm",
            "$screen_width": 1920,
            "$screen_height": 1080,
        });
        let properties = serde_json::to_value(e.properties()).unwrap();
        assert_eq!(properties, expected);
    }

    #[test]
    fn what_is_unknown_is_left_out() {
        let mut e = event(&Lifecycle::Opened, ID, SESSION);
        let unknown = Standard {
            app_version: "0.6.0".into(),
            channel: "cargo",
            ..Standard::default()
        };
        unknown.apply(&mut e);
        let mut keys: Vec<_> = e.properties().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "$app_name",
                "$app_version",
                "$process_person_profile",
                "$session_id",
                "channel"
            ]
        );
    }

    #[test]
    fn only_usage_events_carry_the_session() {
        // What else the client sends, such as a crash report, gets the standard properties
        // in `before_send`, but no session to tie it to the install ID's events.
        let mut crash = Event::new_anon("$exception");
        standard().apply(&mut crash);
        assert!(!crash.properties().contains_key("$session_id"));
        let e = event(&Lifecycle::Opened, ID, SESSION);
        assert_eq!(e.properties()["$session_id"], json!(SESSION));
    }

    #[test]
    fn an_update_names_the_previous_version() {
        let updated = Lifecycle::Updated {
            previous_version: "0.5.1".into(),
        };
        let e = event(&updated, ID, SESSION);
        assert_eq!(e.event_name(), "Application Updated");
        assert_eq!(e.properties()["previous_version"], json!("0.5.1"));
    }

    #[test]
    fn the_standard_properties_are_this_launchs() {
        let s = Standard::of(&context());
        assert_eq!(s.app_version, "0.6.0");
        assert_eq!(s.channel, Channel::current().name());
        assert_eq!(s.git_version.as_deref(), Some("2.43.0"));
        assert_eq!(s.screen, Some([1920, 1080]));
        let unknown = Context {
            screen: Some([0.0, f32::NAN]),
            git_version: || None,
            ..context()
        };
        let s = Standard::of(&unknown);
        assert_eq!((s.screen, s.git_version), (None, None));
    }

    /// A stand-in for PostHog on this machine: answers every request with 200 and hands
    /// over each body.
    fn posthog_here() -> (String, Receiver<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());
        let (bodies, received) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(n) = lower.strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                let ok = r#"{"status": "Ok"}"#;
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{ok}",
                    ok.len()
                );
                let _ = stream.write_all(answer.as_bytes());
                if let Ok(json) = serde_json::from_slice(&body) {
                    let _ = bodies.send(json);
                }
            }
        });
        (host, received)
    }

    /// The events PostHog was sent, until it has heard nothing for half a second.
    fn sent(received: &Receiver<Value>) -> Vec<Value> {
        let mut sent = Vec::new();
        while let Ok(body) = received.recv_timeout(Duration::from_millis(500)) {
            assert_eq!(body["api_key"], TOKEN);
            sent.extend(body["batch"].as_array().cloned().unwrap_or_default());
        }
        sent
    }

    /// The events PostHog was sent, up to the close's `Application Backgrounded`, waiting at
    /// most 10 seconds: the sending thread may still be starting when [`Usage::close`] stops
    /// waiting for it, as asking Windows for the locale and time zone now and then takes
    /// seconds (#330).
    fn sent_until_closed(received: &Receiver<Value>) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut sent = Vec::new();
        while !sent
            .iter()
            .any(|e: &Value| e["event"] == "Application Backgrounded")
        {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(body) = received.recv_timeout(left) else {
                panic!("not closed: {sent:?}");
            };
            assert_eq!(body["api_key"], TOKEN);
            sent.extend(body["batch"].as_array().cloned().unwrap_or_default());
        }
        sent
    }

    #[test]
    fn a_launch_and_its_close_reach_posthog() {
        let (host, received) = posthog_here();
        let events = crate::launch(false, Some("0.5.1"), "0.6.0 (a1b2c3d)");
        let sender = Sender::start(&host, context(), SESSION.into(), events);
        sender.sink()(Feature::Screen(Screen::Log), SESSION.into());
        // Idle since then: the close is in a session of its own.
        sender.close(NEXT_SESSION.into(), Duration::from_secs(10));
        let sent = sent(&received);
        let names: Vec<_> = sent.iter().map(|e| e["event"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "Application Updated",
                "Application Opened",
                "$screen",
                "Application Backgrounded"
            ]
        );
        assert_eq!(sent[2]["properties"]["$screen_name"], "log");
        for e in &sent {
            let p = &e["properties"];
            assert_eq!(e["distinct_id"], ID);
            assert_eq!(p["$process_person_profile"], false);
            assert_eq!(p["$app_version"], "0.6.0");
            assert_eq!(p["$app_name"], "parterre");
            assert_eq!(p["git_version"], "2.43.0");
            assert_eq!(p["$screen_width"], 1920);
            // The SDK's own.
            assert!(p["$os"].is_string(), "{p}");
            assert!(p["$os_version"].is_string(), "{p}");
            // A desktop app, and GeoIP left on.
            assert!(p.get("$is_server").is_none(), "{p}");
            assert!(p.get("$geoip_disable").is_none(), "{p}");
        }
        assert_eq!(sent[0]["properties"]["previous_version"], "0.5.1");
        let sessions: Vec<_> = sent
            .iter()
            .map(|e| &e["properties"]["$session_id"])
            .collect();
        assert_eq!(sessions, [SESSION, SESSION, SESSION, NEXT_SESSION]);
    }

    /// Every text in `value`.
    fn texts(value: &Value) -> Vec<&str> {
        match value {
            Value::String(text) => vec![text],
            Value::Array(values) => values.iter().flat_map(texts).collect(),
            Value::Object(map) => map.values().flat_map(texts).collect(),
            Value::Null | Value::Bool(_) | Value::Number(_) => Vec::new(),
        }
    }

    #[test]
    fn crash_reports_have_the_home_folder_scrubbed_and_no_session() {
        let mut e = Event::new_anon("$exception");
        let frames = json!([
            {"filename": "/home/x/.cargo/registry/src/egui-0.36.0/src/context.rs", "lineno": 1},
            {"filename": "crates/parterre/src/app.rs", "function": "parterre::app::update"},
            {"filename": "C:\\Users\\x\\.cargo\\registry\\src\\egui\\src\\ui.rs"},
        ]);
        let exception = json!([{
            "type": "Panic",
            "value": "cannot open /home/x/repos/app/.git/index",
            "stacktrace": {"type": "raw", "frames": frames},
        }]);
        e.insert_prop("$exception_list", exception).unwrap();
        let images = json!([{"debug_id": ID, "code_file": "/home/x/.cargo/bin/parterre"}]);
        e.insert_prop("$debug_images", images).unwrap();
        e.insert_prop("$exception_panic_file", "/home/x/.cargo/registry/src/a.rs")
            .unwrap();
        e.insert_prop("$session_id", "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b")
            .unwrap();
        let random = e.distinct_id().to_owned();
        let homes = vec!["/home/x".to_owned(), r"C:\Users\x".to_owned()];
        crash_report(&mut e, &standard(), &homes);
        let p = serde_json::to_value(e.properties()).unwrap();
        let exception = &p["$exception_list"][0];
        assert_eq!(exception["value"], "cannot open ~/repos/app/.git/index");
        let files: Vec<_> = exception["stacktrace"]["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["filename"].as_str().unwrap())
            .collect();
        assert_eq!(
            files,
            [
                "~/.cargo/registry/src/egui-0.36.0/src/context.rs",
                "crates/parterre/src/app.rs",
                r"~\.cargo\registry\src\egui\src\ui.rs"
            ]
        );
        assert_eq!(p["$debug_images"][0]["code_file"], "~/.cargo/bin/parterre");
        assert_eq!(p["$debug_images"][0]["debug_id"], ID);
        assert_eq!(p["$exception_panic_file"], "~/.cargo/registry/src/a.rs");
        // Personless, under the SDK's random ID, and with the standard properties.
        assert!(p.get("$session_id").is_none(), "{p}");
        assert_eq!(e.distinct_id(), random);
        assert_eq!(p["$process_person_profile"], false);
        assert_eq!(p["$app_version"], "0.6.0");
        assert_eq!(p["channel"], "deb");
    }

    #[test]
    fn a_crash_reports_standard_properties_are_known_at_start() {
        let s = Standard::crash("0.6.0 (a1b2c3d)");
        assert_eq!(s.app_version, "0.6.0");
        assert_eq!(s.channel, Channel::current().name());
        assert_eq!((s.git_version, s.screen), (None, None));
    }

    /// Set for the child process [`a_panic_reaches_posthog_without_the_install_id`] starts:
    /// where PostHog's stand-in listens.
    const CRASH_HOST: &str = "PARTERRE_TEST_CRASH_HOST";

    /// What the child process's panic hook, installed before the crash reports, prints.
    const HOOK_BEFORE: &str = "the panic hook from before";

    /// A panic in a process of its own, as the SDK's panic capture is for the whole process
    /// and can be turned on only once.
    #[test]
    fn a_panic_reaches_posthog_without_the_install_id() {
        let exe = std::env::current_exe().unwrap();
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let exe_folder = exe.parent().unwrap().to_string_lossy().into_owned();
        // Stand-ins for the home folder: one in the panic's message, and where the test and
        // its sources are, which the stack frames and `$debug_images` name.
        let homes = vec!["/home/x".to_owned(), workspace.clone(), exe_folder.clone()];
        if let Ok(host) = std::env::var(CRASH_HOST) {
            std::panic::set_hook(Box::new(|_| eprintln!("{HOOK_BEFORE}")));
            // The usage statistics' properties, which a crash report must not carry.
            crate::register(crate::feature::tests::properties());
            assert!(capture_panics(&host, "0.6.0 (a1b2c3d)", || None, homes));
            assert!(!capture_panics(&host, "0.6.0", || None, Vec::new()));
            panic!("cannot open /home/x/repos/app/.git/index");
        }
        let (host, received) = posthog_here();
        let name = "posthog::tests::a_panic_reaches_posthog_without_the_install_id";
        let child = std::process::Command::new(&exe)
            .args([name, "--exact", "--nocapture", "--test-threads=1"])
            .env(CRASH_HOST, &host)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&child.stderr);
        assert!(!child.status.success(), "{stderr}");
        // The hook from before still ran, after the report was sent.
        assert!(stderr.contains(HOOK_BEFORE), "{stderr}");
        let body = received
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("nothing sent: {stderr}"));
        assert_eq!(body["api_key"], TOKEN);
        let sent = body["batch"].as_array().unwrap();
        assert_eq!(sent.len(), 1, "{body}");
        let e = &sent[0];
        let p = &e["properties"];
        assert_eq!(e["event"], "$exception");
        // Personless, with a random ID: not the install ID, nor a session.
        assert_ne!(e["distinct_id"], ID);
        assert!(uuid::Uuid::parse_str(e["distinct_id"].as_str().unwrap()).is_ok());
        assert_eq!(p["$process_person_profile"], false);
        assert!(p.get("$session_id").is_none(), "{p}");
        for (key, _) in crate::feature::tests::properties().pairs() {
            assert!(p.get(key).is_none(), "{key}: {p}");
        }
        assert_eq!(p["$exception_level"], "fatal");
        let exception = &p["$exception_list"][0];
        assert_eq!(exception["type"], "Panic");
        assert_eq!(exception["value"], "cannot open ~/repos/app/.git/index");
        assert_eq!(p["$app_version"], "0.6.0");
        assert_eq!(p["$app_name"], "parterre");
        assert!(p["$os"].is_string(), "{p}");
        assert!(p.get("$is_server").is_none(), "{p}");
        for text in texts(p) {
            assert!(!text.contains(&workspace), "{text}");
            assert!(!text.contains(&exe_folder), "{text}");
            assert!(!text.contains("/home/x"), "{text}");
        }
        if let Some(images) = p["$debug_images"].as_array() {
            for image in images {
                let file = image["code_file"].as_str().unwrap_or("~");
                assert!(!file.contains(&exe_folder), "{file}");
            }
        }
        // Nothing more: one panic, one report.
        assert!(received.recv_timeout(Duration::from_millis(500)).is_err());
    }

    /// Whether a property of an event as sent is one of the fixed set: a feature's name, or
    /// one of the properties every event carries.
    fn is_fixed(key: &str, value: &Value) -> bool {
        let named = |names: Vec<&str>| value.as_str().is_some_and(|v| names.contains(&v));
        let registered = crate::feature::tests::properties().pairs();
        match key {
            "$screen_name" => named(Screen::ALL.iter().map(|s| s.name()).collect()),
            "menu" => named(Menu::ALL.iter().map(|m| m.name()).collect()),
            "action" => named(Action::ALL.iter().map(|a| a.name()).collect()),
            _ => {
                let standard = [
                    "$session_id",
                    "$process_person_profile",
                    "$app_name",
                    "$app_version",
                    "channel",
                    "git_version",
                    "$locale",
                    "$timezone",
                    "$screen_width",
                    "$screen_height",
                ];
                standard.contains(&key)
                    || registered.iter().any(|(k, _)| *k == key)
                    // The SDK's own: `$lib`, `$os` and the like.
                    || key.starts_with("$lib")
                    || key.starts_with("$os")
            }
        }
    }

    /// The facade, [`crate::record`], is the whole process's: one test sends through it.
    #[test]
    fn features_are_sent_while_usage_statistics_are_and_never_otherwise() {
        use crate::{Build, Choices, Usage, record, register};
        const RELEASE: Build = Build {
            send: true,
            debug: false,
        };
        let on = Some(Choices::default());
        let unticked = Some(Choices {
            usage_statistics: false,
            crash_reports: true,
        });
        let (host, received) = posthog_here();
        let start = |build, do_not_track, choices| {
            Usage::start_with(build, do_not_track, &host, choices, context(), Vec::new())
        };
        // Unticked, DO_NOT_TRACK, unanswered, a debug build, a build without `send`.
        let debug = Build {
            send: true,
            debug: true,
        };
        let without_send = Build {
            send: false,
            debug: false,
        };
        let off = [
            (RELEASE, false, unticked),
            (RELEASE, true, on),
            (RELEASE, false, None),
            (debug, false, on),
            (without_send, false, on),
        ];
        for (build, do_not_track, choices) in off {
            let usage = start(build, do_not_track, choices);
            assert!(usage.is_none(), "{build:?} {do_not_track} {choices:?}");
            record(Feature::Action(Action::Fit));
        }
        assert!(sent(&received).is_empty(), "nothing is sent while off");

        // On: what is recorded is sent, with the properties registered on every event.
        register(crate::feature::tests::properties());
        let usage = start(RELEASE, false, on).unwrap();
        record(Feature::Screen(Screen::Merge));
        record(Feature::Menu(Menu::Node));
        record(Feature::Action(Action::Merge));
        usage.close();
        let events = sent_until_closed(&received);
        let features: Vec<_> = events
            .iter()
            .filter_map(|e| {
                let p = &e["properties"];
                let which = ["$screen_name", "menu", "action"]
                    .into_iter()
                    .find_map(|k| p[k].as_str())?;
                Some((e["event"].as_str()?, which))
            })
            .collect();
        for expected in [
            ("$screen", "merge"),
            ("menu_view", "node"),
            ("action_run", "merge"),
        ] {
            assert!(features.contains(&expected), "{expected:?}: {features:?}");
        }
        // All in the session the usage statistics started with, the feature events included.
        let session = events[0]["properties"]["$session_id"].clone();
        assert!(uuid::Uuid::parse_str(session.as_str().unwrap()).is_ok());
        for e in &events {
            let p = e["properties"].as_object().unwrap();
            assert_eq!(e["distinct_id"], ID);
            assert_eq!(p["$session_id"], session, "{e}");
            assert_eq!(p["theme"], "dark");
            assert_eq!(p["graph_mode"], "branchings_and_merges");
            assert_eq!(p["log_layout"], "side_by_side");
            assert_eq!(p["text_size"], 1.25);
            assert_eq!(p["is_auto_reload_on"], true);
            assert_eq!(p["$screen_density"], 1.5);
            assert_eq!(p["repository_count"], 3);
            assert_eq!(p["commit_count_range"], "10000-99999");
            assert_eq!(p["node_count_range"], "100-999");
            for (key, value) in p {
                assert!(is_fixed(key, value), "{key}: {value}");
            }
        }

        // Closed: nothing more.
        record(Feature::Action(Action::Fit));
        assert!(sent(&received).is_empty(), "nothing is sent once closed");

        // Unticked while sending (dropped): nothing more.
        let usage = start(RELEASE, false, on).unwrap();
        drop(usage);
        record(Feature::Action(Action::Fit));
        let after = sent(&received);
        assert!(
            after.iter().all(|e| e["event"] != "action_run"),
            "{after:?}"
        );
    }
}
