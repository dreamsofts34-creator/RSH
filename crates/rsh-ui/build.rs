//! Build script: compiles the Slint UI (`ui/main.slint`).

fn main() {
    slint_build::compile("ui/main.slint").unwrap();
}
