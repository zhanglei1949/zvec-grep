#![cfg(feature = "knowledge-graph")]
use zg_engine::graph_facts::{
    FactInput, FactInspection, inspect_sources,
    relations::{CodeModule, Language, RelationKind, ResolutionStatus},
};
use zg_knowledge::processing::ProcessingContext;

fn inspect(language: Language, sources: &[(&str, &[&str], &str)]) -> FactInspection {
    let inputs = sources
        .iter()
        .map(|(path, module, text)| FactInput {
            path: (*path).into(),
            language,
            bytes: text.as_bytes().to_vec(),
            module: Some(CodeModule {
                namespace: "coverage".into(),
                path: module.iter().map(|s| (*s).into()).collect(),
                is_package: false,
            }),
        })
        .collect::<Vec<_>>();
    inspect_sources(
        "coverage",
        &inputs,
        &ProcessingContext {
            max_input_bytes: 1 << 20,
            max_output_items: 100_000,
            deadline: None,
            cancellation: tokio_util::sync::CancellationToken::new(),
        },
    )
    .expect("valid facts")
}
fn named_calls(result: &FactInspection, text: &str, name: &str) -> Vec<ResolutionStatus> {
    result
        .references
        .iter()
        .filter(|r| {
            r.reference.kind == RelationKind::Call
                && text.get(r.reference.span.start..r.reference.span.end) == Some(name)
        })
        .map(|r| r.status)
        .collect()
}
fn rust_case(text: &str, expected: &[ResolutionStatus]) {
    let r = inspect(Language::Rust, &[("lib.rs", &[], text)]);
    assert!(r.files[0].complete, "{:?}", r.files[0].diagnostics);
    assert_eq!(named_calls(&r, text, "helper"), expected, "{text}");
}
#[test]
fn rust_conditions_patterns_and_initializer_scopes() {
    use ResolutionStatus::{Resolved as R, Unresolved as U};
    for (text, states) in [
        (
            "fn helper() {} fn run(a:Option<fn()>){ if let Some(helper)=a && {helper();true} {helper();} else {helper();} helper();}",
            vec![U, U, R, R],
        ),
        (
            "fn helper() {} fn run(a:Option<fn()>){while let Some(helper)=a {helper();} helper();}",
            vec![U, R],
        ),
        (
            "fn helper() {} fn run(a:Option<fn()>){match a {Some(helper) if {helper();true}=>helper(), _=>helper()} helper();}",
            vec![U, U, R, R],
        ),
        (
            "fn helper()->Vec<fn()> {vec![]} fn run(){for helper in helper(){helper();} helper();}",
            vec![R, U, R],
        ),
        (
            "fn helper()->Option<fn()> {None} fn run(){let Some(helper)=helper() else {helper();return;};helper();}",
            vec![R, R, U],
        ),
        (
            "struct Wrap{helper:fn()} fn helper(){} fn run(w:Wrap){let Wrap{helper:other}=w;helper();other();}",
            vec![R],
        ),
        (
            "fn helper(){} fn run(v:Option<fn()>){match v {Some(helper)|Some(helper)=>helper(),_=>helper()}}",
            vec![U, R],
        ),
    ] {
        rust_case(text, &states);
    }
}
#[test]
fn rust_glob_bindings_preserve_explicit_precedence_and_references() {
    let text = "fn helper(){} mod inner {use super::*;fn run(){helper();}}";
    let r = inspect(Language::Rust, &[("lib.rs", &[], text)]);
    assert!(r.files[0].complete);
    assert_eq!(
        named_calls(&r, text, "helper"),
        [ResolutionStatus::Resolved]
    );
    assert_eq!(
        r.references
            .iter()
            .filter(|r| r.reference.kind == RelationKind::Import)
            .count(),
        1
    );
    let text =
        "mod a{pub fn helper(){}} mod b{pub fn helper(){}} use a::*;use b::*;fn run(){helper();}";
    let r = inspect(Language::Rust, &[("lib.rs", &[], text)]);
    assert_eq!(
        named_calls(&r, text, "helper"),
        [ResolutionStatus::Ambiguous]
    );
    let text = "mod a{pub fn helper(){}} mod b{pub fn helper(){}} use a::*;use b::*;fn helper(){} fn run(){helper();}";
    let r = inspect(Language::Rust, &[("lib.rs", &[], text)]);
    assert_eq!(
        named_calls(&r, text, "helper"),
        [ResolutionStatus::Resolved]
    );
}
#[test]
fn rust_globs_reexports_visibility_unknowns_and_cycles() {
    for (text, expected) in [
        (
            "mod a{pub fn helper(){}} mod b{pub use crate::a::*;} use b::*;fn run(){helper();}",
            ResolutionStatus::Resolved,
        ),
        (
            "mod a{fn helper(){}} use a::*;fn run(){helper();}",
            ResolutionStatus::Unresolved,
        ),
        (
            "mod a{pub(crate) fn helper(){}} use a::*;fn run(){helper();}",
            ResolutionStatus::Resolved,
        ),
        (
            "use external::*;fn run(){helper();}",
            ResolutionStatus::Unresolved,
        ),
        (
            "fn helper(){} fn run(){use external::*;helper();}",
            ResolutionStatus::Unresolved,
        ),
        (
            "fn helper(){} use external::*;fn run(){helper();}",
            ResolutionStatus::Resolved,
        ),
        (
            "mod a{pub use crate::b::*;} mod b{pub use crate::a::*;} use a::*;fn run(){helper();}",
            ResolutionStatus::Unresolved,
        ),
        (
            "mod a{mod hidden{pub fn helper(){}} pub use self::hidden::helper;} use a::*;fn run(){helper();}",
            ResolutionStatus::Resolved,
        ),
        (
            "mod a{mod hidden{pub fn helper(){}} pub use self::hidden::*;} use a::*;fn run(){helper();}",
            ResolutionStatus::Resolved,
        ),
        (
            "mod a{pub mod b{pub fn helper(){}} pub use self::b::*;fn helper(){}} use a::*;fn run(){helper();}",
            ResolutionStatus::Unresolved,
        ),
        (
            "mod a{pub fn helper(){}} use a::*;use external::*;fn run(){helper();}",
            ResolutionStatus::Unresolved,
        ),
        (
            "fn helper(){} fn run(){use external::*;crate::helper();}",
            ResolutionStatus::Resolved,
        ),
        (
            "enum E{helper} use E::*;fn run(){helper();}",
            ResolutionStatus::Unresolved,
        ),
    ] {
        let r = inspect(Language::Rust, &[("lib.rs", &[], text)]);
        assert!(r.files[0].complete, "{text}");
        let name = if text.contains("crate::helper();") {
            "crate::helper"
        } else {
            "helper"
        };
        assert_eq!(
            named_calls(&r, text, name),
            [expected],
            "{text}\nreferences={:?}\nbindings={:?}\nscopes={:?}",
            r.references,
            r.files[0].bindings,
            r.files[0].scopes
        );
    }
}
#[test]
fn python_comprehensions_keep_first_iterable_outside_and_targets_inside() {
    use ResolutionStatus::{Resolved as R, Unresolved as U};
    for (text, states) in [
        (
            "def helper(): return []\ndef run(xs):\n    return [helper() for helper in helper()]\n",
            vec![U, R],
        ),
        (
            "def helper(): pass\ndef run(xs):\n    [helper() for x in xs]\n    helper()\n",
            vec![R, R],
        ),
        (
            "def helper(): pass\ndef run(xs):\n    (helper() for helper in xs)\n    helper()\n",
            vec![U, R],
        ),
        (
            "def helper(): pass\ndef run(xs):\n    [x for x in xs if (helper := x)]\n    helper()\n",
            vec![U],
        ),
        (
            "def helper(): return []\ndef run(xs):\n    return [(helper() for x in helper()) for helper in xs]\n",
            vec![U, U],
        ),
    ] {
        let r = inspect(Language::Python, &[("a.py", &["a"], text)]);
        assert!(r.files[0].complete, "{:?}", r.files[0].diagnostics);
        assert_eq!(named_calls(&r, text, "helper"), states, "{text}");
    }
}
#[test]
fn python_with_except_delete_and_case_are_function_local_bindings() {
    for body in [
        "    with context() as helper:\n        helper()\n",
        "    try: pass\n    except Exception as helper:\n        helper()\n    helper()\n",
        "    try: pass\n    except* Exception as helper:\n        helper()\n",
        "    del helper\n    helper()\n",
        "    helper: object\n    helper()\n",
        "    match value:\n        case {'key': helper}:\n            helper()\n",
    ] {
        let text = format!("def helper(): pass\ndef run(value):\n{body}");
        let r = inspect(Language::Python, &[("a.py", &["a"], &text)]);
        assert!(r.files[0].complete, "{:?}", r.files[0].diagnostics);
        let calls = named_calls(&r, &text, "helper");
        assert!(!calls.is_empty());
        assert!(
            calls.iter().all(|s| *s == ResolutionStatus::Unresolved),
            "{text}"
        );
    }
}
#[test]
fn python_global_nonlocal_routes_and_mutations_survive_collection_order() {
    let text = "def helper(): pass\ndef outer():\n    def helper(): pass\n    def inner():\n        global helper\n        helper()\n";
    let r = inspect(Language::Python, &[("a.py", &["a"], text)]);
    assert_eq!(
        named_calls(&r, text, "helper"),
        [ResolutionStatus::Resolved]
    );
    let call = r
        .references
        .iter()
        .find(|r| r.reference.kind == RelationKind::Call)
        .expect("call");
    let target = r.files[0]
        .nodes
        .iter()
        .find(|n| n.id == call.candidates[0].node_id)
        .expect("target");
    assert_eq!(target.qualified_name, ["helper"]);
    for body in [
        "global helper\n    helper = replacement",
        "global helper\n    del helper",
    ] {
        let text = format!("def helper():pass\ndef mutate():\n    {body}\ndef run():helper()\n");
        let r = inspect(Language::Python, &[("a.py", &["a"], &text)]);
        assert!(r.files[0].complete);
        assert_eq!(
            named_calls(&r, &text, "helper"),
            [ResolutionStatus::Unresolved]
        );
    }
    let text = "def helper():pass\ndef outer():\n    def inner():\n        nonlocal helper\n        helper()\n    def helper():pass\n";
    let r = inspect(Language::Python, &[("a.py", &["a"], text)]);
    assert!(r.files[0].complete);
    assert_eq!(
        named_calls(&r, text, "helper"),
        [ResolutionStatus::Resolved]
    );
}
#[test]
fn python_defaults_are_evaluated_in_enclosing_scope() {
    let text = "def helper():pass\ndef run(helper=helper()):\n    helper()\n";
    let r = inspect(Language::Python, &[("a.py", &["a"], text)]);
    assert_eq!(
        named_calls(&r, text, "helper"),
        [ResolutionStatus::Resolved, ResolutionStatus::Unresolved]
    );
}

