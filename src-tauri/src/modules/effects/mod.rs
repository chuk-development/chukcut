//! Effect runtime.
//!
//! Ports the existing reverse-engineered engine from `~/git/x/capcut-renderer`:
//! a Lua VM driving multi-pass GLSL-ES shader graphs. Effect packages are
//! fetched at runtime from a URL the user supplies — nothing ships in the
//! bundle. Last phase; the compositor works without it.
