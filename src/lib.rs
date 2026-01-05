pub mod config;
pub mod log;
pub mod peer;
pub mod poisson;

pub const CONFIG_FILE_PATH: &str = "config.txt";
pub const WORDS_FILE_PATH: &str = "brainrot.txt";
pub const RATE: f32 = 1.;
pub const MALICIOUS_RATE: f32 = 0.6;
pub const MAX_DIFF: u64 = 20;
