use super::*;
use crate::domain::FileFormat;
use crate::graph_facts::module_roots::{ModuleMapping, ModuleRoots, PackageLayout};
use crate::graph_facts::{FactInput, inspect_sources};
use zg_knowledge::processing::ProcessingContext;

fn roots(root: &Path, language: Language) -> ModuleRoots {
    std::fs::create_dir_all(root.join("src")).expect("source root");
    ModuleRoots::new(vec![ModuleRoot {
        root: "src".into(),
        language,
        namespace: "fixture".into(),
        module_prefix: vec![],
        layout: PackageLayout::Flat,
    }])
    .expect("roots")
}
fn configured(root: &Path, language: Language) -> ModuleRoots {
    roots(root, language)
        .configured(root)
        .expect("configuration")
}
fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 1 << 20,
        max_output_items: 10000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}
fn facts(root: &ModuleRoots, language: Language, text: &str) -> FileFacts {
    let ModuleMapping::Mapped(module) = root.map("src/a.c", language).expect("mapping") else {
        panic!("module")
    };
    inspect_sources(
        "config",
        &[FactInput {
            path: "src/a.c".into(),
            language,
            bytes: text.as_bytes().to_vec(),
            module: Some(module),
        }],
        &context(),
    )
    .expect("facts")
    .files
    .remove(0)
}
#[test]
fn go_module_snapshot_tracks_absence_creation_and_changes() {
    let dir = tempfile::tempdir().expect("workspace");
    let initial = configured(dir.path(), Language::Go);
    validate_inputs(dir.path(), initial.inputs()).expect("same missing inputs");
    std::fs::write(
        dir.path().join("go.mod"),
        "module example.test/project\n\ngo 1.24\n",
    )
    .expect("go.mod");
    assert!(validate_inputs(dir.path(), initial.inputs()).is_err());
    let current = configured(dir.path(), Language::Go);
    let ModuleMapping::Mapped(module) = current.map("src/pkg/a.go", Language::Go).expect("module")
    else {
        panic!("module")
    };
    assert_eq!(module.path, ["example.test", "project", "src", "pkg"]);
    validate_inputs(dir.path(), current.inputs()).expect("pinned bytes");
    std::fs::write(dir.path().join("go.mod"), "module changed.test/project\n").expect("new config");
    assert!(validate_inputs(dir.path(), current.inputs()).is_err());
}
#[test]
fn invalid_go_module_and_configuration_escape_fail_explicitly() {
    let dir = tempfile::tempdir().expect("workspace");
    let roots = roots(dir.path(), Language::Go);
    for input in [
        "module one\nmodule two\n",
        "go 1.24\n",
        "module ../escape\n",
    ] {
        std::fs::write(dir.path().join("go.mod"), input).expect("configuration");
        assert!(roots.clone().configured(dir.path()).is_err());
    }
    assert!(
        validate_inputs(
            dir.path(),
            &[ConfigurationInput {
                path: "../outside".into(),
                digest: None
            }]
        )
        .is_err()
    );
}
#[test]
fn headers_use_explicit_root_language_and_case_sensitive_extensions() {
    let dir = tempfile::tempdir().expect("workspace");
    let c = configured(dir.path(), Language::C);
    assert_eq!(
        c.language("src/header.h", &[FileFormat::C, FileFormat::Cpp]),
        Some(Language::C)
    );
    assert_eq!(
        c.language("src/upper.C", &[FileFormat::Cpp]),
        Some(Language::Cpp)
    );
    assert_eq!(
        c.map("src/upper.C", Language::Cpp).expect("mapping"),
        ModuleMapping::LanguageMismatch
    );
    let cpp = configured(dir.path(), Language::Cpp);
    assert!(matches!(
        cpp.map("src/upper.C", Language::Cpp).expect("mapping"),
        ModuleMapping::Mapped(_)
    ));
}
#[test]
fn compile_arguments_pin_include_search_and_macros_without_execution() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(dir.path().join("include")).expect("headers");
    std::fs::write(dir.path().join("include/helper.h"), "void helper(void);\n").expect("header");
    std::fs::write(dir.path().join("compile_commands.json"),serde_json::to_vec(&serde_json::json!([{"directory":dir.path(),"file":"src/a.c","arguments":["cc","-Iinclude","-DMACRO=helper","-x","c","-c","src/a.c"]}])).expect("json")).expect("commands");
    let root = configured(dir.path(), Language::C);
    let mut facts = facts(
        &root,
        Language::C,
        "#include <helper.h>\nvoid run(){MACRO();}\n",
    );
    root.apply_configuration(&mut facts)
        .expect("configure facts");
    assert!(facts.references.iter().any(|r|matches!(&r.lookup,Lookup::Include{target:Some(target),..} if target=="include/helper.h")));
    assert!(
        facts
            .bindings
            .iter()
            .any(|b| b.name == "MACRO" && b.target == BindingTarget::Dynamic)
    );
    assert!(root.matches_configuration(&facts).expect("fingerprint"));
    assert!(!dir.path().join("executed").exists());
}
#[test]
fn quoted_include_does_not_fall_through_an_unindexed_higher_priority_file() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(dir.path().join("src")).expect("source");
    std::fs::create_dir(dir.path().join("include")).expect("headers");
    std::fs::write(dir.path().join("src/helper.h"), "// higher priority\n").expect("local header");
    std::fs::write(dir.path().join("include/helper.h"), "void helper(){}\n")
        .expect("lower priority");
    std::fs::write(dir.path().join("compile_commands.json"),serde_json::to_vec(&serde_json::json!([{"directory":dir.path(),"file":"src/a.c","arguments":["cc","-Iinclude","src/a.c"]}])).expect("json")).expect("commands");
    let root = configured(dir.path(), Language::C);
    let mut facts = facts(&root, Language::C, "#include \"helper.h\"\n");
    root.apply_configuration(&mut facts).expect("pin include");
    assert!(facts.references.iter().any(
        |r| matches!(&r.lookup,Lookup::Include{target:Some(target),..} if target=="src/helper.h")
    ));
}
#[test]
fn conflicting_commands_and_unsupported_preprocessing_mark_facts_incomplete() {
    let dir = tempfile::tempdir().expect("workspace");
    for arguments in [
        vec![vec!["cc", "-DCHOSEN=1"], vec!["cc", "-DCHOSEN=2"]],
        vec![vec!["cc", "-include", "generated.h"]],
        vec![vec!["cc", "-x", "c++"]],
    ] {
        let values=arguments.into_iter().map(|arguments|serde_json::json!({"directory":dir.path(),"file":"src/a.c","arguments":arguments})).collect::<Vec<_>>();
        std::fs::write(
            dir.path().join("compile_commands.json"),
            serde_json::to_vec(&values).expect("json"),
        )
        .expect("commands");
        let root = configured(dir.path(), Language::C);
        let mut facts = facts(&root, Language::C, "void run(){}\n");
        root.apply_configuration(&mut facts).expect("diagnostic");
        assert!(!facts.complete);
        assert!(!facts.diagnostics.is_empty());
    }
}
#[test]
fn compiler_command_words_preserve_quotes_and_reject_shell_expansion() {
    assert_eq!(
        words("cc -I'include path' -DNAME=\\\"value\\\" -c a.c").expect("words"),
        ["cc", "-Iinclude path", "-DNAME=\"value\"", "-c", "a.c"]
    );
    for value in [
        "cc $(touch executed)",
        "cc -I\"$HOME/include\"",
        "cc a.c; touch executed",
        "cc 'unterminated",
    ] {
        assert!(words(value).is_err());
    }
}
#[test]
fn include_flag_forms_and_search_groups_keep_compiler_order() {
    let dir = tempfile::tempdir().expect("workspace");
    for name in ["late", "system", "quote"] {
        std::fs::create_dir(dir.path().join(name)).expect("include directory");
        std::fs::write(dir.path().join(name).join("chosen.h"), "// header\n").expect("header");
    }
    std::fs::write(dir.path().join("compile_commands.json"),serde_json::to_vec(&serde_json::json!([{"directory":dir.path(),"file":"src/a.c","arguments":["cc","-idirafterlate","-isystemsystem","-iquotequote","src/a.c"]}])).expect("json")).expect("commands");
    let root = configured(dir.path(), Language::C);
    let mut file = facts(
        &root,
        Language::C,
        "#include <chosen.h>\n#include \"chosen.h\"\n",
    );
    root.apply_configuration(&mut file).expect("pin targets");
    let targets = file
        .references
        .iter()
        .filter_map(|r| match &r.lookup {
            Lookup::Include { target, .. } => target.as_deref(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(targets, ["system/chosen.h", "quote/chosen.h"]);
    let mut other = facts(&root, Language::C, "#include <chosen.h>\n");
    other.version.path = "src/unlisted.c".into();
    root.apply_configuration(&mut other)
        .expect("separate compilation unit");
    assert!(other.references.iter().all(|r| !matches!(
        &r.lookup,
        Lookup::Include {
            target: Some(_),
            ..
        }
    )));
}
