//! Phase 4 integration tests — dynamic-only runtime.
//!
//! ObjC test fixtures live in `tests/objc/*.m`.  Each `#[test]` compiles its
//! fixture on demand and runs it.  Only the tests selected by `cargo test`'s
//! filter get compiled, so `cargo test --test integration -- cache_hit`
//! compiles and runs only `cache_hit.m`.
//!
//! Run with: `cargo test --test integration`

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{env, fs, process::Command};

// ---------------------------------------------------------------------------
// Library discovery
// ---------------------------------------------------------------------------

fn lib_dir() -> Result<&'static str, &'static str> {
    static DIR: OnceLock<Result<String, String>> = OnceLock::new();
    let result = DIR.get_or_init(|| {
        let manifest = env!("CARGO_MANIFEST_DIR");
        ["target/debug", "target/release"]
            .iter()
            .map(|d| format!("{manifest}/{d}"))
            .find(|d| fs::metadata(format!("{d}/libo3jc.so")).is_ok())
            .ok_or_else(|| {
                format!(
                    "\n\
                    libo3jc.so not found in target/debug/ or target/release/.\n\
                    \n\
                    `cargo test` does not build the cdylib — run one of:\n\
                    \n\
                    \x20   cargo build            # debug\n\
                    \x20   cargo build --release   # release\n\
                    \n\
                    then re-run `cargo test --test integration`.\n"
                )
            })
    });
    match result {
        Ok(s) => Ok(s.as_str()),
        Err(e) => Err(e.as_str()),
    }
}

// ---------------------------------------------------------------------------
// Shared output directory for compiled binaries
// ---------------------------------------------------------------------------

fn out_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = env::temp_dir().join(format!("o3jc_fixtures_{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create fixture output dir");
        dir
    })
}

// ---------------------------------------------------------------------------
// Compile + run a single fixture
// ---------------------------------------------------------------------------

/// Compile `tests/objc/{name}.m`, run the binary, and assert stdout.
///
/// Compilation happens per-fixture, so only fixtures selected by cargo's
/// test filter are compiled.  Since `cargo test` runs `#[test]` functions
/// on a thread pool, multiple fixtures compile in parallel automatically.
#[track_caller]
fn run_fixture(name: &str, expected: &str) {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let lib_dir = lib_dir().unwrap_or_else(|msg| panic!("{msg}"));

    let src = format!("{manifest}/tests/objc/{name}.m");
    assert!(
        fs::metadata(&src).is_ok(),
        "fixture tests/objc/{name}.m not found"
    );

    let bin = out_dir().join(name);
    let inc = format!("{manifest}/tests");
    let rpath = format!("-Wl,-rpath,{lib_dir}");

    // Compile.
    let compile = Command::new("clang")
        .args([
            "-fobjc-runtime=gnustep-2.0",
            "-fno-objc-arc",
            "-I",
            &inc,
            "-Wall",
            "-o",
            bin.to_str().unwrap(),
            &src,
            "-L",
            lib_dir,
            "-lo3jc",
            &rpath,
        ])
        .output()
        .expect("spawn clang");

    assert!(
        compile.status.success(),
        "{name}.m: compilation failed:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    // Run.
    let output = Command::new(&bin).output().expect("run fixture");

    assert!(
        output.status.success(),
        "{name}: exited with {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), expected.trim(), "{name}: stdout mismatch");
}

// ---------------------------------------------------------------------------
// Phase 4 tests
// ---------------------------------------------------------------------------

#[test]
fn class_creation() {
    run_fixture("class_creation", "1");
}

#[test]
fn class_invisible() {
    run_fixture("class_invisible", "1\n1");
}

#[test]
fn class_add_method() {
    run_fixture("class_add_method", "1 0");
}

#[test]
fn selector_same() {
    run_fixture("selector_same", "1");
}

#[test]
fn selector_different() {
    run_fixture("selector_different", "1");
}

#[test]
fn msg_lookup_slow() {
    run_fixture("msg_lookup_slow", "1");
}

#[test]
fn imp_returns_self() {
    run_fixture("imp_returns_self", "1");
}

#[test]
fn cache_hit() {
    run_fixture("cache_hit", "1");
}

#[test]
fn unknown_selector() {
    run_fixture("unknown_selector", "1");
}

#[test]
fn null_receiver() {
    run_fixture("null_receiver", "1");
}

#[test]
fn introspection() {
    run_fixture("introspection", "1\n1");
}

#[test]
fn subclass_inherits() {
    run_fixture("subclass_inherits", "1");
}

#[test]
fn method_swizzle() {
    run_fixture("method_swizzle", "1 0");
}

// Phase 5: static class loading
#[test]
fn static_class() {
    run_fixture("static_class", "1");
}

// Phase 6: static class hierarchies
#[test]
fn static_hierarchy() {
    run_fixture("static_hierarchy", "1\n2\n1");
}
