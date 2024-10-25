use crate::indices::{
    DataIndex, ElemIndex, FieldIndex, FuncIndex, FuncRefIndex, GlobalIndex, LocalIndex,
    MemoryIndex, TableIndex, TagIndex, TypeIndex,
};
use crate::translate::const_expr::ConstExpr;
use crate::translate::{
    EntityIndex, EntityType, FuncCompileInput, FunctionType, Import, LabelIndex, MemoryInitializer,
    MemoryPlan, ProducersLanguage, ProducersLanguageField, ProducersSdk, ProducersSdkField,
    ProducersTool, ProducersToolField, TableInitialValue, TablePlan, TableSegment,
    TableSegmentElements, Translation,
};
use crate::wasm_unsupported;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;
use cranelift_entity::packed_option::ReservedValue;
use hashbrown::HashMap;
use wasmparser::{
    BinaryReader, CustomSectionReader, DataKind, DataSectionReader, ElementItems, ElementKind,
    ElementSectionReader, ExportSectionReader, ExternalKind, FunctionSectionReader,
    GlobalSectionReader, ImportSectionReader, IndirectNameMap, MemorySectionReader, NameMap,
    NameSectionReader, Parser, Payload, ProducersSectionReader, TableInit, TableSectionReader,
    TagSectionReader, TypeRef, TypeSectionReader, Validator, WasmFeatures,
};
use wasmparser::{Name, ProducersFieldValue};

pub struct ModuleTranslator<'a, 'wasm> {
    result: Translation<'wasm>,
    validator: &'a mut Validator,
}

impl<'a, 'wasm> ModuleTranslator<'a, 'wasm> {
    pub fn new(validator: &'a mut Validator) -> Self {
        Self {
            result: Translation::default(),
            validator,
        }
    }

