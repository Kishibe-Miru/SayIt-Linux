pub mod shortcuts;
pub mod text_input;

pub use shortcuts::{configure_shortcuts, is_native_wayland_session};
pub use text_input::try_commit_text;
