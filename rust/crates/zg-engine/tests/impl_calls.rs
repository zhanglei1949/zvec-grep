#![cfg(feature = "knowledge-graph")]
use zg_engine::graph_facts::{
    FactInput, FactInspection, inspect_sources,
    relations::{CodeModule, EntityKind, Language, RelationKind, ResolutionStatus},
};
use zg_knowledge::processing::ProcessingContext;

fn inspect(sources: &[(&str, &[&str], &str)]) -> FactInspection {
    let inputs = sources
        .iter()
        .map(|(path, module, text)| FactInput {
            path: (*path).into(),
            language: Language::Rust,
            bytes: text.as_bytes().to_vec(),
            module: Some(CodeModule {
                namespace: "impl-fixture".into(),
                path: module.iter().map(|p| (*p).into()).collect(),
                is_package: false,
            }),
        })
        .collect::<Vec<_>>();
    inspect_sources(
        "impl-fixture",
        &inputs,
        &ProcessingContext {
            max_input_bytes: 1 << 20,
            max_output_items: 100_000,
            deadline: None,
            cancellation: tokio_util::sync::CancellationToken::new(),
        },
    )
    .expect("inspect")
}
fn check(
    result: &FactInspection,
    path: &str,
    text: &str,
    expr: &str,
    expected: ResolutionStatus,
    target: Option<(&str, &str)>,
) {
    let refs = result
        .references
        .iter()
        .filter(|r| {
            r.source.path == path
                && r.reference.kind == RelationKind::Call
                && text.get(r.reference.span.start..r.reference.span.end) == Some(expr)
        })
        .collect::<Vec<_>>();
    assert!(!refs.is_empty(), "missing call {expr}");
    for r in refs {
        assert_eq!(
            r.status, expected,
            "{expr}: {} {:?}",
            r.reason, r.candidates
        );
        if let Some((target_path, qualified)) = target {
            assert_eq!(r.candidates.len(), 1);
            let t = &r.candidates[0];
            assert_eq!(t.path, target_path);
            let n = result
                .files
                .iter()
                .find(|f| f.version.path == t.path)
                .expect("target source")
                .nodes
                .iter()
                .find(|n| n.id == t.node_id)
                .expect("target node");
            assert_eq!(n.qualified_name.join("::"), qualified);
            assert!(n.callable);
            assert_eq!(n.entity_kind, EntityKind::Method);
        }
    }
}
#[test]
fn inherent_type_self_and_aliases_preserve_occurrences_and_lexical_owner() {
    use ResolutionStatus::Resolved as R;
    let text = "fn f(){} struct A; use A as Renamed; type Alias=A; type Chain=Alias; impl A {fn f(){} fn recur(){Self::f();Self::f();A::f();f();} fn method(&self){}} impl A{fn g(){Self::f();}} fn run(a:&A){A::f();Renamed::f();Alias::f();Chain::f();A::method(a);}";
    let r = inspect(&[("lib.rs", &[], text)]);
    assert!(r.files[0].complete);
    for expr in ["Self::f", "A::f", "Renamed::f", "Alias::f", "Chain::f"] {
        check(&r, "lib.rs", text, expr, R, Some(("lib.rs", "impl:A::f")));
    }
    check(
        &r,
        "lib.rs",
        text,
        "A::method",
        R,
        Some(("lib.rs", "impl:A::method")),
    );
    let bare = r
        .references
        .iter()
        .find(|r| text.get(r.reference.span.start..r.reference.span.end) == Some("f"))
        .expect("unqualified free call");
    assert_eq!(bare.status, R);
    assert_eq!(
        bare.candidates[0].node_id,
        r.files[0]
            .nodes
            .iter()
            .find(|n| n.qualified_name == ["f"])
            .expect("free function declaration")
            .id
    );
    assert_eq!(
        r.references
            .iter()
            .filter(|r| text.get(r.reference.span.start..r.reference.span.end) == Some("Self::f"))
            .count(),
        3
    );
    let method = r.files[0]
        .nodes
        .iter()
        .find(|n| n.qualified_name == ["impl:A", "f"])
        .expect("associated function declaration");
    assert_eq!(method.parent.as_ref(), Some(&r.files[0].nodes[0].id));
    assert_eq!(
        r.files[0]
            .nodes
            .iter()
            .filter(|n| n.entity_kind == EntityKind::Method)
            .count(),
        4
    );
}
#[test]
fn out_of_line_impl_alias_reexport_and_visibility() {
    use ResolutionStatus::{Resolved as R, Unresolved as U};
    let root = "mod types;mod methods;use crate::types::A as Imported;type Alias=Imported;fn root(){Imported::public();Alias::public();types::A::public();types::A::private();}";
    let types = "pub struct A; pub type Export=A;";
    let methods = "use crate::types::Export as Local;impl Local {pub fn public(){Self::private();}fn private(){}}";
    let r = inspect(&[
        ("lib.rs", &[], root),
        ("types.rs", &["types"], types),
        ("methods.rs", &["methods"], methods),
    ]);
    for expr in ["Imported::public", "Alias::public", "types::A::public"] {
        check(
            &r,
            "lib.rs",
            root,
            expr,
            R,
            Some(("methods.rs", "impl:Local::public")),
        );
    }
    check(&r, "lib.rs", root, "types::A::private", U, None);
    check(
        &r,
        "methods.rs",
        methods,
        "Self::private",
        R,
        Some(("methods.rs", "impl:Local::private")),
    );
}
#[test]
fn qualified_enum_paths_and_globs_share_nominal_members() {
    use ResolutionStatus::{Resolved as R, Unresolved as U};
    let text = "enum E{V} impl E{fn f(){Self::f();}} fn run(){E::f();crate::E::f();} mod child{use super::E::*; fn run(){V();}}";
    let r = inspect(&[("lib.rs", &[], text)]);
    check(&r, "lib.rs", text, "E::f", R, Some(("lib.rs", "impl:E::f")));
    check(
        &r,
        "lib.rs",
        text,
        "crate::E::f",
        R,
        Some(("lib.rs", "impl:E::f")),
    );
    check(
        &r,
        "lib.rs",
        text,
        "Self::f",
        R,
        Some(("lib.rs", "impl:E::f")),
    );
    check(&r, "lib.rs", text, "V", U, None);
}
#[test]
fn ambiguous_type_and_duplicate_method_never_resolve_through_common_tail() {
    use ResolutionStatus::Ambiguous as A;
    let text = "struct T;impl T{fn f(){}}impl T{fn f(){}}fn run(){T::f();}";
    check(
        &inspect(&[("lib.rs", &[], text)]),
        "lib.rs",
        text,
        "T::f",
        A,
        None,
    );
    let text = "mod a{pub struct T;impl T{pub fn f(){}}}mod b{pub struct T;}use a::*;use b::*;fn run(){T::f();}";
    check(
        &inspect(&[("lib.rs", &[], text)]),
        "lib.rs",
        text,
        "T::f",
        A,
        None,
    );
}
#[test]
fn receivers_traits_generics_and_complex_aliases_remain_unresolved() {
    use ResolutionStatus::{Resolved as R, Unresolved as U};
    let text = "struct T;impl T{fn f(){}fn run(&self){self.f();let obj=self;obj.f();fn nested(){Self::f();}}}trait Trait{fn trait_f(){}}impl Trait for T{fn trait_f(){Self::f();}}type Bad=&'static T;type Generic<X>=T;fn generic<T>(){T::f();}fn run(){Bad::f();Generic::f();T::trait_f();T::f();<T as Trait>::trait_f();}";
    let r = inspect(&[("lib.rs", &[], text)]);
    assert!(r.files[0].complete);
    for expr in [
        "self.f",
        "obj.f",
        "Self::f",
        "T::trait_f",
        "Bad::f",
        "Generic::f",
        "<T as Trait>::trait_f",
    ] {
        check(&r, "lib.rs", text, expr, U, None);
    }
    let calls = r
        .references
        .iter()
        .filter(|r| text.get(r.reference.span.start..r.reference.span.end) == Some("T::f"))
        .map(|r| r.status)
        .collect::<Vec<_>>();
    assert_eq!(calls, [U, R], "generic T shadows the module type");
}
#[test]
fn generic_or_conditional_impl_cannot_create_a_false_unique_method() {
    use ResolutionStatus::Unresolved as U;
    for text in [
        "struct T<X>(X);impl<X> T<X>{fn f(){Self::f();}}fn run(){T::f();}",
        "struct T<X>(X);impl T<u8>{fn f(){Self::f();}}impl T<u16>{fn f(){}}fn run(){T::f();}",
        "struct T;impl T{fn f(){}}impl T where T:Send{fn f(){}}fn run(){T::f();}",
    ] {
        let r = inspect(&[("lib.rs", &[], text)]);
        assert!(r.files[0].complete);
        check(&r, "lib.rs", text, "T::f", U, None);
    }
}
#[test]
fn cyclic_alias_and_shadowing_do_not_guess_a_type() {
    use ResolutionStatus::Unresolved as U;
    for text in [
        "type A=B;type B=A;fn run(){A::f();}",
        "struct A;impl A{fn f(){}}fn run(){let A=||{};A::f();}",
    ] {
        check(
            &inspect(&[("lib.rs", &[], text)]),
            "lib.rs",
            text,
            "A::f",
            U,
            None,
        );
    }
}
#[test]
fn deleted_or_newly_ambiguous_method_changes_the_resolution() {
    use ResolutionStatus::{Ambiguous as A, Resolved as R, Unresolved as U};
    let root = "pub struct T;mod methods;fn caller(){T::f();}";
    for (methods, expected) in [
        ("impl crate::T{pub fn f(){}}", R),
        ("impl crate::T{pub fn removed(){}}", U),
        ("impl crate::T{pub fn f(){}}impl crate::T{pub fn f(){}}", A),
        ("impl crate::T{pub fn f(){}}", R),
    ] {
        let r = inspect(&[("lib.rs", &[], root), ("methods.rs", &["methods"], methods)]);
        check(&r, "lib.rs", root, "T::f", expected, None);
        assert!(
            r.references
                .iter()
                .find(|r| r.source.path == "lib.rs" && r.reference.kind == RelationKind::Call)
                .expect("caller reference")
                .dependencies
                .iter()
                .any(|v| v.path == "methods.rs")
        );
    }
}

