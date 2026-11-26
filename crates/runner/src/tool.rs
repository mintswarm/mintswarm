//! The ported dogfood tools behind `mintswarm-runner tool <name>`, each with the command line of
//! the Python script it replaces: [`save_output`], the port of `save-output.py`, and [`msw_vcs`],
//! the port of the `msw-vcs` shim.

pub mod msw_vcs;
pub mod save_output;
