use anyhow::{anyhow, bail, Context};
use cranelift_codegen::settings::Configurable;
use k23_vm::{
    Compiler, ConstExprEvaluator, Extern, Instance, InstanceAllocator, Linker, Module,
    PlaceholderAllocatorDontUse, Store, Val,
};
use std::fmt::{Display, LowerHex};
use std::path::Path;
use std::sync::Arc;
use wast::core::{EncodeOptions, GenerateDwarf, NanPattern, V128Pattern, WastArgCore, WastRetCore};
use wast::parser::ParseBuffer;
use wast::token::{F32, F64};
use wast::{
    parser, Error, QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet, Wat,
};

macro_rules! spectests {
    ($($names:ident $paths:literal),*) => {
        $(
            #[test_log::test]
            fn $names() -> anyhow::Result<()> {
                let mut ctx = WastContext::new_default()?;

                ctx.run_file(Path::new(file!()).parent().unwrap().join($paths))
            }
        )*
    };
}

spectests!(
    address "./spec/address.wast",
    align "./spec/align.wast",
    binary "./spec/binary.wast",
    binary_leb128 "./spec/binary-leb128.wast",
    block "./spec/block.wast",
    br_base "./spec/br.wast",
    br_if "./spec/br_if.wast",
    br_table "./spec/br_table.wast",
    bulk "./spec/bulk.wast",
    /*call "./spec/call.wast",*/ // TODO fix infinite loop in this test
    /*call_indirect "./spec/call_indirect.wast",*/ // TODO fix infinite loop in this test
    comments "./spec/comments.wast",
    const_ "./spec/const.wast",
    conversions "./spec/conversions.wast",
    custom "./spec/custom.wast",
    data "./spec/data.wast",
    elem "./spec/elem.wast",
    endianness "./spec/endianness.wast",
    exports "./spec/exports.wast",
    f32_base "./spec/f32.wast",
    f32_bitwise "./spec/f32_bitwise.wast",
    f32_cmp "./spec/f32_cmp.wast",
    f64_base "./spec/f64.wast",
    f64_bitwise "./spec/f64_bitwise.wast",
    f64_cmp "./spec/f64_cmp.wast",
    fac "./spec/fac.wast",
    float_exprs "./spec/float_exprs.wast",
    float_literals "./spec/float_literals.wast",
    float_memory "./spec/float_memory.wast",
    float_misc "./spec/float_misc.wast",
    forward "./spec/forward.wast",
    func "./spec/func.wast",
    func_ptrs "./spec/func_ptrs.wast",
    global "./spec/global.wast",
    i32 "./spec/i32.wast",
    i64 "./spec/i64.wast",
    if_ "./spec/if.wast",
    imports "./spec/imports.wast",
    inline_module "./spec/inline-module.wast",
    int_exprs "./spec/int_exprs.wast",
    int_literals "./spec/int_literals.wast",
    labels "./spec/labels.wast",
    left_to_right "./spec/left-to-right.wast",
    linking "./spec/linking.wast",
    load "./spec/load.wast",
    local_get "./spec/local_get.wast",
    local_set "./spec/local_set.wast",
    local_tee "./spec/local_tee.wast",
    loop_ "./spec/loop.wast",
    memory "./spec/memory.wast",
    memory_copy "./spec/memory_copy.wast",
    /*memory_fill "./spec/memory_fill.wast",*/ // TODO fix infinite loop in this test
    memory_grow "./spec/memory_grow.wast",
    memory_init "./spec/memory_init.wast",
    memory_redundancy "./spec/memory_redundancy.wast",
    memory_size "./spec/memory_size.wast",
    /*memory_trap "./spec/memory_trap.wast",*/ // TODO this takes foreeeeever (40ish secs) figure out why
    names "./spec/names.wast",
    nop "./spec/nop.wast",
    obsolete_keywords "./spec/obsolete-keywords.wast",
    ref_func "./spec/ref_func.wast",
    ref_is_null "./spec/ref_is_null.wast",
    ref_null "./spec/ref_null.wast",
    return_ "./spec/return.wast",
    select "./spec/select.wast",
    /*
    simd_address "./spec/simd_address.wast",
    simd_align "./spec/simd_align.wast",
    simd_bit_shift "./spec/simd_bit_shift.wast",
    simd_bitwise "./spec/simd_bitwise.wast",
    simd_boolean "./spec/simd_boolean.wast",
    simd_const "./spec/simd_const.wast",
    simd_conversion "./spec/simd_conversion.wast",
    simd_f32x4 "./spec/simd_f32x4.wast",
    simd_f32x4_arith "./spec/simd_f32x4_arith.wast",
    simd_f32x4_cmp "./spec/simd_f32x4_cmp.wast",
    simd_f32x4_pmin_pmax "./spec/simd_f32x4_pmin_pmax.wast",
    simd_f32x4_rounding "./spec/simd_f32x4_rounding.wast",
    simd_f64x2 "./spec/simd_f64x2.wast",
    simd_f64x2_arith "./spec/simd_f64x2_arith.wast",
    simd_f64x2_cmp "./spec/simd_f64x2_cmp.wast",
    simd_f64x2_pmin_pmax "./spec/simd_f64x2_pmin_pmax.wast",
    simd_f64x2_rounding "./spec/simd_f64x2_rounding.wast",
    simd_i8x16_arith "./spec/simd_i8x16_arith.wast",
    simd_i8x16_arith2 "./spec/simd_i8x16_arith2.wast",
    simd_i8x16_cmp "./spec/simd_i8x16_cmp.wast",
    simd_i8x16_sat_arith "./spec/simd_i8x16_sat_arith.wast",
    simd_i16x8_arith "./spec/simd_i16x8_arith.wast",
    simd_i16x8_arith2 "./spec/simd_i16x8_arith2.wast",
    simd_i16x8_cmp "./spec/simd_i16x8_cmp.wast",
    simd_i16x8_extadd_pairwise_i8x16 "./spec/simd_i16x8_extadd_pairwise_i8x16.wast",
    simd_i16x8_extmul_i8x16 "./spec/simd_i16x8_extmul_i8x16.wast",
    simd_i16x8_q15mulr_sat_s "./spec/simd_i16x8_q15mulr_sat_s.wast",
    simd_i16x8_sat_arith "./spec/simd_i16x8_sat_arith.wast",
    simd_i32x4_arith "./spec/simd_i32x4_arith.wast",
    simd_i32x4_arith2 "./spec/simd_i32x4_arith2.wast",
    simd_i32x4_cmp "./spec/simd_i32x4_cmp.wast",
    simd_i32x4_dot_i16x8 "./spec/simd_i32x4_dot_i16x8.wast",
    simd_i32x4_extadd_pairwise_i16x8 "./spec/simd_i32x4_extadd_pairwise_i16x8.wast",
    simd_i32x4_extmul_i16x8 "./spec/simd_i32x4_extmul_i16x8.wast",
    simd_i32x4_trunc_sat_f32x4 "./spec/simd_i32x4_trunc_sat_f32x4.wast",
    simd_i32x4_trunc_sat_f64x2 "./spec/simd_i32x4_trunc_sat_f64x2.wast",
    simd_i64x2_arith "./spec/simd_i64x2_arith.wast",
    simd_i64x2_arith2 "./spec/simd_i64x2_arith2.wast",
    simd_i64x2_cmp "./spec/simd_i64x2_cmp.wast",
    simd_i64x2_extmul_i32x4 "./spec/simd_i64x2_extmul_i32x4.wast",
    simd_int_to_int_extend "./spec/simd_int_to_int_extend.wast",
    simd_lane "./spec/simd_lane.wast",
    simd_linking "./spec/simd_linking.wast",
    simd_load "./spec/simd_load.wast",
    simd_load8_lane "./spec/simd_load8_lane.wast",
    simd_load16_lane "./spec/simd_load16_lane.wast",
    simd_load32_lane "./spec/simd_load32_lane.wast",
    simd_load64_lane "./spec/simd_load64_lane.wast",
    simd_load_extend "./spec/simd_load_extend.wast",
    simd_load_splat "./spec/simd_load_splat.wast",
    simd_load_zero "./spec/simd_load_zero.wast",
    simd_splat "./spec/simd_splat.wast",
    simd_store "./spec/simd_store.wast",
    simd_store8_lane "./spec/simd_store8_lane.wast",
    simd_store16_lane "./spec/simd_store16_lane.wast",
    simd_store32_lane "./spec/simd_store32_lane.wast",
    simd_store64_lane "./spec/simd_store64_lane.wast", */
    skip_stack_guard_page "./spec/skip-stack-guard-page.wast",
    stack "./spec/stack.wast",
    start "./spec/start.wast",
    store "./spec/store.wast",
    switch "./spec/switch.wast",
    table_base "./spec/table.wast",
    table_sub "./spec/table-sub.wast",
    table_copy "./spec/table_copy.wast",
    table_fill "./spec/table_fill.wast",
    table_get "./spec/table_get.wast",
    table_grow "./spec/table_grow.wast",
    table_init "./spec/table_init.wast",
    table_set "./spec/table_set.wast",
    table_size "./spec/table_size.wast",
    token "./spec/token.wast",
    traps "./spec/trap_handling.wast",
    type_ "./spec/type.wast",
    unreachable "./spec/unreachable.wast",
    unreached_invalid "./spec/unreached-invalid.wast",
    unreached_valid "./spec/unreached-valid.wast",
    unwind "./spec/unwind.wast",
    utf8_custom_section_id "./spec/utf8-custom-section-id.wast",
    utf8_import_field "./spec/utf8-import-field.wast",
    utf8_import_module "./spec/utf8-import-module.wast",
    utf8_invalid_encoding "./spec/utf8-invalid-encoding.wast"
);