    pub fn translate(mut self, data: &'wasm [u8]) -> crate::TranslationResult<Translation<'wasm>> {
        let mut parser = Parser::default();
        parser.set_features(*self.validator.features());

        for payload in parser.parse_all(data) {
            self.translate_payload(payload?)?;
        }

        self.validator.reset();
        Ok(self.result)
    }

    pub fn translate_payload(&mut self, payload: Payload<'wasm>) -> crate::TranslationResult<()> {
        match payload {
            Payload::Version {
                num,
                encoding,
                range,
            } => {
                self.validator.version(num, encoding, &range)?;
            }
            Payload::End(offset) => {
                self.validator.end(offset)?;
            }
            Payload::TypeSection(types) => {
                self.validator.type_section(&types)?;
                self.read_type_section(types)?;
            }
            Payload::ImportSection(imports) => {
                self.validator.import_section(&imports)?;
                self.read_import_section(imports)?;
            }
            Payload::FunctionSection(functions) => {
                self.validator.function_section(&functions)?;
                self.read_function_section(functions)?;
            }
            Payload::TableSection(tables) => {
                self.validator.table_section(&tables)?;
                self.read_table_section(tables)?;
            }
            Payload::MemorySection(memories) => {
                self.validator.memory_section(&memories)?;
                self.read_memory_section(memories)?;
            }
            Payload::TagSection(tags) => {
                self.validator.tag_section(&tags)?;
                self.read_tag_section(tags)?;
            }
            Payload::GlobalSection(globals) => {
                self.validator.global_section(&globals)?;
                self.read_global_section(globals)?;
            }
            Payload::ExportSection(exports) => {
                self.validator.export_section(&exports)?;
                self.read_export_section(exports)?;
            }
            Payload::StartSection { func, range } => {
                self.validator.start_section(func, &range)?;
                self.result.module.start = Some(FuncIndex::from_u32(func));
            }
            Payload::ElementSection(elements) => {
                self.validator.element_section(&elements)?;
                self.read_element_section(elements)?;
            }
            Payload::DataCountSection { count, range } => {
                self.validator.data_count_section(count, &range)?;
            }
            Payload::DataSection(section) => {
                self.validator.data_section(&section)?;
                self.read_data_section(section)?;
            }
            Payload::CodeSectionStart { count, range, .. } => {
                self.validator.code_section_start(count, &range)?;
                self.result
                    .func_compile_inputs
                    .reserve_exact(count as usize);
            }
            Payload::CodeSectionEntry(body) => {
                let validator = self.validator.code_section_entry(&body)?;
                self.result
                    .func_compile_inputs
                    .push(FuncCompileInput { body, validator });
            }
            Payload::CustomSection(sec) if sec.name() == "target_features" => {
                self.read_target_feature_section(&sec);
            }
            Payload::CustomSection(sec) if sec.name() == "name" => {
                self.read_name_section(NameSectionReader::new(BinaryReader::new(
                    sec.data(),
                    sec.data_offset(),
                )))?;
            }
            Payload::CustomSection(sec) if sec.name() == "producers" => {
                let reader = ProducersSectionReader::new(BinaryReader::new_features(
                    sec.data(),
                    sec.data_offset(),
                    *self.validator.features(),
                ))?;

                self.read_producers_section(reader)?;
            }
            Payload::CustomSection(sec) => {
                let name = sec.name().trim_end_matches(".dwo");
                if !name.starts_with(".debug_") {
                    tracing::warn!("unhandled custom section {sec:?}");
                    return Ok(());
                }
                self.read_dwarf_section(name, &sec);
            }
            Payload::ModuleSection { .. }
            | Payload::InstanceSection(_)
            | Payload::CoreTypeSection(_)
            | Payload::ComponentSection { .. }
            | Payload::ComponentInstanceSection(_)
            | Payload::ComponentAliasSection(_)
            | Payload::ComponentTypeSection(_)
            | Payload::ComponentCanonicalSection(_)
            | Payload::ComponentStartSection { .. }
            | Payload::ComponentImportSection(_)
            | Payload::ComponentExportSection(_) => {
                return Err(wasm_unsupported!("component module"));
            }
            p => tracing::warn!("unknown section {p:?}"),
        }

        Ok(())
    }

    fn flag_func_as_escaped(&mut self, func_index: FuncIndex) {
        let ty = &mut self.result.module.functions[func_index];
        if ty.is_escaping() {
            return;
        }
        let index = self.result.module.num_escaped_functions;
        ty.func_ref = FuncRefIndex::from_u32(index);
        self.result.module.num_escaped_functions += 1;
    }

    fn read_type_section(
        &mut self,
        types: TypeSectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        let count = types.count();
        self.result.module.types.reserve_exact(count as usize);

        for ty in types.into_iter_err_on_gc_types() {
            self.result.module.types.push(ty?);
        }

        Ok(())
    }

    fn read_import_section(
        &mut self,
        imports: ImportSectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        self.result
            .module
            .imports
            .reserve_exact(imports.count() as usize);

        for import in imports {
            let import = import?;
            let ty = match import.ty {
                TypeRef::Func(index) => {
                    let index = TypeIndex::from_u32(index);
                    self.result.module.num_imported_functions += 1;
                    EntityType::Function(index)
                }
                TypeRef::Table(ty) => {
                    self.result.module.num_imported_tables += 1;
                    EntityType::Table(
                        self.result
                            .module
                            .table_plans
                            .push(TablePlan::for_table(ty)),
                    )
                }
                TypeRef::Memory(ty) => {
                    self.result.module.num_imported_memories += 1;
                    EntityType::Memory(
                        self.result
                            .module
                            .memory_plans
                            .push(MemoryPlan::for_memory(ty)),
                    )
                }
                TypeRef::Global(ty) => {
                    self.result.module.num_imported_globals += 1;
                    EntityType::Global(self.result.module.globals.push(ty))
                }

                // doesn't get past validation
                TypeRef::Tag(_) => unreachable!(),
            };

            self.result.module.imports.push(Import {
                module: import.module,
                name: import.name,
                ty,
            });
        }

        Ok(())
    }

    fn read_function_section(
        &mut self,
        functions: FunctionSectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        self.result
            .module
            .functions
            .reserve_exact(functions.count() as usize);

        for index in functions {
            let signature = TypeIndex::from_u32(index?);
            self.result.module.functions.push(FunctionType {
                signature,
                func_ref: FuncRefIndex::reserved_value(),
            });
        }

        Ok(())
    }

    fn read_table_section(
        &mut self,
        tables: TableSectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        self.result
            .module
            .table_plans
            .reserve_exact(tables.count() as usize);
        self.result
            .module
            .table_initializers
            .initial_values
            .reserve_exact(tables.count() as usize);

        for table in tables {
            let table = table?;
            self.result
                .module
                .table_plans
                .push(TablePlan::for_table(table.ty));

            let init = match table.init {
                TableInit::RefNull => TableInitialValue::RefNull,
                TableInit::Expr(expr) => {
                    let (expr, escaped) = ConstExpr::from_wasmparser(expr)?;
                    for func in escaped {
                        self.flag_func_as_escaped(func);
                    }
                    TableInitialValue::ConstExpr(expr)
                }
            };
            self.result
                .module
                .table_initializers
                .initial_values
                .push(init);
        }

        Ok(())
    }

    fn read_memory_section(
        &mut self,
        memories: MemorySectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        self.result
            .module
            .memory_plans
            .reserve_exact(memories.count() as usize);

        for ty in memories {
            self.result
                .module
                .memory_plans
                .push(MemoryPlan::for_memory(ty?));
        }

        Ok(())
    }

    fn read_tag_section(&self, _tags: TagSectionReader<'wasm>) -> crate::TranslationResult<()> {
        Err(wasm_unsupported!("exception handling"))
    }

    fn read_global_section(
        &mut self,
        globals: GlobalSectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        self.result
            .module
            .globals
            .reserve_exact(globals.count() as usize);
        self.result
            .module
            .global_initializers
            .reserve_exact(globals.count() as usize);

        for global in globals {
            let global = global?;
            self.result.module.globals.push(global.ty);

            let (init_expr, escaped) = ConstExpr::from_wasmparser(global.init_expr)?;
            for func in escaped {
                self.flag_func_as_escaped(func);
            }
            self.result.module.global_initializers.push(init_expr);
        }

        Ok(())
    }

    fn read_export_section(
        &mut self,
        exports: ExportSectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        for export in exports {
            let export = export?;
            let index = match export.kind {
                ExternalKind::Func => {
                    let index = FuncIndex::from_u32(export.index);
                    self.flag_func_as_escaped(index);
                    self.result
                        .debug_info
                        .names
                        .func_names
                        .insert(index, export.name);
                    EntityIndex::Function(index)
                }
                ExternalKind::Table => {
                    let index = TableIndex::from_u32(export.index);
                    self.result
                        .debug_info
                        .names
                        .table_names
                        .insert(index, export.name);
                    EntityIndex::Table(index)
                }
                ExternalKind::Memory => {
                    let index = MemoryIndex::from_u32(export.index);
                    self.result
                        .debug_info
                        .names
                        .memory_names
                        .insert(index, export.name);
                    EntityIndex::Memory(index)
                }
                ExternalKind::Tag => {
                    let index = TagIndex::from_u32(export.index);
                    self.result
                        .debug_info
                        .names
                        .tag_names
                        .insert(index, export.name);
                    EntityIndex::Tag(index)
                }
                ExternalKind::Global => {
                    let index = GlobalIndex::from_u32(export.index);
                    self.result
                        .debug_info
                        .names
                        .global_names
                        .insert(index, export.name);
                    EntityIndex::Global(index)
                }
            };

            self.result.module.exports.insert(export.name, index);
        }

        Ok(())
    }

    fn read_element_section(
        &mut self,
        elements: ElementSectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        for (elem_index, element) in elements.into_iter().enumerate() {
            let element = element?;
            let elem_index = ElemIndex::from_u32(elem_index as u32);

            let elements = match element.items {
                ElementItems::Functions(funcs) => {
                    let mut out = Vec::with_capacity(funcs.count() as usize);
                    for func_idx in funcs {
                        out.push(FuncIndex::from_u32(func_idx?));
                    }
                    TableSegmentElements::Functions(out.into_boxed_slice())
                }
                ElementItems::Expressions(_, exprs) => {
                    let mut out = Vec::with_capacity(exprs.count() as usize);

                    for expr in exprs {
                        let (expr, escaped) = ConstExpr::from_wasmparser(expr?)?;
                        for func in escaped {
                            self.flag_func_as_escaped(func);
                        }
                        out.push(expr);
                    }
                    TableSegmentElements::Expressions(out.into_boxed_slice())
                }
            };

            match element.kind {
                ElementKind::Active {
                    table_index,
                    offset_expr,
                } => {
                    let table_index = TableIndex::from_u32(table_index.unwrap_or(0));
                    let (offset, escaped) = ConstExpr::from_wasmparser(offset_expr)?;
                    debug_assert!(escaped.is_empty());

                    self.result
                        .module
                        .table_initializers
                        .segments
                        .push(TableSegment {
                            table_index,
                            offset,
                            elements,
                        });
                    self.result
                        .module
                        .active_table_initializers
                        .insert(elem_index);
                }
                ElementKind::Passive => {
                    self.result
                        .module
                        .passive_table_initializers
                        .insert(elem_index, elements);
                }
                ElementKind::Declared => {}
            }
        }

        Ok(())
    }

    fn read_data_section(
        &mut self,
        section: DataSectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        for (data_index, entry) in section.into_iter().enumerate() {
            let entry = entry?;
            let data_index = DataIndex::from_u32(data_index as u32);

            match entry.kind {
                DataKind::Active {
                    memory_index,
                    offset_expr,
                } => {
                    let memory_index = MemoryIndex::from_u32(memory_index);
                    let (offset, escaped) = ConstExpr::from_wasmparser(offset_expr)?;
                    debug_assert!(escaped.is_empty());

                    self.result
                        .module
                        .memory_initializers
                        .push(MemoryInitializer {
                            memory_index,
                            offset,
                            bytes: entry.data,
                        });
                    self.result
                        .module
                        .active_memory_initializers
                        .insert(data_index);
                }
                DataKind::Passive => {
                    self.result
                        .module
                        .passive_memory_initializers
                        .insert(data_index, entry.data);
                }
            }
        }

        Ok(())
    }

    fn read_target_feature_section(&mut self, section: &CustomSectionReader<'wasm>) {
        let mut r = BinaryReader::new_features(
            section.data(),
            section.data_offset(),
            *self.validator.features(),
        );

        let _count = r.read_u8().unwrap();

        let mut required_features = WasmFeatures::empty();

        while !r.eof() {
            let prefix = r.read_u8().unwrap();
            assert_eq!(prefix, 0x2b, "only the `+` prefix is supported");

            let len = r.read_var_u64().unwrap();
            let feature = r.read_bytes(usize::try_from(len).unwrap()).unwrap();
            let feature = core::str::from_utf8(feature).unwrap();

            match feature {
                "atomics" => required_features.insert(WasmFeatures::THREADS),
                "bulk-memory" => required_features.insert(WasmFeatures::BULK_MEMORY),
                "exception-handling" => required_features.insert(WasmFeatures::EXCEPTIONS),
                "multivalue" => required_features.insert(WasmFeatures::MULTI_VALUE),
                "mutable-globals" => required_features.insert(WasmFeatures::MUTABLE_GLOBAL),
                "nontrapping-fptoint" => {
                    required_features.insert(WasmFeatures::SATURATING_FLOAT_TO_INT);
                }
                "sign-ext" => required_features.insert(WasmFeatures::SIGN_EXTENSION),
                "simd128" => required_features.insert(WasmFeatures::SIMD),
                "tail-call" => required_features.insert(WasmFeatures::TAIL_CALL),
                "reference-types" => required_features.insert(WasmFeatures::REFERENCE_TYPES),
                "gc" => required_features.insert(WasmFeatures::GC),
                "memory64" => required_features.insert(WasmFeatures::MEMORY64),
                "relaxed-simd" => required_features.insert(WasmFeatures::RELAXED_SIMD),
                "extended-const" => required_features.insert(WasmFeatures::EXTENDED_CONST),
                "multimemory" => required_features.insert(WasmFeatures::MULTI_MEMORY),
                "shared-everything" => {
                    required_features.insert(WasmFeatures::SHARED_EVERYTHING_THREADS)
                }
                _ => tracing::warn!("unknown required WASM feature `{feature}`"),
            }
        }

        self.result.required_features = required_features;
    }

    fn read_name_section(
        &mut self,
        reader: NameSectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        for subsection in reader {
            fn for_each_direct_name<'wasm>(
                names: NameMap<'wasm>,
                mut f: impl FnMut(u32, &'wasm str),
            ) -> crate::TranslationResult<()> {
                for name in names {
                    let name = name?;

                    f(name.index, name.name)
                }

                Ok(())
            }

            fn for_each_indirect_name<'wasm, I>(
                names: IndirectNameMap<'wasm>,
                mut f1: impl FnMut(&mut HashMap<I, &'wasm str>, u32, &'wasm str),
                mut f2: impl FnMut(HashMap<I, &'wasm str>, u32),
            ) -> crate::TranslationResult<()> {
                for naming in names {
                    let name = naming?;
                    let mut result = HashMap::default();

                    for name in name.names {
                        let name = name?;

                        f1(&mut result, name.index, name.name)
                    }

                    f2(result, name.index);
                }

                Ok(())
            }

            match subsection? {
                Name::Module { name, .. } => {
                    self.result.debug_info.names.module_name = Some(name);
                }
                Name::Function(names) => {
                    for_each_direct_name(names, |idx, name| {
                        // Skip this naming if it's naming a function that
                        // doesn't actually exist.
                        if (idx as usize) < self.result.module.functions.len() {
                            self.result
                                .debug_info
                                .names
                                .func_names
                                .insert(FuncIndex::from_u32(idx), name);
                        }
                    })?;
                }
                Name::Local(names) => {
                    for_each_indirect_name(
                        names,
                        |result, idx, name| {
                            result.insert(LocalIndex::from_u32(idx), name);
                        },
                        |result, idx| {
                            // Skip this naming if it's naming a function that
                            // doesn't actually exist.
                            if (idx as usize) < self.result.module.functions.len() {
                                self.result
                                    .debug_info
                                    .names
                                    .locals_names
                                    .insert(FuncIndex::from_u32(idx), result);
                            }
                        },
                    )?;
                }
                Name::Global(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .global_names
                            .insert(GlobalIndex::from_u32(idx), name);
                    })?;
                }
                Name::Data(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .data_names
                            .insert(DataIndex::from_u32(idx), name);
                    })?;
                }
                Name::Type(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .type_names
                            .insert(TypeIndex::from_u32(idx), name);
                    })?;
                }
                Name::Label(names) => {
                    for_each_indirect_name(
                        names,
                        |result, idx, name| {
                            result.insert(LabelIndex::from_u32(idx), name);
                        },
                        |result, idx| {
                            // Skip this naming if it's naming a function that
                            // doesn't actually exist.
                            if (idx as usize) < self.result.module.functions.len() {
                                self.result
                                    .debug_info
                                    .names
                                    .labels_names
                                    .insert(FuncIndex::from_u32(idx), result);
                            }
                        },
                    )?;
                }
                Name::Table(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .table_names
                            .insert(TableIndex::from_u32(idx), name);
                    })?;
                }
                Name::Memory(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .memory_names
                            .insert(MemoryIndex::from_u32(idx), name);
                    })?;
                }
                Name::Element(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .element_names
                            .insert(ElemIndex::from_u32(idx), name);
                    })?;
                }
                Name::Field(names) => {
                    for_each_indirect_name(
                        names,
                        |result, idx, name| {
                            // Skip this naming if it's naming a function that
                            // doesn't actually exist.
                            if (idx as usize) < self.result.module.functions.len() {
                                result.insert(FieldIndex::from_u32(idx), name);
                            }
                        },
                        |result, idx| {
                            self.result
                                .debug_info
                                .names
                                .fields_names
                                .insert(FuncIndex::from_u32(idx), result);
                        },
                    )?;
                }
                Name::Tag(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .tag_names
                            .insert(TagIndex::from_u32(idx), name);
                    })?;
                }
                Name::Unknown { .. } => {}
            }
        }

        Ok(())
    }

    fn read_producers_section(
        &mut self,
        section: ProducersSectionReader<'wasm>,
    ) -> crate::TranslationResult<()> {
        for field in section {
            let field = field?;
            match field.name {
                "language" => {
                    for value in field.values {
                        let ProducersFieldValue { name, version } = value?;
                        let name = match name {
                            "wat" => ProducersLanguage::Wat,
                            "C" => ProducersLanguage::C,
                            "C++" => ProducersLanguage::Cpp,
                            "Rust" => ProducersLanguage::Rust,
                            "JavaScript" => ProducersLanguage::JavaScript,
                            _ => ProducersLanguage::Other(name),
                        };

                        self.result
                            .debug_info
                            .producers
                            .language
                            .push(ProducersLanguageField { name, version });
                    }
                }
                "processed-by" => {
                    for value in field.values {
                        let ProducersFieldValue { name, version } = value?;
                        let name = match name {
                            "wabt" => ProducersTool::Wabt,
                            "LLVM" => ProducersTool::Llvm,
                            "clang" => ProducersTool::Clang,
                            "lld" => ProducersTool::Lld,
                            "Binaryen" => ProducersTool::Binaryen,
                            "rustc" => ProducersTool::Rustc,
                            "wasm-bindgen" => ProducersTool::WasmBindgen,
                            "wasm-pack" => ProducersTool::WasmPack,
                            "webassemblyjs" => ProducersTool::Webassemblyjs,
                            "wasm-snip" => ProducersTool::WasmSnip,
                            "Javy" => ProducersTool::Javy,
                            _ => ProducersTool::Other(name),
                        };

                        self.result
                            .debug_info
                            .producers
                            .processed_by
                            .push(ProducersToolField { name, version });
                    }
                }
                "sdk" => {
                    for value in field.values {
                        let ProducersFieldValue { name, version } = value?;
                        let name = match name {
                            "Emscripten" => ProducersSdk::Emscripten,
                            "Webpack" => ProducersSdk::Webpack,
                            _ => ProducersSdk::Other(name),
                        };

                        self.result
                            .debug_info
                            .producers
                            .sdk
                            .push(ProducersSdkField { name, version });
                    }
                }
                _ => unreachable!(),
            }
        }

        Ok(())
    }

    fn read_dwarf_section(&mut self, name: &'wasm str, section: &CustomSectionReader<'wasm>) {
        let endian = gimli::LittleEndian;
        let data = section.data();
        let slice = gimli::EndianSlice::new(data, endian);

        let mut dwarf = gimli::Dwarf::default();
        let info = &mut self.result.debug_info;

        match name {
            // `gimli::Dwarf` fields.
            ".debug_abbrev" => dwarf.debug_abbrev = gimli::DebugAbbrev::new(data, endian),
            ".debug_addr" => dwarf.debug_addr = gimli::DebugAddr::from(slice),
            ".debug_info" => {
                dwarf.debug_info = gimli::DebugInfo::new(data, endian);
            }
            ".debug_line" => dwarf.debug_line = gimli::DebugLine::new(data, endian),
            ".debug_line_str" => dwarf.debug_line_str = gimli::DebugLineStr::from(slice),
            ".debug_str" => dwarf.debug_str = gimli::DebugStr::new(data, endian),
            ".debug_str_offsets" => dwarf.debug_str_offsets = gimli::DebugStrOffsets::from(slice),
            ".debug_str_sup" => {
                let dwarf_sup = gimli::Dwarf {
                    debug_str: gimli::DebugStr::from(slice),
                    ..Default::default()
                };
                dwarf.sup = Some(Arc::new(dwarf_sup));
            }
            ".debug_types" => dwarf.debug_types = gimli::DebugTypes::from(slice),

            // Additional fields.
            ".debug_loc" => info.debug_loc = gimli::DebugLoc::from(slice),
            ".debug_loclists" => info.debug_loclists = gimli::DebugLocLists::from(slice),
            ".debug_ranges" => info.debug_ranges = gimli::DebugRanges::new(data, endian),
            ".debug_rnglists" => info.debug_rnglists = gimli::DebugRngLists::new(data, endian),

            // DWARF package fields
            ".debug_cu_index" => info.debug_cu_index = gimli::DebugCuIndex::new(data, endian),
            ".debug_tu_index" => info.debug_tu_index = gimli::DebugTuIndex::new(data, endian),

            // We don't use these at the moment.
            ".debug_aranges" | ".debug_pubnames" | ".debug_pubtypes" => return,
            other => {
                tracing::warn!("unknown debug section `{}`", other);
                return;
            }
        }

        dwarf.ranges = gimli::RangeLists::new(info.debug_ranges, info.debug_rnglists);
        dwarf.locations = gimli::LocationLists::new(info.debug_loc, info.debug_loclists);

        info.dwarf = dwarf;
    }
}
