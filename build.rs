use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");

    // Generate rust definitions for the jmp_buf and sigjmp_buf
    // types. Do NOT generate bindings for the function declarations;
    // that's done below.
    let bindings = bindgen::Builder::default()
        .header("src/placeholder/setjmp-wrapper.h")
        // we only want these two type definitions
        .allowlist_type("jmp_buf")
        .allowlist_function("setjmp")
        .allowlist_function("longjmp")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/jmpbuf.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("setjmp.rs"))
        .expect("Couldn't write bindings!");
}