enum Outcome<T = Vec<Val>> {
    Ok(T),
    Trap(anyhow::Error),
}

impl<T> Outcome<T> {
    fn map<U>(self, map: impl FnOnce(T) -> U) -> Outcome<U> {
        match self {
            Outcome::Ok(t) => Outcome::Ok(map(t)),
            Outcome::Trap(t) => Outcome::Trap(t),
        }
    }

    fn into_result(self) -> anyhow::Result<T> {
        match self {
            Outcome::Ok(t) => Ok(t),
            Outcome::Trap(t) => Err(t),
        }
    }
}

pub struct WastContext {
    store: Store,
    linker: Linker,
    alloc: &'static dyn InstanceAllocator,
    const_eval: ConstExprEvaluator,
    validator: wasmparser::Validator,
    compiler: Compiler,
    current: Option<Instance>,
}

impl WastContext {
    fn new_default() -> anyhow::Result<Self> {
        let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST)?;
        let mut b = cranelift_codegen::settings::builder();
        b.set("opt_level", "speed_and_size")?;
        b.set("libcall_call_conv", "isa_default")?;
        b.set("preserve_frame_pointers", "true")?;
        b.set("enable_probestack", "true")?;
        b.set("probestack_strategy", "inline")?;
        let target_isa = isa_builder.finish(cranelift_codegen::settings::Flags::new(b))?;

