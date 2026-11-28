//! ROS 1 message definitions: parsing, dependency resolution and the MD5 identity rule.
//!
//! # Module map
//! - `parse`: `.msg` and `.srv` lexer/parser
//! - `spec`: parsed field, constant, and message AST types
//! - `constant`: typed constant value parsing and range checks
//! - `resolve`: recursive dependency resolution and canonical full-text assembly
//! - `md5`: ROS 1 MD5 identity hashing for messages and services
//! - `definition`: resolved message/service definition bundles
//! - `size`: fixed-size vs. dynamic encoded byte-size computation

extern crate alloc;

pub mod constant;
pub mod definition;
pub mod md5;
pub mod parse;
pub mod resolve;
pub mod size;
pub mod spec;
