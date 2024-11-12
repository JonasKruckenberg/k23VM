use criterion::{black_box, criterion_group, criterion_main, Criterion};
use k23vm::{Engine, Module};
use wasmparser::Validator;

fn compile(bytes: &[u8]) {
    let engine = Engine::default();
    let mut validator = Validator::new();

    let module = Module::from_bytes(&engine, &mut validator, bytes).unwrap();
    black_box(module);
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Translate");
    group.bench_function("compile fib_cpp", |b| {
        let bytes = wat::parse_str(include_str!("../tests/fib_cpp.wat")).unwrap();

        b.iter(|| compile(black_box(&bytes)))
    });
    group.bench_function("translate kiwi-editor", |b| {
        let bytes = wat::parse_str(include_str!("../tests/kiwi-editor.wat")).unwrap();

        b.iter(|| compile(black_box(&bytes)))
    });
    group.bench_function("compile embenchen_fannkuch", |b| {
        b.iter(|| {
            let bytes = wat::parse_str(include_str!("../tests/embenchen_fannkuch.wat")).unwrap();

            compile(black_box(&bytes))
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
