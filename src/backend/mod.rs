pub mod audio;
pub mod convert;
pub mod data;
pub mod device;
pub mod transcode;

use crate::{
    error::Error,
    model::{
        disc::{DiscGraph, Session},
        plan::{BurnPlan, BurnStep},
    },
};

pub fn execute(graph: &DiscGraph, plan: &BurnPlan, dev: &str, debug: bool, progress_json: bool) -> Result<(), Error> {
    device::check_device(dev)?;

    // Pre-flight disc state check
    match device::query_disc_state(dev) {
        Ok(device::DiscState::Finalized) => {
            return Err(Error::device(format!(
                "Disc on {} is already finalized. Insert a blank disc or use `rustydisc recover --blank fast` for CD-RW.",
                dev
            )));
        }
        Ok(device::DiscState::OpenSession) => {
            return Err(Error::device(format!(
                "Disc on {} has an interrupted burn (open session). \
                 Run `rustydisc recover --device {}` to attempt repair before burning.",
                dev, dev
            )));
        }
        Ok(_) | Err(_) => {} // blank, appendable, unknown: let the backend decide
    }

    // CD-RW cannot handle multisession appends (session_index > 0 = Blue Book data session
    // appended after an audio session). Single-session DataCD burns (session_index == 0)
    // write to a blank disc and are fine on CD-RW.
    match device::detect_media_type(dev) {
        Ok(device::DiscMediaType::CdRw) => {
            let needs_multisession = plan.steps.iter().any(|s| {
                matches!(s, BurnStep::AppendDataSession { session_index, .. } if *session_index > 0)
            });
            if needs_multisession {
                return Err(Error::device(
                    "CD-RW does not support multisession appends required for Blue Book (CD Extra). \
                     Use a CD-R for Blue Book, or blank the disc and burn a single-session DataCD.",
                ));
            }
        }
        Ok(_) | Err(_) => {} // CD-R or unknown: proceed
    }

    // Warn if drive has no buffer underrun protection
    if let Ok(false) = device::has_buffer_underrun_protection(dev) {
        eprintln!(
            "Warning: drive does not report buffer underrun protection (BURN-Proof/SMART-BURN). \
             Ensure no background tasks compete for CPU/IO during burn."
        );
    }

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
                        // Convert any non-CDDA tracks before burning
                        let prepared = audio::prepare_tracks(a, debug)?;
                        audio::write_audio_session(&prepared, dev, !finalize, debug, progress_json)?;
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
                        data::append_data_session(d, dev, msinfo.as_deref(), &graph.label, debug, progress_json)?;
                    }
                    _ => return Err(Error::backend("Expected data session")),
                }
            }
            BurnStep::FinalizeDisc => {
                // xorriso cdrecord (without -multi) finalizes and ejects the disc
                // automatically. Only attempt an explicit finalize if the disc is still
                // in appendable state — in all other cases (already finalized, ejected,
                // no disc, unknown state, or drive error) skip silently.
                match device::query_disc_state(dev) {
                    Ok(device::DiscState::Appendable { .. }) => {
                        device::finalize_disc(dev, debug)?;
                    }
                    _ => {
                        if debug {
                            eprintln!("FinalizeDisc: disc not in appendable state — skipping (likely already closed by write backend).");
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
