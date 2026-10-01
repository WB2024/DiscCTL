pub mod audio;
pub mod cache;
pub mod convert;
pub mod data;
pub mod device;
pub mod dvd;
pub mod loudness;
pub mod normalize;
pub mod source;
pub mod speed;
pub mod transcode;

use crate::{
    error::Error,
    model::{
        disc::{DiscGraph, Session},
        plan::{BurnPlan, BurnStep},
    },
};

/// DVD and Blu-ray discs: the media is checked with xorriso (CD tools don't understand DVDs)
/// and images are built with genisoimage.
fn execute_dvd(graph: &DiscGraph, plan: &BurnPlan, dev: &str, debug: bool, progress_json: bool) -> Result<(), Error> {
    if !dvd::writing_to_file() {
        device::check_device(dev)?;
    }
    for step in &plan.steps {
        match step {
            BurnStep::AppendDataSession { session_index, .. } => match graph.sessions.get(*session_index) {
                Some(Session::Data(d)) => dvd::burn_data_dvd(d, dev, &graph.label, debug, progress_json)?,
                _ => return Err(Error::backend("Expected a data session")),
            },
            BurnStep::BurnMusicDvd { audio_session_index, data_session_index } => {
                let tracks = match graph.sessions.get(*audio_session_index) {
                    Some(Session::Audio(a)) => a.tracks.clone(),
                    _ => return Err(Error::backend("Expected an audio session")),
                };
                let data_dir = match data_session_index.and_then(|i| graph.sessions.get(i)) {
                    Some(Session::Data(d)) => Some(d.source_dir.clone()),
                    _ => None,
                };
                let opts = graph.dvd.clone().unwrap_or_default();
                dvd::burn_music_dvd(&tracks, data_dir.as_deref(), &opts, dev, &graph.label, debug, progress_json)?;
            }
            BurnStep::FinalizeDisc => {} // the write closes the disc
            BurnStep::BurnAudioSession { .. } => return Err(Error::backend("CD audio sessions can't be burned to a DVD")),
        }
    }
    Ok(())
}

/// Measure the session's tracks and choose a gain for each, telling the user what will change.
fn normalize_gains(tracks: &[String], spec: normalize::Spec, progress_json: bool) -> Result<Vec<f64>, Error> {
    let say = |m: &str| {
        if progress_json {
            println!("{}", serde_json::json!({"type": "step", "msg": m}));
        } else {
            eprintln!("{m}");
        }
    };
    say("Measuring loudness so the tracks can be levelled...");
    let (measured, album) = normalize::measure(tracks)?;
    let gains = normalize::compute(spec, &measured, album);
    say(&normalize::describe(&gains, spec));
    if progress_json {
        println!("{}", serde_json::json!({"type": "normalize", "mode": spec.mode, "target_lufs": spec.target_lufs, "tracks": gains}));
    } else {
        for g in &gains {
            eprintln!("  {:+.1} dB{}  {}", g.gain_db, if g.limited { " (held back)" } else { "" }, g.path);
        }
    }
    Ok(gains.iter().map(|g| g.gain_db).collect())
}

pub fn execute(graph: &DiscGraph, plan: &BurnPlan, dev: &str, debug: bool, progress_json: bool) -> Result<(), Error> {
    // The write speed: refuse one the drive can't do before anything is written, then let the
    // backends read it when they build their commands.
    speed::set(graph.speed);
    if let Some(x) = graph.speed {
        if !(graph.format.is_dvd() && dvd::writing_to_file()) {
            if let Ok(list) = speed::query(dev) {
                speed::check_supported(x, &list)?;
            }
        }
    }
    if graph.format.is_dvd() {
        return execute_dvd(graph, plan, dev, debug, progress_json);
    }
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
                        // Convert the tracks for the disc, then level them if asked: the levelling is
                        // measured on the finished disc audio, after any stream choice or downmix.
                        let mut prepared = audio::prepare_tracks(a, debug)?;
                        if let Some(spec) = graph.normalize {
                            let gains = normalize_gains(&prepared.tracks, spec, progress_json)?;
                            prepared.apply_gains(&gains, debug)?;
                        }
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
            BurnStep::BurnMusicDvd { .. } => {
                return Err(Error::backend("A Music DVD can't be burned as a CD"));
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
