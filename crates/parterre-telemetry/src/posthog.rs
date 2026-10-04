//! PostHog, through its official SDK (`posthog-rs`): all of parterre's PostHog code (#225).
//! Changing services means replacing this module.
//!
//! The SDK batches events on a thread of its own and sends them every few seconds. Every event
//! carries the install ID as its `distinct_id` and is personless (`$process_person_profile:
//! false`), PostHog's anonymous events, and PostHog's standard properties, added in
//! `before_send` as its mobile SDKs add them. Only the channel and git's version get parterre's
//! own names.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use posthog_rs::{Client, ClientOptionsBuilder, Event};

use crate::{Channel, Context, Lifecycle, app_version};

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
    /// The window closes: say so, send what is queued, and tell the sender once done.
    Close(mpsc::Sender<()>),
}

/// The usage statistics' own thread, which talks to PostHog. Dropped, it sends what it has
/// queued and nothing more.
#[derive(Debug)]
pub(crate) struct Sender {
    commands: mpsc::Sender<Command>,
}

impl Sender {
    /// Starts the thread, which sends `events` at once, to `host`.
    pub(crate) fn start(host: &str, context: Context, events: Vec<Lifecycle>) -> Sender {
        let (commands, received) = mpsc::channel();
        let host = host.to_owned();
        let spawned = std::thread::Builder::new()
            .name("usage statistics".into())
            .spawn(move || run(&host, &context, events, &received));
        // Without a thread, nothing is sent: commands go nowhere.
        drop(spawned);
        Sender { commands }
    }

    /// `Application Backgrounded`, then what is queued is sent, waiting at most `wait`.
    pub(crate) fn close(self, wait: Duration) {
        let (done, finished) = mpsc::channel();
        if self.commands.send(Command::Close(done)).is_ok() {
            let _ = finished.recv_timeout(wait);
        }
    }
}

fn run(host: &str, context: &Context, events: Vec<Lifecycle>, commands: &Receiver<Command>) {
    let standard = Standard::of(context);
    let client = client(host, standard, crate::CLOSE);
    for lifecycle in &events {
        client.capture(event(lifecycle, &context.install_id));
    }
    match commands.recv() {
        Ok(Command::Close(done)) => {
            client.capture(event(&Lifecycle::Backgrounded, &context.install_id));
            client.shutdown();
            let _ = done.send(());
        }
        // Stopped: usage statistics unticked.
        Err(_) => client.shutdown(),
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
            Some(event)
        });
    // GeoIP is left on (`disable_geoip` false): the project derives the place, then drops the
    // IP. Options that don't build give a client that sends nothing.
    match options.build() {
        Ok(options) => posthog_rs::client(options),
        Err(_) => posthog_rs::client(""),
    }
}

/// The event for `lifecycle`, from this installation.
fn event(lifecycle: &Lifecycle, install_id: &str) -> Event {
    let mut event = Event::new(lifecycle.name(), install_id);
    if let Lifecycle::Updated { previous_version } = lifecycle {
        let _ = event.insert_prop("previous_version", previous_version);
    }
    event
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
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    const ID: &str = "6f1c2b7e-3a4d-4e8f-9b0a-1c2d3e4f5a6b";

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
        let mut e = event(&Lifecycle::Opened, ID);
        standard().apply(&mut e);
        assert_eq!(e.event_name(), "Application Opened");
        assert_eq!(e.distinct_id(), ID);
        assert_eq!(e.properties()["$process_person_profile"], json!(false));
    }

    #[test]
    fn every_event_carries_the_standard_properties() {
        let mut e = event(&Lifecycle::Installed, ID);
        standard().apply(&mut e);
        let expected = json!({
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
        let mut e = event(&Lifecycle::Opened, ID);
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
                "channel"
            ]
        );
    }

    #[test]
    fn an_update_names_the_previous_version() {
        let updated = Lifecycle::Updated {
            previous_version: "0.5.1".into(),
        };
        let e = event(&updated, ID);
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

    #[test]
    fn a_launch_and_its_close_reach_posthog() {
        let (host, received) = posthog_here();
        let events = crate::launch(false, Some("0.5.1"), "0.6.0 (a1b2c3d)");
        let sender = Sender::start(&host, context(), events);
        sender.close(Duration::from_secs(10));
        let mut sent = Vec::new();
        while let Ok(body) = received.recv_timeout(Duration::from_millis(500)) {
            assert_eq!(body["api_key"], TOKEN);
            sent.extend(body["batch"].as_array().cloned().unwrap_or_default());
        }
        let names: Vec<_> = sent.iter().map(|e| e["event"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "Application Updated",
                "Application Opened",
                "Application Backgrounded"
            ]
        );
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
    }
}
