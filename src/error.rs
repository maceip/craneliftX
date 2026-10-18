use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecurityViolation {
    InvalidBounds,
    PrivilegedInstruction { rip: u64 },
    StackPivot { rip: u64 },
    StackOutOfBounds { rip: u64 },
    AbsoluteAddress { rip: u64, address: u64 },
}

/// Which heap/pointer mitigation rejected an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MitigationKind {
    /// Address tag disagreed with the shadow tag of a granule it touches.
    TagMismatch,
    /// Access landed on a granule that a freed chunk owns.
    UseAfterFree,
    /// Pointer authentication code missing, wrong, or bound elsewhere.
    PointerAuth,
    /// Indirect branch through a value that was never authenticated.
    UnauthenticatedCall,
}
impl MitigationKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::TagMismatch => "heap_tag_mismatch",
            Self::UseAfterFree => "heap_use_after_free",
            Self::PointerAuth => "pointer_auth_failure",
            Self::UnauthenticatedCall => "unauthenticated_indirect_call",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    Ingest(String),
    Process(String),
    Harvest {
        rip: u64,
        reason: &'static str,
    },
    Security(SecurityViolation),
    Lower {
        rip: u64,
        reason: &'static str,
    },
    Codegen(String),
    Execute {
        pc: u64,
        reason: &'static str,
    },
    /// A heap-tagging or pointer-authentication check rejected an access.
    Mitigation {
        pc: u64,
        kind: MitigationKind,
        detail: String,
    },
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
