use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BurnStep {
    BurnAudioSession {
        session_index: usize,
        finalize: bool,
    },
    AppendDataSession {
        session_index: usize,
        filesystem: String,
    },
    FinalizeDisc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurnPlan {
    pub format: String,
    pub label: String,
    pub steps: Vec<BurnStep>,
}
