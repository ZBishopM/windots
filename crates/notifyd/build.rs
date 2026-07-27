// Embeds notifyd.exe.manifest as the binary's RT_MANIFEST resource.
//
// The <msix> element in that manifest is what links the exe back to the sparse
// package. Without it the package registers cleanly and the process still has no
// package identity, which is the one failure mode this whole daemon is built to
// avoid being invisible.
//
// Done with raw linker flags rather than a resource crate (embed-resource,
// winresource): this is two MSVC switches, and the workspace has enough
// dependencies. /MANIFEST:EMBED makes link.exe generate and embed the manifest;
// /MANIFESTINPUT merges our file into it. /MANIFESTUAC is left at its default,
// which supplies the asInvoker trustInfo, so notifyd.exe.manifest must not carry
// one of its own.

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let manifest = std::path::Path::new(&dir).join("notifyd.exe.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed=build.rs");

    // MinGW's ld does not take these switches; the rice builds MSVC, but a
    // wrong-toolchain build should fail to link loudly rather than silently
    // produce an identity-less exe, so it is gated and reported.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    } else {
        println!(
            "cargo:warning=notifyd built without the MSVC linker: no <msix> manifest is embedded, \
             so the process will have NO package identity and the listener will return nothing"
        );
    }
}
