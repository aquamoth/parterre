fn main() {
    let target = std::env::var("TARGET").unwrap();
    println!("cargo:rustc-env=PARTERRE_TARGET={target}");
}
