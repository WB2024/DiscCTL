use crate::{
    error::Error,
    model::{
        disc::{DiscFormat, DiscGraph, Session},
        plan::{BurnPlan, BurnStep},
    },
};

pub fn plan(graph: &DiscGraph) -> Result<BurnPlan, Error> {
    validate(graph)?;
    let steps = build_steps(graph)?;
    Ok(BurnPlan {
        format: graph.format.to_string(),
        label: graph.label.clone(),
        steps,
    })
}

pub fn validate(graph: &DiscGraph) -> Result<(), Error> {
    validate_structure(graph)?;
    validate_files(graph)
}

pub fn validate_structure(graph: &DiscGraph) -> Result<(), Error> {
    match graph.format {
        DiscFormat::RedBook => validate_redbook_structure(graph),
        DiscFormat::DataCD => validate_datacd_structure(graph),
        DiscFormat::BlueBook => validate_bluebook_structure(graph),
    }
}

fn validate_redbook_structure(graph: &DiscGraph) -> Result<(), Error> {
    if graph.sessions.is_empty() {
        return Err(Error::validation("RedBook requires at least one audio session"));
    }
    if graph.sessions.len() > 1 {
        return Err(Error::validation("RedBook supports only a single session"));
    }
    match &graph.sessions[0] {
        Session::Audio(a) if a.tracks.is_empty() => {
            Err(Error::validation("Audio session has no tracks"))
        }
        Session::Audio(_) => Ok(()),
        Session::Data(_) => Err(Error::validation("RedBook format requires an audio session")),
    }
}

fn validate_datacd_structure(graph: &DiscGraph) -> Result<(), Error> {
    if graph.sessions.len() != 1 {
        return Err(Error::validation(format!(
            "DataCD requires exactly 1 data session, got {}",
            graph.sessions.len()
        )));
    }
    match &graph.sessions[0] {
        Session::Data(_) => Ok(()),
        Session::Audio(_) => Err(Error::validation("DataCD format requires a data session")),
    }
}

fn validate_bluebook_structure(graph: &DiscGraph) -> Result<(), Error> {
    if graph.sessions.len() != 2 {
        return Err(Error::validation(format!(
            "BlueBook (CD Extra) requires exactly 2 sessions (audio + data), got {}",
            graph.sessions.len()
        )));
    }
    match &graph.sessions[0] {
        Session::Data(_) => {
            return Err(Error::validation(
                "SESSION_ORDER_INVALID: Data session cannot precede audio session in BlueBook format",
            ));
        }
        Session::Audio(a) if a.tracks.is_empty() => {
            return Err(Error::validation("BlueBook session 1 (audio) has no tracks"));
        }
        Session::Audio(_) => {}
    }
    match &graph.sessions[1] {
        Session::Audio(_) => Err(Error::validation("BlueBook session 2 must be a data session")),
        Session::Data(_) => Ok(()),
    }
}

fn validate_files(graph: &DiscGraph) -> Result<(), Error> {
    for session in &graph.sessions {
        match session {
            Session::Audio(audio) => {
                for track in &audio.tracks {
                    let path = std::path::Path::new(track);
                    if !path.exists() {
                        return Err(Error::validation(format!("Track file not found: {}", track)));
                    }
                    match path.extension().and_then(|e| e.to_str()) {
                        Some("wav") | Some("flac") => {}
                        Some(ext) => {
                            return Err(Error::validation(format!(
                                "Unsupported audio format '{}' for: {}. Use wav or flac",
                                ext, track
                            )));
                        }
                        None => {
                            return Err(Error::validation(format!(
                                "Track has no file extension: {}",
                                track
                            )));
                        }
                    }
                }
            }
            Session::Data(data) => {
                if !std::path::Path::new(&data.source_dir).exists() {
                    return Err(Error::validation(format!(
                        "Source directory not found: {}",
                        data.source_dir
                    )));
                }
            }
        }
    }
    Ok(())
}