#[test]
fn python_dotted_imports_keep_explicitly_loaded_namespace_packages() {
    let source = "import pkg.calc\npkg.calc.helper()\n";
    for with_package in [false, true] {
        let mut inputs = vec![
            ("app.py", &["app"][..], source),
            ("pkg/calc.py", &["pkg", "calc"][..], "def helper():pass\n"),
        ];
        if with_package {
            inputs.push(("pkg/__init__.py", &["pkg"][..], ""));
        }
        let r = inspect(Language::Python, &inputs);
        assert_eq!(r.references.len(), 2);
        assert!(
            r.references
                .iter()
                .all(|r| r.status == ResolutionStatus::Resolved),
            "{:?}",
            r.references
        );
    }
    let r = inspect(
        Language::Python,
        &[
            ("app.py", &["app"], "import pkg\npkg.calc.helper()\n"),
            ("pkg/__init__.py", &["pkg"], "calc = object()\n"),
            ("pkg/calc.py", &["pkg", "calc"], "def helper():pass\n"),
        ],
    );
    assert!(
        r.references
            .iter()
            .filter(|r| r.reference.kind == RelationKind::Call)
            .all(|r| r.status != ResolutionStatus::Resolved)
    );
}

#[test]
fn python_dotted_import_does_not_override_rebound_root() {
    let r = inspect(
        Language::Python,
        &[
            (
                "app.py",
                &["app"],
                "import pkg.calc\npkg = object()\npkg.calc.helper()\n",
            ),
            ("pkg/calc.py", &["pkg", "calc"], "def helper():pass\n"),
        ],
    );
    let call = r
        .references
        .iter()
        .find(|r| r.reference.kind == RelationKind::Call && r.reference.span.start > 20)
        .expect("qualified call");
    assert_eq!(call.status, ResolutionStatus::Unresolved);
}
