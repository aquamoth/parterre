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
        panic!(
            "PARTERRE_CHANNEL={stamp} is not a channel: expected one of {}",
            Channel::STAMPED.map(Channel::name).join(", ")
        );
    }
    let target = std::env::var("TARGET").unwrap();
    println!("cargo:rustc-env=PARTERRE_TARGET={target}");
}
