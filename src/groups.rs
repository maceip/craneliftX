//! Stable logical operations shared by the trace runner, process runtime and
//! live/durable event consumers. IDs are independent of display labels.
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineGroup {
    Run,
    Ingest,
    Load,
    Initialize,
    Decode,
    Audit,
    Lower,
    Codegen,
    Artifacts,
    Execute,
    Syscall,
    Cache,
    Result,
}
impl PipelineGroup {
    pub const ALL: [Self; 13] = [
        Self::Run,
        Self::Ingest,
        Self::Load,
        Self::Initialize,
        Self::Decode,
        Self::Audit,
        Self::Lower,
        Self::Codegen,
        Self::Artifacts,
        Self::Execute,
        Self::Syscall,
        Self::Cache,
        Self::Result,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Ingest => "ingest",
            Self::Load => "load",
            Self::Initialize => "initialize",
            Self::Decode => "decode",
            Self::Audit => "audit",
            Self::Lower => "lower",
            Self::Codegen => "codegen",
            Self::Artifacts => "artifacts",
            Self::Execute => "execute",
            Self::Syscall => "syscall",
            Self::Cache => "cache",
            Self::Result => "result",
        }
    }
    pub fn order(self) -> usize {
        Self::ALL.iter().position(|g| *g == self).unwrap() + 1
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Run => "Run setup",
            Self::Ingest => "Read and identify input",
            Self::Load => "Validate and map image",
            Self::Initialize => "Initialize guest state",
            Self::Decode => "Decode x86 instructions",
            Self::Audit => "Inspect instruction policy",
            Self::Lower => "Lower x86 semantics to CLIF",
            Self::Codegen => "Generate RV64 code",
            Self::Artifacts => "Persist code artifacts",
            Self::Execute => "Execute and resume guest",
            Self::Syscall => "Bridge operating-system services",
            Self::Cache => "Manage compiled blocks",
            Self::Result => "Finalize run result",
        }
    }
    pub fn for_stage(stage: &str) -> Self {
        match stage {
            "ingest" => Self::Ingest,
            "load" => Self::Load,
            "initialize" => Self::Initialize,
            "harvest" | "decode" => Self::Decode,
            "audit" => Self::Audit,
            "lower" => Self::Lower,
            "codegen" => Self::Codegen,
            "artifacts" => Self::Artifacts,
            "execute" | "execution" => Self::Execute,
            "syscall" => Self::Syscall,
            "lift" | "cache" => Self::Cache,
            _ => Self::Run,
        }
    }
    pub fn for_event(kind: &str, fields: &Value) -> Self {
        match kind {
            "stage" => Self::for_stage(fields["stage"].as_str().unwrap_or("")),
            "input" | "ingest" => Self::Ingest,
            "harvest" => Self::Decode,
            "artifact" => Self::Artifacts,
            "trace" | "cache_invalidation" | "compilation_progress" => Self::Cache,
            "execution" | "block_execution_totals" | "progress" | "context" => Self::Execute,
            "syscall_started" | "syscall_completed" | "syscall_failed" => Self::Syscall,
            "result" | "error" | "cancelled" => Self::Result,
            "security" => Self::Audit,
            _ => Self::Run,
        }
    }
    pub fn annotate(self, kind: &str, fields: &mut Value) {
        fields["group"] = json!(self.id());
        fields["group_order"] = json!(self.order());
        if kind == "started" {
            fields["groups"] = Self::catalog();
        }
    }
    pub fn catalog() -> Value {
        json!(Self::ALL.map(|g| json!({"id":g.id(),"order":g.order(),"label":g.label()})))
    }
}
