//! The `agility` feature: the exe exports `D3D12SDKVersion` and
//! `D3D12SDKPath` (see `src/agility.rs`), so the D3D12 runtime loads
//! Microsoft's Agility SDK from a `D3D12` folder next to the exe. Only the
//! Windows 10 build is made with it: a Windows 11 run of an exe with these
//! exports and no such folder fails to make a device (0x887E0003, measured
//! here), so the plain build must stay without them.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let agility = std::env::var_os("CARGO_FEATURE_AGILITY").is_some();
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if agility && msvc {
        // DATA: they are variables the runtime reads, not functions.
        println!("cargo:rustc-link-arg-bins=/EXPORT:D3D12SDKVersion,DATA");
        println!("cargo:rustc-link-arg-bins=/EXPORT:D3D12SDKPath,DATA");
    }
}
