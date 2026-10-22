pub mod audio;
pub mod client;
pub mod gui;
pub mod protocol;
#[cfg(feature = "transcriber")]
pub mod server;
#[cfg(feature = "transcriber")]
pub mod transcription;
