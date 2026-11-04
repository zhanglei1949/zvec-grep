use super::*;

fn target_names(result: &FactInspection) -> Vec<String> {
    calls(result)
        .into_iter()
        .filter(|r| r.status == ResolutionStatus::Resolved)
        .map(|r| {
            let target = &r.candidates[0];
            result
                .files
                .iter()
                .find(|f| f.version.path == target.path)
                .expect("source")
                .nodes
                .iter()
                .find(|n| n.id == target.node_id)
                .expect("target")
                .qualified_name
                .join("::")
        })
        .collect()
}
#[test]
fn go_package_alias_calls_and_method_declarations() {
    let result = inspect(&[
        input(
            "cmd/main.go",
            Language::Go,
            "package main\nimport util \"example.test/lib\"\nfunc Run(){util.Add(); local()}\n",
            &["example.test", "cmd"],
        ),
        input(
            "cmd/local.go",
            Language::Go,
            "package main\nfunc local(){}\n",
            &["example.test", "cmd"],
        ),
        input(
            "lib/add.go",
            Language::Go,
            "package mathutil\ntype Counter struct { Value int }\nfunc (c *Counter) Inc(){}\nfunc Add(){}\n",
            &["example.test", "lib"],
        ),
    ]);
    assert_eq!(target_names(&result), ["Add", "local"]);
    assert!(
        result
            .references
            .iter()
            .filter(|r| r.reference.kind == RelationKind::Import)
            .all(|r| r.status == ResolutionStatus::Resolved)
    );
    assert!(
        result.files[2]
            .nodes
            .iter()
            .any(|n| n.qualified_name == ["Counter", "Inc"])
    );
}
#[test]
fn go_default_import_uses_declared_package_and_shadowing_is_explicit() {
    let result = inspect(&[
        input(
            "main.go",
            Language::Go,
            "package main\nimport \"example.test/v2\"\nfunc Run(callback func()){mathutil.Add();callback();unknown.Inc()}\n",
            &["example.test"],
        ),
        input(
            "lib.go",
            Language::Go,
            "package mathutil\nfunc Add(){}\n",
            &["example.test", "v2"],
        ),
    ]);
    assert_eq!(target_names(&result), ["Add"]);
    assert_eq!(
        calls(&result)
            .iter()
            .filter(|r| r.status != ResolutionStatus::Resolved)
            .count(),
        2
    );
}