#[test]
fn self_in_closure_keeps_the_impl_context() {
    let text = "struct T;impl T{fn f(){}fn g(){let callback=||Self::f();}}";
    check(
        &inspect(&[("lib.rs", &[], text)]),
        "lib.rs",
        text,
        "Self::f",
        ResolutionStatus::Resolved,
        Some(("lib.rs", "impl:T::f")),
    );
}

#[test]
fn impl_type_parameters_shadow_module_types_and_self_ignores_globs() {
    let text = "struct T;impl T{fn f(){}fn g(){use external::*;Self::f();}}struct Other<X>(X);impl<T> Other<T>{fn run(){T::f();}}";
    let r = inspect(&[("lib.rs", &[], text)]);
    check(
        &r,
        "lib.rs",
        text,
        "Self::f",
        ResolutionStatus::Resolved,
        Some(("lib.rs", "impl:T::f")),
    );
    check(
        &r,
        "lib.rs",
        text,
        "T::f",
        ResolutionStatus::Unresolved,
        None,
    );
}

#[test]
fn associated_type_defaults_and_item_aliases_do_not_infer_self() {
    let text = "struct T;impl T{fn f(){}fn g(){type Alias=Self;Alias::f();}}trait Trait{type Item=T;}fn run(){Trait::Item::f();}";
    let r = inspect(&[("lib.rs", &[], text)]);
    for expr in ["Alias::f", "Trait::Item::f"] {
        check(&r, "lib.rs", text, expr, ResolutionStatus::Unresolved, None);
    }
}
#[test]
fn associated_lookup_reuses_cancellation_and_work_budgets() {
    let text = "struct T;impl T{fn f(){Self::f();}}fn run(){T::f();}";
    let r = inspect(&[("lib.rs", &[], text)]);
    let mut context = ProcessingContext {
        max_input_bytes: 1 << 20,
        max_output_items: 100_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    };
    context.cancellation.cancel();
    assert!(matches!(
        zg_knowledge::relations::resolve_code(&r.files, &context),
        Err(zg_knowledge::Error::Cancelled)
    ));
    context.cancellation = tokio_util::sync::CancellationToken::new();
    context.deadline = Some(std::time::Instant::now());
    assert!(matches!(
        zg_knowledge::relations::resolve_code(&r.files, &context),
        Err(zg_knowledge::Error::Deadline)
    ));
    context.deadline = None;
    context.max_output_items = r.files[0].nodes.len()
        + r.files[0].scopes.len()
        + r.files[0].bindings.len()
        + r.files[0].references.len()
        + r.files[0].diagnostics.len();
    assert!(matches!(
        zg_knowledge::relations::resolve_code(&r.files, &context),
        Err(zg_knowledge::Error::ResourceExhausted)
    ));
    let mut broken = r.files.clone();
    broken[0].bindings.retain(|b| b.name != "Self");
    context.max_output_items = 100_000;
    assert!(matches!(
        zg_knowledge::relations::resolve_code(&broken, &context),
        Err(zg_knowledge::Error::Provider(_))
    ));
}