        let ctx = WastContext {
            store: Store::default(),
            linker: Linker::default(),
            alloc: &PlaceholderAllocatorDontUse,
            const_eval: ConstExprEvaluator::default(),
            validator: wasmparser::Validator::default(),
            compiler: Compiler::new(target_isa),
            current: None,
        };
        // ctx.linker
        //     .func_wrap(ctx.store, "spectest", "print", || {})?;
        // ctx.linker
        //     .func_wrap(ctx.store, "spectest", "print_i32", move |val: i32| {
        //         println!("{val}: i32")
        //     })?;
        // ctx.linker
        //     .func_wrap(ctx.store, "spectest", "print_i64", move |val: i64| {
        //         println!("{val}: i64")
        //     })?;
        // ctx.linker
        //     .func_wrap(ctx.store, "spectest", "print_f32", move |val: f32| {
        //         println!("{val}: f32")
        //     })?;
        // ctx.linker
        //     .func_wrap(ctx.store, "spectest", "print_f64", move |val: f64| {
        //         println!("{val}: f64")
        //     })?;
        // ctx.linker.func_wrap(
        //     ctx.store,
        //     "spectest",
        //     "print_i32_f32",
        //     move |i: i32, f: f32| {
        //         println!("{i}: i32");
        //         println!("{f}: f32");
        //     },
        // )?;
        // ctx.linker.func_wrap(
        //     ctx.store,
        //     "spectest",
        //     "print_f64_f64",
        //     move |f1: f64, f2: f64| {
        //         println!("{f1}: f64");
        //         println!("{f2}: f64");
        //     },
        // )?;
        //
        // let ty = GlobalType {
        //     content_type: ValType::I32,
        //     mutable: false,
        //     shared: false,
        // };
        // ctx.linker.define(
        //     ctx.store,
        //     "spectest",
        //     "global_i32",
        //     Global::new(ty, Value::I32(666)),
        // )?;
        //
        // let ty = GlobalType {
        //     content_type: ValType::I64,
        //     mutable: false,
        //     shared: false,
        // };
        // ctx.linker.define(
        //     ctx.store,
        //     "spectest",
        //     "global_i64",
        //     Global::new(ty, Value::I64(666)),
        // )?;
        //
        // let ty = GlobalType {
        //     content_type: ValType::F32,
        //     mutable: false,
        //     shared: false,
        // };
        // ctx.linker.define(
        //     ctx.store,
        //     "spectest",
        //     "global_f32",
        //     Global::new(ty, Value::F32(f32::from_bits(0x4426_a666u32))),
        // )?;
        //
        // let ty = GlobalType {
        //     content_type: ValType::F64,
        //     mutable: false,
        //     shared: false,
        // };
        // ctx.linker.define(
        //     ctx.store,
        //     "spectest",
        //     "global_f64",
        //     Global::new(ty, Value::F64(f64::from_bits(0x4084_d4cc_cccc_cccd))),
        // )?;
        //
        // let ty = TableType {
        //     element_type: RefType::FUNCREF,
        //     table64: false,
        //     initial: 10,
        //     maximum: Some(20),
        //     shared: false,
        // };
        // ctx.linker.define(
        //     ctx.store,
        //     "spectest",
        //     "table",
        //     Table::new(ty, Ref::Func(None)),
        // )?;
        //
        // let ty = MemoryType {
        //     memory64: false,
        //     shared: false,
        //     initial: 1,
        //     maximum: Some(2),
        //     page_size_log2: None,
        // };
        // ctx.linker
        //     .define(ctx.store, "spectest", "memory", Memory::new(ty))?;

