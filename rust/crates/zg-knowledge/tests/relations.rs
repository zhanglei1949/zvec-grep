use tokio_util::sync::CancellationToken;
use zg_knowledge::{
    processing::ProcessingContext,
    relations::{FileFacts, Language, SourceVersion, Span, extract_markdown, resolve_documents},
};
fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 65536,
        max_output_items: 1000,
        deadline: None,
        cancellation: CancellationToken::new(),
    }
}
fn version(path: &str, text: &str) -> SourceVersion {
    let digest = zg_storage::sha256_hex(text.as_bytes());
    SourceVersion {
        generation: "g1".into(),
        path: path.into(),
        raw_digest: digest.clone(),
        decoded_digest: digest,
    }
}
#[test]
fn rejects_invalid_source_digests_paths_and_utf8_spans() {
    let text = "中文";
    assert!(Span { start: 1, end: 3 }.validate(text).is_err());
    assert!(Span { start: 0, end: 7 }.validate(text).is_err());
    assert!(Span { start: 4, end: 3 }.validate(text).is_err());
    for path in [
        "/a.md",
        "../a.md",
        "a/../b.md",
        "a//b.md",
        "a\\b.md",
        "C:a.md",
    ] {
        assert!(FileFacts::new(version(path, text), Language::Markdown, None, text).is_err());
    }
    let mut v = version("a.md", text);
    v.decoded_digest = "incorrect".into();
    assert!(FileFacts::new(v, Language::Markdown, None, text).is_err());
}
#[test]
fn rejects_unknown_scopes_and_parents_and_duplicate_references() {
    let text = "# Title\n[x](#title)\n";
    let facts = extract_markdown(version("a.md", text), text, &context()).expect("facts");
    let mut f = facts.clone();
    f.scopes[0].parent = Some(0);
    assert!(f.validate(text, &context()).is_err());
    let mut f = facts.clone();
    f.nodes[1].parent = Some("missing".into());
    assert!(f.validate(text, &context()).is_err());
    let mut f = facts.clone();
    f.nodes[1].parent = Some(f.nodes[1].id.clone());
    assert!(
        f.validate(text, &context()).is_err(),
        "a node cannot parent itself"
    );
    let mut f = facts.clone();
    f.references.push(f.references[0].clone());
    assert!(f.validate(text, &context()).is_err());
    let mut f = facts;
    f.references[0].scope = 999;
    assert!(f.validate(text, &context()).is_err());
}
#[test]
fn markdown_percent_decoding_heading_formatting_and_reference_links() {
    let a = "# 开始\n[x](b.md#hello-world) [y](b.md#%E4%B8%AD%E6%96%87)\n";
    let b = "# Hello *World*\n# 中文\n";
    let files = vec![
        extract_markdown(version("a.md", a), a, &context()).expect("a"),
        extract_markdown(version("b.md", b), b, &context()).expect("b"),
    ];
    let resolved =
        resolve_documents(&files, &["a.md".into(), "b.md".into()], &context()).expect("resolve");
    assert_eq!(resolved.len(), 2);
    assert!(resolved.iter().all(|r| r.candidates.len() == 1));
}
#[test]
fn markdown_ranges_map_sections_and_cancel_or_budget_return_error() {
    let text = "# One\nintro\n## Two\n[x](#one)\n# Three\n";
    let facts = extract_markdown(version("a.md", text), text, &context()).expect("parse");
    assert_eq!(facts.nodes[2].parent.as_ref(), Some(&facts.nodes[1].id));
    assert_eq!(facts.references[0].owner, facts.nodes[2].id);
    let mut c = context();
    c.max_output_items = 2;
    assert!(extract_markdown(version("a.md", text), text, &c).is_err());
    c = context();
    c.cancellation.cancel();
    assert!(extract_markdown(version("a.md", text), text, &c).is_err());
}

#[test]
fn commonmark_sections_tables_fences_and_unicode_keep_decoded_ranges() {
    let text = "前言\r\n# Parent\r\nintro\r\n## 子章节\r\n[x](#parent)\r\n```md\r\n# fake\r\n[bad](missing.md)\r\n```\r\n\r\n| a | b |\r\n|---|---|\r\n| x | y |\r\n\r\nNext\r\n===\r\nend\r\n";
    let facts = extract_markdown(version("a.md", text), text, &context()).expect("CommonMark");
    assert_eq!(facts.nodes.len(), 4, "root and three actual headings");
    assert_eq!(facts.nodes[1].name, "Parent");
    assert_eq!(facts.nodes[2].name, "子章节");
    assert_eq!(facts.nodes[3].name, "Next");
    assert_eq!(facts.nodes[2].parent.as_ref(), Some(&facts.nodes[1].id));
    assert_eq!(
        facts.nodes[1].span.start,
        text.find("# Parent").expect("parent")
    );
    assert_eq!(facts.nodes[1].span.end, facts.nodes[3].span.start);
    assert_eq!(
        facts.references.len(),
        1,
        "fenced links do not become facts"
    );
    let link = &facts.references[0];
    assert_eq!(&text[link.span.start..link.span.end], "[x](#parent)");
    assert_eq!(link.owner, facts.nodes[2].id);
    assert!(facts.nodes.iter().all(|n| n.span.validate(text).is_ok()));
    facts
        .validate(text, &context())
        .expect("valid UTF-8 mapping");
}
