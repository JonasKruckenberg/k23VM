#![allow(non_camel_case_types)]
// definitions for jmp_buf and sigjmp_buf types
include!(concat!(env!("OUT_DIR"), "/setjmp.rs"));