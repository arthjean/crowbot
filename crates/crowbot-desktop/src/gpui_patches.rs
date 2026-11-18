//! The GPUI crates this repository changes, in `patches/`, must follow the
//! rev of Zed the client pins: a crate of another rev would mix two GPUIs.
//! `scripts/gpui-patches.sh check` proves the full claim (the copy is
//! upstream plus its diff); these tests catch a rev bump that forgot them.

use std::path::Path;

fn manifest(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

/// Every rev of `zed-industries/zed` that a manifest names.
fn zed_revs(manifest: &str) -> Vec<&str> {
    manifest
        .lines()
        .filter(|line| line.contains("github.com/zed-industries/zed\""))
        .filter_map(|line| line.split("rev = \"").nth(1))
        .filter_map(|rest| rest.split('"').next())
        .collect()
}

#[test]
fn a_patched_crate_takes_the_rev_the_client_pins() {
    let client = manifest("Cargo.toml");
    let pinned = zed_revs(&client);
    assert!(!pinned.is_empty());
    assert!(pinned.iter().all(|rev| *rev == pinned[0]), "{pinned:?}");

    let patched = manifest("patches/gpui_wgpu/Cargo.toml");
    let revs = zed_revs(&patched);
    assert!(!revs.is_empty(), "patches/gpui_wgpu names no Zed rev");
    for rev in revs {
        assert_eq!(rev, pinned[0], "patches/gpui_wgpu/Cargo.toml");
    }
}

#[test]
fn the_client_takes_the_patched_crate() {
    let client = manifest("Cargo.toml");
    let section = client
        .split("[patch.\"https://github.com/zed-industries/zed\"]")
        .nth(1)
        .unwrap();
    assert!(section.contains("gpui_wgpu = { path = \"patches/gpui_wgpu\" }"));
    assert!(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("patches/gpui_wgpu.diff")
            .is_file()
    );
}