#[test]
fn lifetime_only_impls_resolve_type_self_and_import_aliases() {
    use ResolutionStatus::Resolved as R;
    let root = "pub struct View<'a,'b>(&'a str,&'b str);mod methods;use View as Alias;fn caller(){View::entry();Alias::entry();}";
    let methods = "use crate::View as Owner;impl<'a,'b> Owner<'a,'b>{pub fn entry(){Self::leaf();}}impl Owner<'_,'_>{fn leaf(){}}";
    let r = inspect(&[("lib.rs", &[], root), ("methods.rs", &["methods"], methods)]);
    assert!(r.files.iter().all(|f| f.complete));
    check(
        &r,
        "lib.rs",
        root,
        "View::entry",
        R,
        Some(("methods.rs", "impl:Owner<'a,'b>::entry")),
    );
    check(
        &r,
        "lib.rs",
        root,
        "Alias::entry",
        R,
        Some(("methods.rs", "impl:Owner<'a,'b>::entry")),
    );
    check(
        &r,
        "methods.rs",
        methods,
        "Self::leaf",
        R,
        Some(("methods.rs", "impl:Owner<'_,'_>::leaf")),
    );
    assert!(r.files.iter().all(|f| {
        f.diagnostics
            .iter()
            .all(|d| d.code != "conditional_impl_resolution_unsupported")
    }));
}
#[test]
fn lifetime_conditions_and_mixed_type_or_const_generics_stay_unknown() {
    use ResolutionStatus::Unresolved as U;
    for implementation in [
        "impl<'a:'b,'b> T<'a,'b>{fn f(){Self::f();}}",
        "impl<'a,'b> T<'a,'b> where 'a:'b {fn f(){Self::f();}}",
        "impl T<'static,'static>{fn f(){Self::f();}}",
        "impl<'a> T<'a,'a>{fn f(){Self::f();}}",
        "impl<'a,X> T<'a,X>{fn f(){Self::f();}}",
        "impl<'a,const N:usize> T<'a,N>{fn f(){Self::f();}}",
    ] {
        let text = format!("struct T<'a,'b>(&'a str,&'b str);{implementation}fn run(){{T::f();}}");
        let r = inspect(&[("lib.rs", &[], &text)]);
        assert!(r.files[0].complete);
        check(&r, "lib.rs", &text, "T::f", U, None);
        check(&r, "lib.rs", &text, "Self::f", U, None);
    }
}
#[test]
fn lifetime_impl_ambiguity_and_static_specializations_do_not_choose_a_target() {
    for (implementation, expected) in [
        (
            "impl<'a> T<'a>{fn f(){}}impl T<'_>{fn f(){}}",
            ResolutionStatus::Ambiguous,
        ),
        (
            "impl<'a> T<'a>{fn f(){}}impl T<'static>{fn f(){}}",
            ResolutionStatus::Unresolved,
        ),
    ] {
        let text = format!("struct T<'a>(&'a str);{implementation}fn run(){{T::f();}}");
        check(
            &inspect(&[("lib.rs", &[], &text)]),
            "lib.rs",
            &text,
            "T::f",
            expected,
            None,
        );
    }
}
#[test]
fn lifetime_impls_keep_visibility_and_reparse_target_removal() {
    let root = "pub struct T<'a>(&'a str);mod methods;fn run(){T::f();T::private();}";
    let original = "impl<'a> crate::T<'a>{pub fn f(){}fn private(){}}";
    for (source, state) in [
        (original.to_owned(), ResolutionStatus::Resolved),
        (
            original.replace("fn f()", "fn gone()"),
            ResolutionStatus::Unresolved,
        ),
    ] {
        let r = inspect(&[("lib.rs", &[], root), ("methods.rs", &["methods"], &source)]);
        check(&r, "lib.rs", root, "T::f", state, None);
        check(
            &r,
            "lib.rs",
            root,
            "T::private",
            ResolutionStatus::Unresolved,
            None,
        );
    }
}
