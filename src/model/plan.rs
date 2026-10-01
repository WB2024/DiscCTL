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
    /// Author a DVD-Video disc from an audio session (plus an optional data session) and write it.
    BurnMusicDvd {
        audio_session_index: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        data_session_index: Option<usize>,
    },
    FinalizeDisc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurnPlan {
    pub format: String,
    pub label: String,
    /// Write speed as an "x" multiple; absent means the drive chooses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<u32>,
    pub steps: Vec<BurnStep>,
}
