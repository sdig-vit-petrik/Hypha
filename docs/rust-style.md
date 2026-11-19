# Rust style

How hypha's Rust code is written. Read it before writing or reviewing Rust in
`crates/` or `apps/`. C++ follows its own rules (`docs/cpp-api.md`).

## Sources, in order of precedence

When two sources disagree, the earlier one wins.

1. The hypha rules in `AGENTS.md` (architecture, unsafe policy, fail-explicit).
2. This file.
3. Clippy and rustc as configured: `[workspace.lints]` in `Cargo.toml`, the
   `[lints]` tables of `hypha_ffi`, `hypha_node`, `hypha_testkit` and
   `hypha_tools`, and `clippy.toml`.
4. The [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/).
5. The [Safety-Critical Rust Coding Guidelines](https://coding-guidelines.arewesafetycriticalyet.org/)
   (SCRC). hypha's rules are loosely based on SCRC: we take the guidelines we
   want and do not claim compliance. The table below says which.

Effective Rust and Rust Design Patterns are a reading list, not a rule source.

Code is idiomatic Rust: iterators, combinators, `?`, `let-else` and closures
where they read naturally. Global "embedded C" or lambda-restriction style rules
do not apply to Rust in this repository.

## House rules

These are not visible from the lint configuration alone.

- **Errors are typed enums, not strings.** A fallible function returns
  `Result<T, E>` with an `E` that names each failure as a variant carrying the
  data a caller needs. A `String` or `Box<dyn Error>` error is for a tool's
  `main` at most. A message is formatted where the error is displayed.
- **No `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!` or
  `unimplemented!` in shipped code.** Clippy denies them. A test may suppress
  `unwrap_used` / `expect_used` for itself (with a reason, see below); a test
  does not need a typed error.
- **Fallible conversions use `TryFrom` and report the failure.** A length, an
  index or a C enum value from outside is converted with `try_from`; the error
  becomes a status or an error variant, never a silent truncation or a default.
  A saturating conversion is allowed only where saturation is the documented
  meaning, with a comment that says why.
- **No raw arithmetic on integers.** `arithmetic_side_effects` is denied: use
  `checked_*` and report the overflow, or `saturating_*` / `wrapping_*` where
  that is the meaning. `clippy.toml` explains the operator cases.
- **No indexing that can panic.** `indexing_slicing` is denied: use `get`,
  `split_at_checked`, `chunks_exact` or an iterator.
- **Every lint suppression is `#[expect(lint, reason = "...")]`.** The reason
  states the fact that makes the code correct, not "needed" or "clippy". An
  `expect` goes stale loudly when the lint stops firing. An `allow` stays only
  where the lint fires on some `cfg` and not others, and then it carries a
  `reason` too.
- **Every `unsafe` block has a `// SAFETY:` comment** naming the invariant it
  relies on: the clause of the function's `# Safety` section, a null checked
  above, a length checked on a named line. Generic text ("pointer is valid") is
  not a reason. One unsafe operation per block. Every `unsafe fn` and every
  `extern "C"` function that takes a pointer has a `# Safety` section.
- **`unsafe` lives in few places:** only in `hypha_ffi` (the C boundary), `hypha_sys`
  (the operating-system calls), the test-only module `hypha_testkit/src/env.rs`,
  whose `set_env` / `remove_env` wrap the edition-2024 `unsafe` `std::env::set_var` /
  `remove_var` for every test, and the C boundary module
  `hypha_testing_mcap/src/ffi.rs` of the test-only MCAP reader. The lint levels hold
  this: the workspace forbids `unsafe_code`; `hypha_ffi` and `hypha_sys` allow it;
  `hypha_testkit` denies it and `src/env.rs` allows it; `hypha_testing_mcap` denies it
  and `src/ffi.rs` allows it; `hypha_node` and `hypha_tools` forbid it, and
  `hypha_tools` reaches its two OS calls through `hypha_node::sdk::sys`.
- **Public items are documented and reachable.** `missing_docs` and
  `unreachable_pub` are denied: an item is `pub` only if a caller outside the
  crate uses it; otherwise `pub(crate)` or private.
- **Test code obeys the same lints** as shipped code, except that an
  integration-test file which calls the C ABI may carry one crate-level
  `#![expect(...)]` naming only the unsafe and cast lints that fire in it.

## Lints and rules mapped to SCRC

SCRC version 0.1 ("Safety-Critical Rust Coding Guidelines 0.1 documentation"),
read on 2026-11-15. Ids and titles are SCRC's; the text is not copied here
(CC BY 4.0). On that date the Unsafety, FFI, Exceptions And Errors and
Concurrency chapters held no guidelines, so rules in those areas have no SCRC
id. "planned (RSn)" is a phase 7 card that will turn the rule on; it becomes
`enforced` when that card lands.

| hypha lint or rule | SCRC id and title | status |
| ------------------ | ----------------- | ------ |
| `clippy::arithmetic_side_effects` = deny | gui_dCquvqE1csI3 Ensure that integer operations do not result in arithmetic overflow | enforced |
| `clippy::arithmetic_side_effects` = deny | gui_kMbiWbn8Z6g5 Do not divide by 0 | enforced |
| Division by a nonzero constant or through `checked_div` / `checked_rem` | gui_7y0GAMmtMhch Do not use an integer type as a divisor during integer division | partial: integer division is used; the divisor is a nonzero constant or the division is `checked_*`, so a zero divisor is an error, not a panic |
| `clippy::arithmetic_side_effects` = deny (covers `<<`, `>>`) | gui_RHvQj8BHlz9b Do not shift an expression by a negative number of bits or by greater than or equal to the bitwidth of the operand; gui_LvmzGKdsAgI5 Avoid out-or-range shifts | enforced |
| `clippy::as_conversions` = deny | gui_ADHABsmK9FXz The 'as' operator should not be used with numeric operands | planned (RS4) |
| `clippy::as_underscore`, `clippy::ptr_as_ptr` = deny | gui_HDnAZ7EZ4z6G Avoid as underscore pointer casts | planned (RS4) |
| `clippy::as_conversions`, `clippy::fn_to_numeric_cast_any` = deny | gui_PM8Vpf7lZ51U An integer shall not be converted to a pointer; gui_iv9yCMHRgpE0 An integer shall not be converted to an invalid pointer | planned (RS4) |
| Fallible conversions use `TryFrom` (house rule) | gui_ADHABsmK9FXz (as above) | partial: rule written, `as` still allowed until RS4 |
| Typed error enums; newtypes for times (`Time`, `Duration`) | gui_xztNdXA2oFNC Use strong types to differentiate between logically distinct values | partial: applied to public types, not to every internal integer |
| No `union` in the code | gui_0cuTYG8RVYjg Ensure reads of union fields produce valid values for the field's type | enforced by absence; no lint guards it |
| Recursion allowed with a depth limit | gui_ot2Zt3dd6of1 Recursive function are not allowed | not adopted: the message-definition resolver recurses over nested types and is bounded by its depth and size limits |
| `#[unsafe(no_mangle)]`, `unsafe extern` (edition 2024) | gui_ZDLZzjeOwLSU Assure visibility of `unsafe` keyword in unsafe code | enforced |
| No macro expands to an `unsafe` block | gui_FRLaMIMb4t3S Do not hide unsafe blocks within macro expansions | enforced by review; no lint guards it |
| Small `macro_rules!` only where a function cannot do it | gui_2jjWUoF1teOY A macro should not be used in place of a function | partial: by review |
| Macros are used (std formatting, derives, `clap`) | gui_a1mHfjgKk4Xr Shall not invoke macros; gui_WJlWqgIxmE8P Shall not use Function-like Macros; gui_h0uG1C9ZjryA Shall not use Declarative Macros; gui_66FSqzD55VRZ Procedural macros should not be used; gui_13XWp3mb0g2P Attribute macros shall not be used; gui_uuDOArzyO3Qw Shall not write code that expands macros | not adopted: `format!`, `write!`, derives and `clap` are idiomatic and checked by the compiler |
| The few `macro_rules!` use `$crate`/std paths and local names only | gui_SJMrWDYZ0dN4 Names in a macro definition shall use a fully qualified path; gui_8hs33nyp0ipX Shall ensure complete hygiene of macros | partial: by review |
| `unsafe_code` = forbid, except `hypha_ffi`, `hypha_sys`, `hypha_testkit/src/env.rs` and `hypha_testing_mcap/src/ffi.rs` | none (Unsafety chapter empty) | enforced |
| `unsafe` only in `hypha_ffi`, `hypha_sys`, the test-only `hypha_testkit/src/env.rs` and the test-only `hypha_testing_mcap/src/ffi.rs` | none (Unsafety chapter empty) | enforced |
| `clippy::missing_safety_doc` = deny (`hypha_ffi`, `hypha_node`, `hypha_tools`) | none (Unsafety chapter empty) | enforced |
| `clippy::undocumented_unsafe_blocks`, `clippy::multiple_unsafe_ops_per_block` = deny | none (Unsafety chapter empty) | planned (RS3) |
| `clippy::non_send_fields_in_send_ty` = deny | none (Concurrency chapter empty) | enforced |
| `clippy::unwrap_used`, `expect_used`, `panic`, `unreachable`, `todo`, `unimplemented`, `exit` = deny | none (Exceptions And Errors chapter empty) | enforced |
| `clippy::indexing_slicing` = deny | none found | enforced |
| `clippy::allow_attributes`, `allow_attributes_without_reason` = deny | none found (Compliance: Deviation Process is the nearest topic) | planned (RS2) |
| Nursery: `use_self`, `derive_partial_eq_without_eq`, `or_fun_call`, `redundant_clone`, `needless_pass_by_ref_mut`, `useless_let_if_seq`, `equatable_if_let`, `trailing_empty_array`, `significant_drop_in_scrutinee`, `significant_drop_tightening` = deny | none found | planned (RS5) |
| `clippy::rc_buffer`, `rc_mutex` = deny | none found | enforced |
| `missing_docs`, `unreachable_pub` = deny | none found | enforced |
| `dead_code` = deny | none found | enforced |
| `clippy::all`, `clippy::pedantic`, `rust_2018_idioms` = warn, and the gate runs `-D warnings` | none found | enforced |
| `needless_pass_by_value`, `module_name_repetitions` = allow | none found | not adopted: pedantic lints whose findings are noise here (owned arguments moved into a builder or thread; crate-prefixed type names) |

## Lints we decided against

So nobody re-adds them without a new reason:

- `clippy::redundant_pub_crate`: contradicts `unreachable_pub`, which we keep.
- `clippy::missing_const_for_fn`: churn, and every `const fn` becomes a
  promise to keep.
- `clippy::option_if_let_else`: `map_or_else` with two closures usually reads
  worse than the `if let`.
- `clippy::too_long_first_doc_paragraph`: may be taken later in a docs change.

## Formatting

Every crate is edition 2024 (`[workspace.package]` in `Cargo.toml`).
`rustfmt.toml`: stock rustfmt with `max_width = 100` (the C++ tree's column
limit) and `use_small_heuristics = "Max"`. `cargo fmt --all -- --check` is part
of the gate.
