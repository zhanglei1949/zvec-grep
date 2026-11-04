# Local grammar revision 0.24.2-zg.1

Based on tree-sitter-c 0.24.2 (MIT). `grammar.patch` records the complete change to the release's `src/grammar.json`.

- Recognize the paired `#ifdef __cplusplus` / `extern "C" {` / `#endif` header wrapper, including its closing guard, as one syntactic linkage wrapper. Other conditions are not evaluated.
- Recognize opaque identifier annotations before a declaration's normal specifiers. Prefer the original type interpretation when both parses are valid. The host keeps annotated declarations and calls unresolved; it does not expand macros or infer their targets.

`src/parser.c`, `src/node-types.json` and the Tree-sitter headers are generated with the official tree-sitter CLI 0.25.10:

```sh
tree-sitter generate src/grammar.json
```

Generation is a maintainer operation. Cargo compiles the checked-in parser without Node, downloads or compiler-command execution. Retrieval and graph extraction continue to share one AST over the original bytes. Missing/error nodes still make graph facts incomplete. The host grammar fingerprint includes this revision.