pub fn build_steps(graph: &DiscGraph) -> Result<Vec<BurnStep>, Error> {
    match graph.format {
        DiscFormat::RedBook => Ok(vec![BurnStep::BurnAudioSession {
            session_index: 0,
            finalize: true,
        }]),
        DiscFormat::DataCD => {
            let filesystem = match &graph.sessions[0] {
                Session::Data(d) => d.filesystem.to_string(),
                _ => "iso9660".to_string(),
            };
            Ok(vec![
                BurnStep::AppendDataSession {
                    session_index: 0,
                    filesystem,
                },
                BurnStep::FinalizeDisc,
            ])
        }
        DiscFormat::BlueBook => {
            let filesystem = match &graph.sessions[1] {
                Session::Data(d) => d.filesystem.to_string(),
                _ => "iso9660".to_string(),
            };
            Ok(vec![
                BurnStep::BurnAudioSession {
                    session_index: 0,
                    finalize: false,
                },
                BurnStep::AppendDataSession {
                    session_index: 1,
                    filesystem,
                },
                BurnStep::FinalizeDisc,
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::disc::*;

    fn audio(tracks: Vec<&str>) -> Session {
        Session::Audio(AudioSession {
            tracks: tracks.iter().map(|s| s.to_string()).collect(),
            cd_text: None,
        })
    }

    fn data(src: &str) -> Session {
        Session::Data(DataSession {
            source_dir: src.to_string(),
            filesystem: Filesystem::Iso9660,
            joliet: false,
            rock_ridge: false,
        })
    }

    fn graph(format: DiscFormat, sessions: Vec<Session>) -> DiscGraph {
        DiscGraph {
            format,
            label: "Test".to_string(),
            sessions,
        }
    }

    #[test]
    fn redbook_valid_structure() {
        let g = graph(DiscFormat::RedBook, vec![audio(vec!["t.wav"])]);
        assert!(validate_structure(&g).is_ok());
    }

    #[test]
    fn redbook_rejects_empty_sessions() {
        let g = graph(DiscFormat::RedBook, vec![]);
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn redbook_rejects_data_session() {
        let g = graph(DiscFormat::RedBook, vec![data("/tmp")]);
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn redbook_rejects_empty_tracks() {
        let g = graph(DiscFormat::RedBook, vec![audio(vec![])]);
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn datacd_valid_structure() {
        let g = graph(DiscFormat::DataCD, vec![data("/tmp")]);
        assert!(validate_structure(&g).is_ok());
    }

    #[test]
    fn datacd_rejects_audio() {
        let g = graph(DiscFormat::DataCD, vec![audio(vec!["t.wav"])]);
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn bluebook_valid_structure() {
        let g = graph(
            DiscFormat::BlueBook,
            vec![audio(vec!["t.wav"]), data("/tmp")],
        );
        assert!(validate_structure(&g).is_ok());
    }

    #[test]
    fn bluebook_rejects_wrong_order() {
        let g = graph(
            DiscFormat::BlueBook,
            vec![data("/tmp"), audio(vec!["t.wav"])],
        );
        let err = validate_structure(&g).unwrap_err();
        assert!(err.to_string().contains("SESSION_ORDER_INVALID"));
    }

    #[test]
    fn bluebook_rejects_single_session() {
        let g = graph(DiscFormat::BlueBook, vec![audio(vec!["t.wav"])]);
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn bluebook_rejects_two_audio_sessions() {
        let g = graph(
            DiscFormat::BlueBook,
            vec![audio(vec!["t.wav"]), audio(vec!["t2.wav"])],
        );
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn redbook_generates_one_step() {
        let g = graph(DiscFormat::RedBook, vec![audio(vec!["t.wav"])]);
        let steps = build_steps(&g).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(matches!(
            &steps[0],
            BurnStep::BurnAudioSession { finalize: true, .. }
        ));
    }

    #[test]
    fn datacd_generates_two_steps() {
        let g = graph(DiscFormat::DataCD, vec![data("/tmp")]);
        let steps = build_steps(&g).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(matches!(&steps[0], BurnStep::AppendDataSession { .. }));
        assert!(matches!(&steps[1], BurnStep::FinalizeDisc));
    }

    #[test]
    fn bluebook_generates_three_steps() {
        let g = graph(
            DiscFormat::BlueBook,
            vec![audio(vec!["t.wav"]), data("/tmp")],
        );
        let steps = build_steps(&g).unwrap();
        assert_eq!(steps.len(), 3);
        assert!(matches!(
            &steps[0],
            BurnStep::BurnAudioSession { finalize: false, .. }
        ));
        assert!(matches!(&steps[1], BurnStep::AppendDataSession { .. }));
        assert!(matches!(&steps[2], BurnStep::FinalizeDisc));
    }
}
