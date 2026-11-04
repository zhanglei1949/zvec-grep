# Local grammar revision 0.23.4-zg.1

Based on [tree-sitter-cpp 0.23.4](https://github.com/tree-sitter/tree-sitter-cpp/tree/v0.23.4) (MIT). `grammar.patch` changes only `optional_parameter_declaration.default_value` from `expression` to a choice of `expression` or `initializer_list`, allowing standard braced defaults such as `Options value = {}`.

`src/parser.c`, `src/node-types.json` and the Tree-sitter headers are generated with the official tree-sitter CLI 0.25.10:

```sh
tree-sitter generate src/grammar.json
```

Cargo compiles the checked-in parser; no generator is required during builds. The host grammar fingerprint includes this revision. Original source bytes, error detection and the shared retrieval/graph AST are unchanged.
