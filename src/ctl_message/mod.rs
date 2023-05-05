pub(crate) mod raw;

#[derive(Debug)]
pub enum ControlError {
    Validation,
    NoFile,
    NoMemory,
    InvalidStatus,
    DecodeFailed,
}
