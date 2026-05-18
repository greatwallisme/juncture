---
name: rust-guidelines
description: ALWAYS invoke this skill BEFORE writing or modifying ANY Rust code (.rs files) even for simple Hello World programs. Enforces Microsoft-style Rust development discipline, zero-tolerance quality standards (zero errors, zero warnings), and requires consulting the appropriate guideline files before any coding activity. This skill is MANDATORY for all Rust development.
---

# Rust Development Skill
This skill enforces structured, guideline-driven Rust development with zero-tolerance quality standards. It ensures all Rust code strictly follows the appropriate Microsoft-style rules, documentation formats, and quality constraints.


## Mandatory Workflow

**This skill MUST be invoked for ANY Rust action**, including:
- Creating new `.rs` files (even minimal examples like Hello World)
- Modifying existing `.rs` files (any change, no matter how small)
- Reviewing, refactoring, or rewriting code Rust code


## Which guideline to read and when

Before writing or modifying Rust code, **Claude must load ONLY the guideline files that apply to the requested task**, using segmented reading (`offset` and `limit`) when needed.

### Guidelines and when they apply

#### 0. `00_quality_gates.md`
**Read FIRST for ALL Rust tasks.** This file defines:
- Zero Tolerance Policy: ALL code must pass with zero errors and zero warnings
- NEVER rules: prohibited practices (unwrap, global allow, Cargo.toml changes)
- ALWAYS rules: required standards (error handling, iterators, documentation)
- Required lint configuration for Cargo.toml
- Verification commands to run before claiming code is complete

#### 1. `01_ai_guidelines.md`
Use when the Rust code involves:
- AI agents and LLM-driven code generation
- Making APIs easier for AI systems to use
- Comprehensive documentation and detailed examples
- Strong type systems that help AI avoid mistakes

#### 2. `02_application_guidelines.md`
Use when working on:
- Application-level error handling with anyhow or eyre
- CLI tools and desktop applications
- Performance optimization using mimalloc allocator
- User-facing features and initialization logic

#### 3. `03_documentation_guidelines.md`
Use when:
- Writing public API documentation and doc comments
- Creating canonical documentation sections (Examples, Errors, Panics, Safety)
- Structuring module-level documentation comprehensively
- Re-exporting items and using #[doc(inline)] annotations

#### 4. `04_ffi_guidelines.md`
Use when:
- Loading multiple Rust-based dynamic libraries (DLLs)
- Creating FFI boundaries and interoperability layers
- Sharing data between different Rust compilation artifacts
- Dealing with portable vs non-portable data types across DLL boundaries

#### 5. `05_performance_guidelines.md`
Use when:
- Identifying and profiling hot paths in your code
- Optimizing for throughput and CPU cycle efficiency
- Managing allocation patterns and memory usage
- Implementing yield points in long-running async tasks

#### 6. `06_safety_guidelines.md`
Use when:
- Writing unsafe code for novel abstractions, performance, or FFI
- Ensuring code soundness and preventing undefined behavior
- Documenting safety requirements and invariants
- Reviewing unsafe blocks for correctness with Miri

#### 7. `07_universal_guidelines.md`
Use in **ALL Rust tasks**. This file defines:
- General Rust best practices applicable to all code
- Style, naming, and organizational conventions
- Cross-cutting concerns that apply everywhere
- Foundational principles for any Rust project

#### 8. `08_libraries_building_guidelines.md`
Use when:
- Creating reusable library crates
- Managing Cargo features and their additivity
- Building native `-sys` crates for C interoperability
- Ensuring libraries work out-of-the-box on all platforms

#### 9. `09_libraries_interoperability_guidelines.md`
Use when:
- Exposing public APIs and managing external dependencies
- Designing types for Send/Sync compatibility
- Avoiding leaking third-party types from public APIs
- Creating escape hatches for native handle interop

#### 10. `10_libraries_resilience_guidelines.md`
Use when:
- Avoiding statics and thread-local state in libraries
- Making I/O and system calls mockable for testing
- Preventing glob re-exports and accidental leaks
- Feature-gating test utilities and mocking functionality

#### 11. `11_libraries_ux_guidelines.md`
Use when:
- Designing user-friendly library APIs
- Managing error types and error handling patterns
- Creating runtime abstractions and trait-based designs
- Structuring crate organization and public interfaces


## Coding Rules

1. **Load the necessary guideline files BEFORE ANY RUST CODE GENERATION.**
2. **Always read `00_quality_gates.md` FIRST** for ALL Rust tasks.
3. Apply the required rules from the relevant guidelines.
4. Apply the *M-CANONICAL-DOCS* documentation format.
5. For common anti-patterns, reference `references/anti_patterns.md` for BEFORE/AFTER examples.
6. Comments must ALWAYS be written in American English, unless the user explicitly requests ‘write comments in French’ or provides another clear instruction specifying a different comment language.
7. If the file is fully compliant, add a comment: `// Rust guideline compliant {date}` where {date} is the guideline date or the date of the current day (format YYYY-MM-DD)


---

