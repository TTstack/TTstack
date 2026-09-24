//! TTstack core library.
//!
//! Provides shared types, engine abstractions, storage backends, and
//! network utilities used by both the host agent and central controller.
//!
//! The [`api`] and [`model`] modules are platform-independent and used
//! by all components (CLI, controller, agent).
//!
//! The [`engine`], [`net`], and [`storage`] modules contain host-specific
//! implementations. Linux is primary; FreeBSD support is experimental.

pub mod api;
pub mod auth;
pub mod model;

pub mod engine;
pub mod net;
pub mod storage;

pub mod command;