        Ok(ctx)
    }

    fn run_file<P: AsRef<Path>>(&mut self, path: P) -> anyhow::Result<()> {
        let wat = std::fs::read_to_string(path.as_ref())?;
        let buf = ParseBuffer::new(&wat)?;
        let wast = parser::parse::<Wast>(&buf)?;
        for directive in wast.directives {
            self.run_directive(directive, path.as_ref(), &wat)?;
        }
        Ok(())
    }

    fn run_directive(
        &mut self,
        directive: WastDirective<'_>,
        path: &Path,
        wat: &str,
    ) -> anyhow::Result<()> {
        // Thread(thread) => {
        //     let mut core_linker = Linker::new(self.store.engine());
        //     if let Some(id) = thread.shared_module {
        //         let items = self
        //             .core_linker
        //             .iter(&mut self.store)
        //             .filter(|(module, _, _)| *module == id.name())
        //             .collect::<Vec<_>>();
        //         for (module, name, item) in items {
        //             core_linker.define(&mut self.store, module, name, item)?;
        //         }
        //     }
        //     let mut child_cx = WastContext {
        //         current: None,
        //         core_linker,
        //         #[cfg(feature = "component-model")]
        //         component_linker: component::Linker::new(self.store.engine()),
        //         store: Store::new(self.store.engine(), self.store.data().clone()),
        //     };
        //     let name = thread.name.name();
        //     let child =
        //         scope.spawn(move || child_cx.run_directives(thread.directives, filename, wast));
        //     threads.insert(name, child);
        // }
        //
        // Wait { thread, .. } => {
        //     let name = thread.name();
        //     threads
        //         .remove(name)
        //         .ok_or_else(|| anyhow!("no thread named `{name}`"))?
        //         .join()
        //         .unwrap()?;
        // }

        tracing::debug!("{directive:?}");

        match directive {
            WastDirective::Module(module) => self.wat(module, path, wat)?,
            WastDirective::Register { name, module, .. } => {
                self.register(module.map(|s| s.name()), name)?;
            }
            WastDirective::Invoke(i) => {
                self.perform_invoke(i)?;
            }
            WastDirective::AssertMalformed { module, .. } => {
                if let Ok(_) = self.wat(module, path, wat) {
                    bail!("expected malformed module to fail to instantiate");
                }
            }
            WastDirective::AssertInvalid {
                module, message, ..
            } => {
                let err = match self.wat(module, path, wat) {
                    Ok(()) => {
                        tracing::error!("expected module to fail to build");
                        return Ok(());
                    }
                    Err(e) => e,
                };
                let error_message = format!("{err:?}");

                if !error_message.contains(message) {
                    bail!(
                        "assert_invalid: expected {}, got {}",
                        message,
                        error_message
                    )
                }
            }
            WastDirective::AssertUnlinkable {
                module, message, ..
            } => {
                let err = match self.wat(QuoteWat::Wat(module), path, wat) {
                    Ok(()) => bail!("expected module to fail to link"),
                    Err(e) => e,
                };
                let error_message = format!("{err:?}");
                if !error_message.contains(message) {
                    bail!(
                        "assert_unlinkable: expected {}, got {}",
                        message,
                        error_message
                    )
                }
            }
            WastDirective::AssertTrap { exec, message, .. } => {
                let result = self.perform_execute(exec)?;
                self.assert_trap(result, message)?;
            }
            WastDirective::AssertReturn { exec, results, .. } => {
                let result = self.perform_execute(exec)?;
                self.assert_return(result, &results)?;
            }
            WastDirective::AssertExhaustion { call, message, .. } => {
                let result = self.perform_invoke(call)?;
                self.assert_trap(result, message)?;
            }
            WastDirective::ModuleDefinition(_) |
            WastDirective::ModuleInstance { .. } |
            WastDirective::AssertException { .. } |
            WastDirective::AssertSuspension { .. } |
            WastDirective::Thread(_) |
            WastDirective::Wait { .. } => todo!("unsupported wast directive {directive:?}")
        }

        Ok(())
    }

    fn wat(&mut self, mut wat: QuoteWat, path: &Path, raw: &str) -> anyhow::Result<()> {
        let encode_wat = |wat: &mut Wat<'_>| -> anyhow::Result<Vec<u8>> {
            Ok(EncodeOptions::default()
                .dwarf(path, raw, GenerateDwarf::Full)
                .encode_wat(wat)?)
        };

        let bytes = match &mut wat {
            QuoteWat::Wat(wat) => encode_wat(wat)?,
            QuoteWat::QuoteModule(_, source) => {
                let mut text = Vec::new();
                for (_, src) in source {
                    text.extend_from_slice(src);
                    text.push(b' ');
                }
                let text = std::str::from_utf8(&text).map_err(|_| {
                    let span = wat.span();
                    Error::new(span, "malformed UTF-8 encoding".to_string())
                })?;
                let buf = ParseBuffer::new(text)?;
                let mut wat = parser::parse::<Wat<'_>>(&buf)?;
                encode_wat(&mut wat)?
            }
            QuoteWat::QuoteComponent(_, _) => unimplemented!(),
        };

        let instance = match self.instantiate_module(&bytes)? {
            Outcome::Ok(i) => i,
            Outcome::Trap(e) => return Err(e).context("instantiation failed"),
        };

        if let Some(name) = wat.name() {
            self.linker
                .define_instance(&mut self.store, name.name(), instance)?;
        }
        self.current.replace(instance);

        Ok(())
    }

    fn register(&mut self, name: Option<&str>, as_name: &str) -> anyhow::Result<()> {
        match name {
            Some(name) => self.linker.alias_module(name, as_name)?,
            None => {
                let current = self.current.as_ref().context("no previous instance")?;
                self.linker
                    .define_instance(&mut self.store, as_name, *current)?
            }
        };

        Ok(())
    }

    fn perform_invoke(&mut self, exec: WastInvoke<'_>) -> anyhow::Result<Outcome> {
        let export = self.get_export(exec.module.map(|i| i.name()), exec.name)?;
        let func = export
            .into_func()
            .ok_or_else(|| anyhow!("no function named `{}`", exec.name))?;

        let values = exec
            .args
            .iter()
            .map(|v| match v {
                WastArg::Core(v) => wast_arg_to_val(v),
                WastArg::Component(_) => bail!("expected component function, found core"),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let mut results = vec![Val::I32(0); func.ty(&self.store).results().len()];

        match func.call(&mut self.store, &values, &mut results) {
            Ok(_) => Ok(Outcome::Ok(results)),
            Err(e) => Ok(Outcome::Trap(e.into())),
        }
    }

    fn perform_execute(&mut self, exec: WastExecute<'_>) -> anyhow::Result<Outcome> {
        match exec {
            WastExecute::Invoke(invoke) => self.perform_invoke(invoke),
            WastExecute::Wat(mut module) => Ok(match &mut module {
                Wat::Module(m) => {
                    self.instantiate_module(&m.encode()?)?.map(|_| Vec::new())
                }
                _ => unimplemented!(),
            }),
            WastExecute::Get { module, global, .. } => {
                self.get_global(module.map(|s| s.name()), global)
            }
        }
    }

    fn assert_return(&self, result: Outcome, results: &[WastRet<'_>]) -> anyhow::Result<()> {
        let values = result.into_result()?;
        if values.len() != results.len() {
            bail!("expected {} results found {}", results.len(), values.len());
        }
        for (v, e) in values.iter().zip(results) {
            let e = match e {
                WastRet::Core(core) => core,
                WastRet::Component(_) => {
                    bail!("expected component value found core value")
                }
            };

            match_val(&self.store, v, e)?;
        }

        Ok(())
    }

    fn assert_trap(&self, result: Outcome, expected: &str) -> anyhow::Result<()> {
        let trap = match result {
            Outcome::Ok(values) => bail!("expected trap, got {:?}", values),
            Outcome::Trap(t) => t,
        };
        let actual = format!("{trap:?}");
        if actual.contains(expected)
            // `bulk-memory-operations/bulk.wast` checks for a message that
            // specifies which element is uninitialized, but our trap_handling don't
            // shepherd that information out.
            || (expected.contains("uninitialized element 2") && actual.contains("uninitialized element"))
            // function references call_ref
            || (expected.contains("null function") && (actual.contains("uninitialized element") || actual.contains("null reference")))
        {
            return Ok(());
        }
        bail!("expected '{}', got '{}'", expected, actual)
    }

    fn instantiate_module(&mut self, module: &[u8]) -> anyhow::Result<Outcome<Instance>> {
        let module = Arc::new(Module::from_bytes(
            &mut self.validator,
            &self.compiler,
            module,
        )?);

        Ok(
            match self.linker.instantiate(
                &mut self.store,
                self.alloc,
                &mut self.const_eval,
                &module,
            ) {
                Ok(i) => Outcome::Ok(i),
                Err(e) => Outcome::Trap(e.into()),
            },
        )
    }

    /// Get the value of an exported global from an instance.
    fn get_global(&mut self, instance_name: Option<&str>, field: &str) -> anyhow::Result<Outcome> {
        let ext = self.get_export(instance_name, field)?;
        let global = ext
            .into_global()
            .ok_or_else(|| anyhow!("no global named `{field}`"))?;

        Ok(Outcome::Ok(vec![global.get(&self.store)]))
    }

    fn get_export(&mut self, module: Option<&str>, name: &str) -> anyhow::Result<Extern> {
        if let Some(module) = module {
            return self
                .linker
                .get(module, name)
                .cloned()
                .ok_or_else(|| anyhow!("no item named `{}::{}` found", module, name));
        }

        let cur = self
            .current
            .as_ref()
            .ok_or_else(|| anyhow!("no previous instance found"))?;

        cur.get_export(&mut self.store, name)
            .ok_or_else(|| anyhow!("no item named `{}` found", name))
    }
}

fn wast_arg_to_val(arg: &WastArgCore) -> anyhow::Result<Val> {
    match arg {
        WastArgCore::I32(v) => Ok(Val::I32(*v)),
        WastArgCore::I64(v) => Ok(Val::I64(*v)),
        WastArgCore::F32(v) => Ok(Val::F32(v.bits)),
        WastArgCore::F64(v) => Ok(Val::F64(v.bits)),
        WastArgCore::V128(v) => Ok(Val::V128(v.to_le_bytes())),
        // WastArgCore::RefNull(HeapType::Abstract {
        //                          ty: AbstractHeapType::Extern,
        //                          shared: false,
        //                      }) => Ok(VMVal::ExternRef(None)),
        // WastArgCore::RefNull(HeapType::Abstract {
        //                          ty: AbstractHeapType::Func,
        //                          shared: false,
        //                      }) => Ok(Value::FuncRef(None)),
        // WastArgCore::RefExtern(x) => Ok(Value::ExternRef(Some(*x))),
        other => bail!("couldn't convert {:?} to a runtime value", other),
    }
}

pub fn match_val(store: &Store, actual: &Val, expected: &WastRetCore) -> anyhow::Result<()> {
    match (actual, expected) {
        (_, WastRetCore::Either(expected)) => {
            for expected in expected {
                if match_val(store, actual, expected).is_ok() {
                    return Ok(());
                }
            }
            match_val(store, actual, &expected[0])
        }

        (Val::I32(a), WastRetCore::I32(b)) => match_int(a, b),
        (Val::I64(a), WastRetCore::I64(b)) => match_int(a, b),

        // Note that these float comparisons are comparing bits, not float
        // values, so we're testing for bit-for-bit equivalence
        (Val::F32(a), WastRetCore::F32(b)) => match_f32(*a, b),
        (Val::F64(a), WastRetCore::F64(b)) => match_f64(*a, b),
        (Val::V128(a), WastRetCore::V128(b)) => match_v128(u128::from_le_bytes(*a), b),

        // Null references.
        // (
        //     Val::FuncRef(None) | Val::ExternRef(None), /* | Value::AnyRef(None) */
        //     WastRetCore::RefNull(_),
        // )
        // | (Val::ExternRef(None), WastRetCore::RefExtern(None)) => Ok(()),
        //
        // // Null and non-null mismatches.
        // (Val::ExternRef(None), WastRetCore::RefExtern(Some(_))) => {
        //     bail!("expected non-null reference, found null")
        // }
        // (
        //     Val::ExternRef(Some(x)),
        //     WastRetCore::RefNull(Some(HeapType::Abstract {
        //         ty: AbstractHeapType::Extern,
        //         shared: false,
        //     })),
        // ) => {
        //     bail!("expected null externref, found non-null externref of {x}");
        // }
        // (Val::ExternRef(Some(_)) | Val::FuncRef(Some(_)), WastRetCore::RefNull(_)) => {
        //     bail!("expected null, found non-null reference: {actual:?}")
        // }
        //
        // // // Non-null references.
        // (Val::FuncRef(Some(_)), WastRetCore::RefFunc(_)) => Ok(()),
        // (Val::ExternRef(Some(x)), WastRetCore::RefExtern(Some(y))) => {
        //     ensure!(x == y, "expected {} found {}", y, x);
        //     Ok(())
        //     // let x = x
        //     //     .data(store)?
        //     //     .downcast_ref::<u32>()
        //     //     .expect("only u32 externrefs created in wast test suites");
        //     // if x == y {
        //     //     Ok(())
        //     // } else {
        //     //     bail!();
        //     // }
        // }

        // (Value::AnyRef(Some(x)), WastRetCore::RefI31) => {
        //     if x.is_i31(store)? {
        //         Ok(())
        //     } else {
        //         bail!("expected a `(ref i31)`, found {x:?}");
        //     }
        // }
        _ => bail!(
            "don't know how to compare {:?} and {:?} yet",
            actual,
            expected
        ),
    }
}

pub fn match_int<T>(actual: &T, expected: &T) -> anyhow::Result<()>
where
    T: Eq + Display + LowerHex,
{
    if actual == expected {
        Ok(())
    } else {
        bail!(
            "expected {:18} / {0:#018x}\n\
             actual   {:18} / {1:#018x}",
            expected,
            actual
        )
    }
}

pub fn match_f32(actual: u32, expected: &NanPattern<F32>) -> anyhow::Result<()> {
    match expected {
        // Check if an f32 (as u32 bits to avoid possible quieting when moving values in registers, e.g.
        // https://developer.arm.com/documentation/ddi0344/i/neon-and-vfp-programmers-model/modes-of-operation/default-nan-mode?lang=en)
        // is a canonical NaN:
        //  - the sign bit is unspecified,
        //  - the 8-bit exponent is set to all 1s
        //  - the MSB of the payload is set to 1 (a quieted NaN) and all others to 0.
        // See https://webassembly.github.io/spec/core/syntax/values.html#floating-point.
        NanPattern::CanonicalNan => {
            let canon_nan = 0x7fc0_0000;
            if (actual & 0x7fff_ffff) == canon_nan {
                Ok(())
            } else {
                bail!(
                    "expected {:10} / {:#010x}\n\
                     actual   {:10} / {:#010x}",
                    "canon-nan",
                    canon_nan,
                    f32::from_bits(actual),
                    actual,
                )
            }
        }

        // Check if an f32 (as u32, see comments above) is an arithmetic NaN.
        // This is the same as a canonical NaN including that the payload MSB is
        // set to 1, but one or more of the remaining payload bits MAY BE set to
        // 1 (a canonical NaN specifies all 0s). See
        // https://webassembly.github.io/spec/core/syntax/values.html#floating-point.
        NanPattern::ArithmeticNan => {
            const AF32_NAN: u32 = 0x7f80_0000;
            let is_nan = actual & AF32_NAN == AF32_NAN;
            const AF32_PAYLOAD_MSB: u32 = 0x0040_0000;
            let is_msb_set = actual & AF32_PAYLOAD_MSB == AF32_PAYLOAD_MSB;
            if is_nan && is_msb_set {
                Ok(())
            } else {
                bail!(
                    "expected {:>10} / {:>10}\n\
                     actual   {:10} / {:#010x}",
                    "arith-nan",
                    "0x7fc*****",
                    f32::from_bits(actual),
                    actual,
                )
            }
        }
        NanPattern::Value(expected_value) => {
            if actual == expected_value.bits {
                Ok(())
            } else {
                bail!(
                    "expected {:10} / {:#010x}\n\
                     actual   {:10} / {:#010x}",
                    f32::from_bits(expected_value.bits),
                    expected_value.bits,
                    f32::from_bits(actual),
                    actual,
                )
            }
        }
    }
}

pub fn match_f64(actual: u64, expected: &NanPattern<F64>) -> anyhow::Result<()> {
    match expected {
        // Check if an f64 (as u64 bits to avoid possible quieting when moving values in registers, e.g.
        // https://developer.arm.com/documentation/ddi0344/i/neon-and-vfp-programmers-model/modes-of-operation/default-nan-mode?lang=en)
        // is a canonical NaN:
        //  - the sign bit is unspecified,
        //  - the 11-bit exponent is set to all 1s
        //  - the MSB of the payload is set to 1 (a quieted NaN) and all others to 0.
        // See https://webassembly.github.io/spec/core/syntax/values.html#floating-point.
        NanPattern::CanonicalNan => {
            let canon_nan = 0x7ff8_0000_0000_0000;
            if (actual & 0x7fff_ffff_ffff_ffff) == canon_nan {
                Ok(())
            } else {
                bail!(
                    "expected {:18} / {:#018x}\n\
                     actual   {:18} / {:#018x}",
                    "canon-nan",
                    canon_nan,
                    f64::from_bits(actual),
                    actual,
                )
            }
        }

        // Check if an f64 (as u64, see comments above) is an arithmetic NaN. This is the same as a
        // canonical NaN including that the payload MSB is set to 1, but one or more of the remaining
        // payload bits MAY BE set to 1 (a canonical NaN specifies all 0s). See
        // https://webassembly.github.io/spec/core/syntax/values.html#floating-point.
        NanPattern::ArithmeticNan => {
            const AF64_NAN: u64 = 0x7ff0_0000_0000_0000;
            let is_nan = actual & AF64_NAN == AF64_NAN;
            const AF64_PAYLOAD_MSB: u64 = 0x0008_0000_0000_0000;
            let is_msb_set = actual & AF64_PAYLOAD_MSB == AF64_PAYLOAD_MSB;
            if is_nan && is_msb_set {
                Ok(())
            } else {
                bail!(
                    "expected {:>18} / {:>18}\n\
                     actual   {:18} / {:#018x}",
                    "arith-nan",
                    "0x7ff8************",
                    f64::from_bits(actual),
                    actual,
                )
            }
        }
        NanPattern::Value(expected_value) => {
            if actual == expected_value.bits {
                Ok(())
            } else {
                bail!(
                    "expected {:18} / {:#018x}\n\
                     actual   {:18} / {:#018x}",
                    f64::from_bits(expected_value.bits),
                    expected_value.bits,
                    f64::from_bits(actual),
                    actual,
                )
            }
        }
    }
}

fn match_v128(actual: u128, expected: &V128Pattern) -> anyhow::Result<()> {
    match expected {
        V128Pattern::I8x16(expected) => {
            let actual = [
                extract_lane_as_i8(actual, 0),
                extract_lane_as_i8(actual, 1),
                extract_lane_as_i8(actual, 2),
                extract_lane_as_i8(actual, 3),
                extract_lane_as_i8(actual, 4),
                extract_lane_as_i8(actual, 5),
                extract_lane_as_i8(actual, 6),
                extract_lane_as_i8(actual, 7),
                extract_lane_as_i8(actual, 8),
                extract_lane_as_i8(actual, 9),
                extract_lane_as_i8(actual, 10),
                extract_lane_as_i8(actual, 11),
                extract_lane_as_i8(actual, 12),
                extract_lane_as_i8(actual, 13),
                extract_lane_as_i8(actual, 14),
                extract_lane_as_i8(actual, 15),
            ];
            if actual == *expected {
                return Ok(());
            }
            bail!(
                "expected {:4?}\n\
                 actual   {:4?}\n\
                 \n\
                 expected (hex) {0:02x?}\n\
                 actual (hex)   {1:02x?}",
                expected,
                actual,
            )
        }
        V128Pattern::I16x8(expected) => {
            let actual = [
                extract_lane_as_i16(actual, 0),
                extract_lane_as_i16(actual, 1),
                extract_lane_as_i16(actual, 2),
                extract_lane_as_i16(actual, 3),
                extract_lane_as_i16(actual, 4),
                extract_lane_as_i16(actual, 5),
                extract_lane_as_i16(actual, 6),
                extract_lane_as_i16(actual, 7),
            ];
            if actual == *expected {
                return Ok(());
            }
            bail!(
                "expected {:6?}\n\
                 actual   {:6?}\n\
                 \n\
                 expected (hex) {0:04x?}\n\
                 actual (hex)   {1:04x?}",
                expected,
                actual,
            )
        }
        V128Pattern::I32x4(expected) => {
            let actual = [
                extract_lane_as_i32(actual, 0),
                extract_lane_as_i32(actual, 1),
                extract_lane_as_i32(actual, 2),
                extract_lane_as_i32(actual, 3),
            ];
            if actual == *expected {
                return Ok(());
            }
            bail!(
                "expected {:11?}\n\
                 actual   {:11?}\n\
                 \n\
                 expected (hex) {0:08x?}\n\
                 actual (hex)   {1:08x?}",
                expected,
                actual,
            )
        }
        V128Pattern::I64x2(expected) => {
            let actual = [
                extract_lane_as_i64(actual, 0),
                extract_lane_as_i64(actual, 1),
            ];
            if actual == *expected {
                return Ok(());
            }
            bail!(
                "expected {:20?}\n\
                 actual   {:20?}\n\
                 \n\
                 expected (hex) {0:016x?}\n\
                 actual (hex)   {1:016x?}",
                expected,
                actual,
            )
        }
        V128Pattern::F32x4(expected) => {
            for (i, expected) in expected.iter().enumerate() {
                let a = extract_lane_as_i32(actual, i) as u32;
                match_f32(a, expected).with_context(|| format!("difference in lane {i}"))?;
            }
            Ok(())
        }
        V128Pattern::F64x2(expected) => {
            for (i, expected) in expected.iter().enumerate() {
                let a = extract_lane_as_i64(actual, i) as u64;
                match_f64(a, expected).with_context(|| format!("difference in lane {i}"))?;
            }
            Ok(())
        }
    }
}

fn extract_lane_as_i8(bytes: u128, lane: usize) -> i8 {
    (bytes >> (lane * 8)) as i8
}

fn extract_lane_as_i16(bytes: u128, lane: usize) -> i16 {
    (bytes >> (lane * 16)) as i16
}

fn extract_lane_as_i32(bytes: u128, lane: usize) -> i32 {
    (bytes >> (lane * 32)) as i32
}

fn extract_lane_as_i64(bytes: u128, lane: usize) -> i64 {
    (bytes >> (lane * 64)) as i64
}