#[test]
fn go_range_declarations_shadow_only_the_loop_body() {
    let source = "package p\nfunc Target() []func() { return nil }\nfunc Run() {\n Target()\n for _, Target := range Target() { Target() }\n Target()\n}\n";
    let result = inspect(&[input("main.go", Language::Go, source, &["p"])]);
    assert!(result.files[0].complete);
    let references = calls(&result);
    assert_eq!(references.len(), 4);
    assert_eq!(
        references.iter().map(|r| r.status).collect::<Vec<_>>(),
        [
            ResolutionStatus::Resolved,
            ResolutionStatus::Resolved,
            ResolutionStatus::Unresolved,
            ResolutionStatus::Resolved,
        ]
    );
    assert!(references[2].candidates.is_empty());
    let span = references[2].reference.span;
    assert_eq!(&source[span.start..span.end], "Target");
    assert_eq!(target_names(&result), ["Target", "Target", "Target"]);
}
#[test]
fn go_duplicates_namespaces_and_unselected_builds_never_create_calls() {
    let mut inputs = vec![
        input(
            "a.go",
            Language::Go,
            "package p\nfunc Run(){Same()}\nfunc Same(){}\n",
            &["p"],
        ),
        input("b.go", Language::Go, "package p\nfunc Same(){}\n", &["p"]),
    ];
    assert_eq!(
        calls(&inspect(&inputs))[0].status,
        ResolutionStatus::Ambiguous
    );
    inputs[1].module.as_mut().expect("module").namespace = "other".into();
    assert_eq!(target_names(&inspect(&inputs)), ["Same"]);
    inputs[1] = input(
        "platform_linux.go",
        Language::Go,
        "package p\nfunc Same(){}\n",
        &["p"],
    );
    assert_ne!(
        calls(&inspect(&inputs))[0].status,
        ResolutionStatus::Resolved
    );
}
#[test]
fn java_packages_imports_static_calls_and_type_relations() {
    let result = inspect(&[
        input(
            "a/Child.java",
            Language::Java,
            "package a;import b.Base;import b.Store;class Child extends Base implements Store {public static void run(){Base.work(); helper();} private static void helper(){}}",
            &["Child"],
        ),
        input(
            "b/Base.java",
            Language::Java,
            "package b;public class Base {public static void work(){}}",
            &["Base"],
        ),
        input(
            "b/Store.java",
            Language::Java,
            "package b;public interface Store {}",
            &["Store"],
        ),
    ]);
    assert_eq!(target_names(&result), ["Base::work", "Child::helper"]);
    let types = result
        .references
        .iter()
        .filter(|r| {
            matches!(
                r.reference.kind,
                RelationKind::Extends | RelationKind::Implements
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(types.len(), 2);
    assert!(types.iter().all(|r| r.status == ResolutionStatus::Resolved));
}
#[test]
fn java_overloads_virtual_receivers_and_private_access_stay_explicit() {
    let result = inspect(&[
        input(
            "a/A.java",
            Language::Java,
            "package a;class A {public static void f(){}public static void f(int x){}public void virtualCall(){}public static void run(A obj){f(); obj.virtualCall();}}",
            &[],
        ),
        input(
            "b/B.java",
            Language::Java,
            "package b;import a.A;class B{static void run(){A.hidden();}}",
            &[],
        ),
    ]);
    assert!(
        calls(&result)
            .iter()
            .all(|r| r.status != ResolutionStatus::Resolved)
    );
    assert_eq!(calls(&result)[0].status, ResolutionStatus::Ambiguous);
}
#[test]
fn c_calls_includes_and_compilation_unit_isolation() {
    let result = inspect(&[
        input(
            "src/a.c",
            Language::C,
            "#include \"helper.h\"\nstatic void local(void){}\nvoid run(void){local(); leaf(); foreign();}\n",
            &["a"],
        ),
        input(
            "src/helper.h",
            Language::C,
            "static inline void leaf(void){}\n",
            &["helper"],
        ),
        input("src/b.c", Language::C, "void foreign(void){}\n", &["b"]),
    ]);
    assert_eq!(target_names(&result), ["local", "leaf"]);
    assert_ne!(calls(&result)[2].status, ResolutionStatus::Resolved);
    assert!(
        result
            .references
            .iter()
            .filter(|r| r.reference.kind == RelationKind::Import)
            .all(|r| r.status == ResolutionStatus::Resolved)
    );
}
#[test]
fn c_prototypes_pointer_shadowing_macros_and_conditions_do_not_guess() {
    let result = inspect(&[input(
        "a.c",
        Language::C,
        "void local(void);\nvoid local(void){}\n#define MACRO() local()\nvoid run(void){local();MACRO();}\n#if UNKNOWN\nvoid hidden(void){}\n#endif\nvoid later(void){hidden();}\n",
        &["a"],
    )]);
    assert_eq!(target_names(&result), ["local"]);
    assert_eq!(calls(&result).len(), 3);
}
#[test]
fn cpp_namespaces_multiple_bases_and_static_methods() {
    let result = inspect(&[input(
        "a.cpp",
        Language::Cpp,
        "namespace n {struct Base {};struct Other {};struct Child: Base, Other {static void leaf(){}};void helper(){Child::leaf();}}\nvoid run(){n::helper();}\n",
        &["a"],
    )]);
    assert_eq!(target_names(&result), ["n::Child::leaf", "n::helper"]);
    let bases = result
        .references
        .iter()
        .filter(|r| r.reference.kind == RelationKind::Extends)
        .collect::<Vec<_>>();
    assert_eq!(bases.len(), 2);
    assert!(bases.iter().all(|r| r.status == ResolutionStatus::Resolved));
}

#[test]
fn cpp_global_qualification_bypasses_namespace_and_parameter_shadowing() {
    let child = input(
        "child.cpp",
        Language::Cpp,
        "#include \"base.h\"\nnamespace ns {struct GlobalChild : ::Base {};struct LocalChild : Base {};void Run(void (*leaf)()) { ::leaf(); leaf(); }}\n",
        &[],
    );
    let base = input(
        "base.h",
        Language::Cpp,
        "struct Base {};void leaf() {}\nnamespace ns {struct Base {};void leaf() {}}\n",
        &[],
    );
    let result = inspect(&[child.clone(), base]);
    let bases = result
        .references
        .iter()
        .filter(|r| r.reference.kind == RelationKind::Extends)
        .collect::<Vec<_>>();
    assert_eq!(bases.len(), 2);
    for (reference, name) in bases.iter().zip([vec!["Base"], vec!["ns", "Base"]]) {
        assert_eq!(reference.status, ResolutionStatus::Resolved);
        let target = &reference.candidates[0];
        assert_eq!(target.path, "base.h");
        let node = result.files[1]
            .nodes
            .iter()
            .find(|n| n.id == target.node_id)
            .expect("base node");
        assert_eq!(node.qualified_name, name);
    }
    assert_eq!(target_names(&result), ["leaf"]);
    assert_eq!(calls(&result)[1].status, ResolutionStatus::Unresolved);
    assert!(calls(&result)[1].candidates.is_empty());

    for (declarations, expected) in [
        (
            "namespace ns {struct Base {};}",
            ResolutionStatus::Unresolved,
        ),
        (
            "struct Base {};struct Base {};namespace ns {struct Base {};}",
            ResolutionStatus::Ambiguous,
        ),
    ] {
        let result = inspect(&[
            child.clone(),
            input("base.h", Language::Cpp, declarations, &[]),
        ]);
        let reference = result
            .references
            .iter()
            .find(|r| r.reference.kind == RelationKind::Extends)
            .expect("global base reference");
        assert_eq!(reference.status, expected);
        if expected == ResolutionStatus::Unresolved {
            assert!(reference.candidates.is_empty());
        }
    }
}
#[test]
fn cpp_overloads_templates_and_unknown_receivers_are_unresolved() {
    let result = inspect(&[input(
        "a.cpp",
        Language::Cpp,
        "void f(){} void f(int x){} template<class T> void generic(T x){}\nstruct A{virtual void method(){}};void run(A& obj){f();obj.method();generic(1);}\n",
        &["a"],
    )]);
    assert!(
        calls(&result)
            .iter()
            .all(|r| r.status != ResolutionStatus::Resolved)
    );
    assert_eq!(calls(&result)[0].status, ResolutionStatus::Ambiguous);
}
#[test]
fn compiled_sources_keep_utf8_bom_crlf_locations_and_cross_language_isolation() {
    for (language, path, text) in [
        (
            Language::Go,
            "a.go",
            "package p\r\n// 中文\r\nfunc leaf(){}\r\nfunc run(){leaf()}\r\n",
        ),
        (
            Language::Java,
            "A.java",
            "// 中文\r\nclass A{static void leaf(){}static void run(){leaf();}}\r\n",
        ),
        (
            Language::C,
            "a.c",
            "// 中文\r\nvoid leaf(){}\r\nvoid run(){leaf();}\r\n",
        ),
        (
            Language::Cpp,
            "a.cpp",
            "// 中文\r\nvoid leaf(){}\r\nvoid run(){leaf();}\r\n",
        ),
    ] {
        let mut source = input(path, language, text, &[]);
        source.bytes.splice(0..0, [0xef, 0xbb, 0xbf]);
        let result = inspect(&[source]);
        let call = calls(&result)[0];
        assert_eq!(call.status, ResolutionStatus::Resolved);
        let span = call.reference.span;
        assert!(text[span.start..span.end].starts_with("leaf"));
        assert!(
            result.files[0]
                .nodes
                .iter()
                .any(|n| n.name == "leaf" && !n.entity_indices.is_empty())
        );
    }
    let result = inspect(&[
        input(
            "a.go",
            Language::Go,
            "package p\nfunc run(){foreign()}\n",
            &[],
        ),
        input("a.c", Language::C, "void foreign(){}", &[]),
    ]);
    assert_ne!(calls(&result)[0].status, ResolutionStatus::Resolved);
}

#[test]
fn c_type_uses_pointer_variables_and_macros_do_not_create_false_symbols_or_calls() {
    let result = inspect(&[input(
        "a.c",
        Language::C,
        "struct Record {int value;};void known(void){}\nvoid run(struct Record* r){void (*known)(void)=0;known();}\nstruct Record global;\n#define shadow() 0\nvoid shadow(void){}void later(){shadow();}\n",
        &[],
    )]);
    assert_eq!(
        result.files[0]
            .nodes
            .iter()
            .filter(|n| n.name == "Record")
            .count(),
        1
    );
    assert!(
        calls(&result)
            .iter()
            .all(|r| r.status != ResolutionStatus::Resolved)
    );
}
#[test]
fn java_type_parameters_and_named_packages_do_not_resolve_unrelated_types() {
    let result = inspect(&[
        input(
            "a/A.java",
            Language::Java,
            "package a;class A<T> extends T {static void run(){Foreign.leaf();}}",
            &[],
        ),
        input(
            "Foreign.java",
            Language::Java,
            "public class Foreign {public static void leaf(){}}",
            &[],
        ),
        input(
            "a/T.java",
            Language::Java,
            "package a;public class T{}",
            &[],
        ),
    ]);
    assert!(
        result
            .references
            .iter()
            .filter(|r| matches!(r.reference.kind, RelationKind::Call | RelationKind::Extends))
            .all(|r| r.status != ResolutionStatus::Resolved)
    );
}
#[test]
fn cpp_private_and_conditional_types_do_not_create_public_inheritance_or_calls() {
    let result = inspect(&[input(
        "a.cpp",
        Language::Cpp,
        "class A{static void hidden(){}};void run(){A::hidden();}\n#if UNKNOWN\nstruct Maybe{};\n#endif\nstruct Child:Maybe{};",
        &[],
    )]);
    assert!(
        result
            .references
            .iter()
            .filter(|r| matches!(r.reference.kind, RelationKind::Call | RelationKind::Extends))
            .all(|r| r.status != ResolutionStatus::Resolved)
    );
}

#[test]
fn go_embedded_declarations_and_interface_methods_are_symbols_without_inheritance() {
    let result = inspect(&[input(
        "a.go",
        Language::Go,
        "package p\ntype Base struct{}\ntype Child struct{Base}\ntype Store interface{Read() int}\n",
        &[],
    )]);
    assert!(
        result.files[0]
            .nodes
            .iter()
            .any(|n| n.qualified_name == ["Child", "Base"]
                && n.entity_kind == zg_knowledge::relations::EntityKind::Field)
    );
    assert!(
        result.files[0]
            .nodes
            .iter()
            .any(|n| n.qualified_name == ["Store", "Read"]
                && n.entity_kind == zg_knowledge::relations::EntityKind::Method
                && !n.callable)
    );
    assert!(result.references.is_empty());
}
#[test]
fn c_prototype_and_function_pointer_parameter_names_do_not_shadow_other_functions() {
    let result = inspect(&[input(
        "a.c",
        Language::C,
        "void prototype(int leaf);void (*pointer)(int leaf);void leaf(){}void run(){leaf();}\n",
        &[],
    )]);
    assert_eq!(target_names(&result), ["leaf"]);
}
#[test]
fn go_filename_constraints_require_a_suffix_and_cover_known_targets() {
    let result = inspect(&[
        input(
            "linux.go",
            Language::Go,
            "package p\nfunc Plain(){}\n",
            &["p"],
        ),
        input(
            "special_aix.go",
            Language::Go,
            "package p\nfunc Aix(){}\n",
            &["p"],
        ),
        input(
            "special_riscv64.go",
            Language::Go,
            "package p\nfunc Riscv(){}\n",
            &["p"],
        ),
    ]);
    assert!(result.files[0].complete);
    assert!(!result.files[1].complete);
    assert!(!result.files[2].complete);
}

#[test]
fn c_guarded_linkage_and_opaque_export_annotations_keep_original_spans() {
    let source = "// 中\r\n#ifndef HEADER_H\r\n#define HEADER_H\r\n#ifdef __cplusplus\r\nextern \"C\" {\r\n#endif\r\nAPI_DLL void* allocate(int size);\r\nAPI_DLL PRINTF_ATTR int report(const char* message);\r\nstatic inline int run(void) { return report(\"ok\"); }\r\n#ifdef __cplusplus\r\n}\r\n#endif\r\n#endif\r\n";
    let result = inspect(&[input("api.h", Language::C, source, &[])]);
    let facts = &result.files[0];
    assert!(facts.complete, "{:?}", facts.diagnostics);
    for name in ["allocate", "report", "run"] {
        let node = facts
            .nodes
            .iter()
            .find(|node| node.name == name)
            .expect("declaration");
        assert!(source[node.span.start..node.span.end].contains(name));
    }
    assert!(
        facts
            .diagnostics
            .iter()
            .any(|d| d.code == "macro_declaration_target_unresolved")
    );
    assert_eq!(calls(&result).len(), 1);
    assert_eq!(calls(&result)[0].status, ResolutionStatus::Unresolved);
}

#[test]
fn c_opaque_macro_declarations_do_not_invent_a_static_target() {
    let source = "API_DLL void target(void);\nvoid caller(void) { target(); }\n";
    let result = inspect(&[input("api.h", Language::C, source, &[])]);
    assert!(result.files[0].complete);
    assert_eq!(calls(&result)[0].status, ResolutionStatus::Unresolved);
}

#[test]
fn cpp_braced_default_parameters_parse_and_keep_call_evidence() {
    let source = "void helper() {}\nvoid run(Options options = {}) { helper(); }\nvoid other(Options options = {1, {2, 3}});\n";
    let result = inspect(&[input("defaults.cc", Language::Cpp, source, &[])]);
    assert!(
        result.files[0].complete,
        "{:?}",
        result.files[0].diagnostics
    );
    assert_eq!(target_names(&result), ["helper"]);
    let reference = &calls(&result)[0].reference;
    assert_eq!(&source[reference.span.start..reference.span.end], "helper");
}

#[test]
fn c_family_grammar_repairs_still_reject_missing_tokens() {
    for (language, path, source) in [
        (
            Language::C,
            "bad.h",
            "#ifdef __cplusplus\nextern \"C\" {\n#endif\nint f(void);\n",
        ),
        (Language::C, "bad.h", "API_DLL void f(int value;\n"),
        (Language::Cpp, "bad.cc", "void f(Options value = {);\n"),
    ] {
        let result = inspect(&[input(path, language, source, &[])]);
        assert!(!result.files[0].complete, "{source}");
        assert!(
            result.files[0]
                .diagnostics
                .iter()
                .any(|d| d.code == "syntax_error")
        );
    }
}

#[test]
fn cpp_included_bases_follow_namespace_lookup_and_keep_ambiguity() {
    let child = input(
        "child.h",
        Language::Cpp,
        "#include \"base.h\"\nnamespace outer::inner {struct Child:Base{};}\n",
        &[],
    );
    let base = input(
        "base.h",
        Language::Cpp,
        "namespace outer {struct Base{};} namespace unrelated {struct Base{};}",
        &[],
    );
    let result = inspect(&[child.clone(), base.clone()]);
    let edge = result
        .references
        .iter()
        .find(|r| r.reference.kind == RelationKind::Extends)
        .expect("base reference");
    assert_eq!(edge.status, ResolutionStatus::Resolved);
    assert_eq!(edge.candidates[0].path, "base.h");
    let missing = inspect(std::slice::from_ref(&child));
    assert_ne!(
        missing
            .references
            .iter()
            .find(|r| r.reference.kind == RelationKind::Extends)
            .expect("base")
            .status,
        ResolutionStatus::Resolved
    );
    let duplicate = input(
        "base.h",
        Language::Cpp,
        "namespace outer::inner {struct Base{};struct Base{};}",
        &[],
    );
    let result = inspect(&[child, duplicate]);
    assert_eq!(
        result
            .references
            .iter()
            .find(|r| r.reference.kind == RelationKind::Extends)
            .expect("base")
            .status,
        ResolutionStatus::Ambiguous
    );
}
