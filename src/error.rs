use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecurityViolation {
    InvalidBounds,
    PrivilegedInstruction { rip: u64 },
    StackPivot { rip: u64 },
    StackOutOfBounds { rip: u64 },
    AbsoluteAddress { rip: u64, address: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    Ingest(String),
    Process(String),
    Harvest { rip: u64, reason: &'static str },
    Security(SecurityViolation),
    Lower { rip: u64, reason: &'static str },
    Codegen(String),
    Execute { pc: u64, reason: &'static str },
}

impl From<SecurityViolation> for EngineError {
    fn from(value: SecurityViolation) -> Self {
        Self::Security(value)
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for EngineError {}

impl fmt::Display for SecurityViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for SecurityViolation {}
