//! Types and rules shared by the Musik Faktory server and the Dioxus UI.
//!
//! Everything that both sides must agree on lives here: the generation
//! parameters, the composition-plan model, the API request/response bodies,
//! validation rules and the cost estimate.

pub mod api;
pub mod plan;
pub mod rules;

pub use api::*;
pub use plan::*;
pub use rules::*;
