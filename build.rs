//! Build the vendored libxml2 and the HTML/XPath shim.
//!
//! Python ddgs parses HTML with lxml, and lxml 6.1.3 bundles libxml2 2.14.6.
//! For 1:1 parity we build exactly that libxml2 (git submodule
//! `vendor/libxml2`, tag v2.14.6) as a static library, with only the modules
//! the parser and XPath need. `csrc/html_shim.c` reproduces lxml's parser
//! setup on top of it.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let source = manifest.join("vendor").join("libxml2");
    if !source.join("CMakeLists.txt").exists() {
        panic!(
            "vendor/libxml2 is missing. Run `git submodule update --init vendor/libxml2` \
             (libxml2 v2.14.6, the version bundled by lxml 6.1.3)."
        );
    }

    let on = "ON";
    let off = "OFF";
    let dst = cmake::Config::new(&source)
        .profile("Release")
        .define("BUILD_SHARED_LIBS", off)
        // Needed by the parser and XPath.
        .define("LIBXML2_WITH_HTML", on)
        .define("LIBXML2_WITH_XPATH", on)
        .define("LIBXML2_WITH_THREADS", on)
        .define("LIBXML2_WITH_SAX1", on)
        .define("LIBXML2_WITH_PUSH", on)
        .define("LIBXML2_WITH_OUTPUT", on)
        .define("LIBXML2_WITH_ISO8859X", on)
        // Not needed: no external libraries, no tools, no network.
        .define("LIBXML2_WITH_ICONV", off)
        .define("LIBXML2_WITH_ICU", off)
        .define("LIBXML2_WITH_ZLIB", off)
        .define("LIBXML2_WITH_LZMA", off)
        .define("LIBXML2_WITH_HTTP", off)
        .define("LIBXML2_WITH_PYTHON", off)
        .define("LIBXML2_WITH_PROGRAMS", off)
        .define("LIBXML2_WITH_TESTS", off)
        .define("LIBXML2_WITH_MODULES", off)
        .define("LIBXML2_WITH_READLINE", off)
        .define("LIBXML2_WITH_HISTORY", off)
        .define("LIBXML2_WITH_CATALOG", off)
        .define("LIBXML2_WITH_DEBUG", off)
        .define("LIBXML2_WITH_XINCLUDE", off)
        .define("LIBXML2_WITH_XPTR", off)
        .define("LIBXML2_WITH_C14N", off)
        .define("LIBXML2_WITH_SCHEMAS", off)
        .define("LIBXML2_WITH_SCHEMATRON", off)
        .define("LIBXML2_WITH_RELAXNG", off)
        .define("LIBXML2_WITH_READER", off)
        .define("LIBXML2_WITH_WRITER", off)
        .define("LIBXML2_WITH_VALID", off)
        .define("LIBXML2_WITH_PATTERN", off)
        .define("LIBXML2_WITH_REGEXPS", off)
        .define("LIBXML2_WITH_LEGACY", off)
        // Build only the library: `install` would also need doc/ and the man
        // pages, which the published crate does not ship.
        .build_target("LibXml2")
        .build();
    let build_dir = dst.join("build");
    let target = env::var("TARGET").unwrap_or_default();

    let mut shim = cc::Build::new();
    shim.file(manifest.join("csrc").join("html_shim.c"))
        // Generated `libxml/xmlversion.h` and `config.h` live in the build
        // directory; the other headers in the source tree.
        .include(&build_dir)
        .include(source.join("include"))
        .warnings(true);
    if target.contains("windows") {
        shim.define("LIBXML_STATIC", None);
    }
    shim.compile("websearch_html_shim");

    // Link libxml2 after the shim: static link order matters on Linux.
    println!("cargo:rustc-link-search=native={}", build_dir.display());
    // Multi-config generators (MSVC) put the library in a per-profile folder.
    println!(
        "cargo:rustc-link-search=native={}",
        build_dir.join("Release").display()
    );
    if target.contains("msvc") {
        // CMake's static MSVC build is named libxml2s.lib.
        println!("cargo:rustc-link-lib=static=libxml2s");
        println!("cargo:rustc-link-lib=bcrypt");
    } else {
        println!("cargo:rustc-link-lib=static=xml2");
        if !target.contains("windows") {
            println!("cargo:rustc-link-lib=m");
        }
    }

    println!("cargo:rerun-if-changed=csrc/html_shim.c");
    println!("cargo:rerun-if-changed=vendor/libxml2/CMakeLists.txt");
    println!("cargo:rerun-if-changed=build.rs");
}
