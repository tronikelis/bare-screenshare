fn main() {
    println!("cargo:rustc-link-search=./src/pipewire/");
    println!("cargo:rustc-link-lib=dylib=pipewire-0.3");
    println!("cargo:rustc-link-lib=static=pipewire");
}
