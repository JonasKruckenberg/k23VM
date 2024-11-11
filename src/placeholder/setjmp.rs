#![expect(non_camel_case_types, reason = "FFI types")]
// definitions for jmp_buf and sigjmp_buf types
include!(concat!(env!("OUT_DIR"), "/setjmp.rs"));
