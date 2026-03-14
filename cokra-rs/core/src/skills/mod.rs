//! Skills are the lightweight prompt surface of the kernel.
//!
//! They stay intentionally small:
//! - [`loader`] discovers project/user skill documents
//! - [`injection`] resolves explicit mentions like `$skill` and `@persona`

pub mod injection;
pub mod loader;
