---
paths:
  - "**/*.rs"
---

# Constraints

- All Rust code must pass with zero warnings and zero errors (clippy pedantic/nursery/cargo/restriction)
- Never use `unwrap()`, `todo!()`, `unimplemented!()` in committed code
- Never write placeholder/mock code
- Never use file-level `#![allow(...)]` or `#![expect(...)]` 
- always apply `#[allow(...)]` or `#[expect(...)]` at the item level (function, struct, field, impl block) with a reason

#  Tools
- Use rust-analyzer-lsp for code analysis, navigation, refactoring, real-time diagnostics, auto-completion, and workspace management.