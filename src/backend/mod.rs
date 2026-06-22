pub mod audio;
pub mod data;
pub mod device;

use crate::{
    error::Error,
    model::{
        disc::{DiscGraph, Session},
        plan::{BurnPlan, BurnStep},
    },
};

pub fn execute(graph: &DiscGraph, plan: &BurnPlan, dev: &str, debug: bool) -> Result<(), Error> {
    device::check_device(dev)?;

    for step in &plan.steps {
        match step {
            BurnStep::BurnAudioSession {
                session_index,
                finalize,
            } => {
                let session = graph.sessions.get(*session_index).ok_or_else(|| {
                    Error::backend(format!("Session index {} out of range", session_index))
                })?;
                match session {
                    Session::Audio(a) => {
                        audio::write_audio_session(a, dev, !finalize, debug)?;
                    }
                    _ => return Err(Error::backend("Expected audio session")),
                }
            }
            BurnStep::AppendDataSession {
                session_index,
                filesystem: _,
            } => {
                let session = graph.sessions.get(*session_index).ok_or_else(|| {
                    Error::backend(format!("Session index {} out of range", session_index))
                })?;
                match session {
                    Session::Data(d) => {
                        let msinfo = if *session_index > 0 {
                            Some(device::get_msinfo(dev)?)
                        } else {
                            None
                        };
                        data::append_data_session(d, dev, msinfo.as_deref(), debug)?;
                    }
                    _ => return Err(Error::backend("Expected data session")),
                }
            }
            BurnStep::FinalizeDisc => {
                device::finalize_disc(dev, debug)?;
            }
        }
    }

    Ok(())
}
