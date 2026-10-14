//! Drawing, erasing, shapes, text, and ordering, shared by the host and the
//! simple-sketch sub-application. The host runs them on its live copy. The
//! sub-application commits arrangement, saving, and undo.

pub mod defaults;
pub mod document;
pub mod ink;
pub mod prefs;
pub mod registry;
pub mod schema;
pub mod session;
pub mod store;
pub mod text;

pub use session::Session;
