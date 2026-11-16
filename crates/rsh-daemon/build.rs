// The Slint build script is trivial; the workspace `missing_docs`
// lint would flag it.
#![allow(missing_docs)]

fn main() {
    slint_build::compile("ui/tray.slint").expect("tray.slint compiles");
}
