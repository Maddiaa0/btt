//! btt — branch tree testing, for any language.
//!
//! A generalization of [bulloak](https://bulloak.dev): `.tree` files specify
//! test suites as given/when/it branching trees; language packs (data-only:
//! a tree-sitter query, naming rules, and a scaffold template) teach the core
//! how those trees map onto each language's test conventions.

pub mod check;
pub mod config;
pub mod extract;
pub mod mapping;
pub mod pack;
pub mod runner;
pub mod scaffold;
pub mod tree;
