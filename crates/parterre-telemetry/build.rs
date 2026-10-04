//! Checks the channel a packaging job stamps into the build (`PARTERRE_CHANNEL`, read by
//! `Channel::current`), and passes on the target, which names the release files.

#[path = "src/channel.rs"]
#[allow(dead_code)]
mod channel;

use channel::Channel;

fn main() {
    println!("cargo:rerun-if-env-changed=PARTERRE_CHANNEL");
    if let Some(stamp) = std::env::var("PARTERRE_CHANNEL")
        .ok()
        .filter(|s| !s.is_empty())
        && Channel::stamped(&stamp).is_none()
    {
        let names: Vec<_> = Channel::STAMPED.map(Channel::name).into();
        panic!(
            "PARTERRE_CHANNEL={stamp} is not a channel: expected one of {}",
            names.join(", ")
        );
    }
    let target = std::env::var("TARGET").unwrap();
    println!("cargo:rustc-env=PARTERRE_TARGET={target}");
}
