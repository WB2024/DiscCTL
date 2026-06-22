/// Hardware integration tests — require a physical CD-R drive and blank disc.
///
/// Run with:
///   cargo test --features hardware_tests -- --test-threads=1
///
/// Environment variables (all optional — defaults shown):
///   DISCCTL_TEST_DEVICE=/dev/sr0
///   DISCCTL_TEST_AUDIO_DIR=tests/fixtures/audio
///   DISCCTL_TEST_DATA_DIR=tests/fixtures/data
///   DISCCTL_ENABLE_BURN_TESTS=0   (set to 1 to run destructive burn tests)
#[cfg(feature = "hardware_tests")]
mod hardware {
    use std::env;
    use std::path::PathBuf;

    fn device() -> String {
        env::var("DISCCTL_TEST_DEVICE").unwrap_or_else(|_| "/dev/sr0".to_string())
    }

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    fn audio_dir() -> String {
        env::var("DISCCTL_TEST_AUDIO_DIR")
            .unwrap_or_else(|_| fixtures_dir().join("audio").to_string_lossy().to_string())
    }

    fn data_dir() -> String {
        env::var("DISCCTL_TEST_DATA_DIR")
            .unwrap_or_else(|_| fixtures_dir().join("data").to_string_lossy().to_string())
    }

    fn burn_tests_enabled() -> bool {
        env::var("DISCCTL_ENABLE_BURN_TESTS").as_deref() == Ok("1")
    }

    // ── Non-destructive: drive / stack checks ────────────────────────────

    #[test]
    fn device_node_exists() {
        assert!(
            std::path::Path::new(&device()).exists(),
            "Device {} not found. Attach a drive or set DISCCTL_TEST_DEVICE.",
            device()
        );
    }

    #[test]
    fn disc_state_is_detectable() {
        use discctl::backend::device as dev;
        let state = dev::query_disc_state(&device()).expect("query_disc_state failed");
        println!("Disc state: {:?}", state);
    }

    #[test]
    fn media_type_is_detectable() {
        use discctl::backend::device as dev;
        let media = dev::detect_media_type(&device()).expect("detect_media_type failed");
        println!("Media type: {:?}", media);
    }

    #[test]
    fn buffer_underrun_protection_detected() {
        use discctl::backend::device as dev;
        let has_bup = dev::has_buffer_underrun_protection(&device())
            .expect("has_buffer_underrun_protection failed");
        println!("Buffer underrun protection: {}", has_bup);
        // ASUS SDRW-08D2S-U has BurnProof — assert it's detected correctly
        assert!(has_bup, "Expected BurnProof to be detected on ASUS SDRW-08D2S-U");
    }

    // ── Non-destructive: fixture + planner checks ────────────────────────

    #[test]
    fn fixture_wavs_exist_and_are_valid() {
        use discctl::planner;
        let adir = audio_dir();
        let pattern = format!("{}/*.wav", adir);
        let graph = discctl::parser::from_cli("redbook", Some(&[pattern]), None, None, "Fixture Test", false)
            .expect("from_cli failed");
        // validate() checks WAV headers — will catch any spec violations
        planner::validate(&graph).expect("WAV validation failed on fixture files");
        println!("Fixtures validated: {:?}", graph.sessions);
    }

    #[test]
    fn redbook_dry_run_plan() {
        let pattern = format!("{}/*.wav", audio_dir());
        let graph =
            discctl::parser::from_cli("redbook", Some(&[pattern]), None, None, "HW Test RedBook", false)
                .unwrap();
        let plan = discctl::planner::plan(&graph).unwrap();
        println!("{}", serde_json::to_string_pretty(&plan).unwrap());
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn datacd_dry_run_plan() {
        let graph =
            discctl::parser::from_cli("datacd", None, None, Some(&data_dir()), "HW Test DataCD", false)
                .unwrap();
        let plan = discctl::planner::plan(&graph).unwrap();
        println!("{}", serde_json::to_string_pretty(&plan).unwrap());
        assert_eq!(plan.steps.len(), 2);
    }

    #[test]
    fn bluebook_dry_run_plan() {
        let pattern = format!("{}/*.wav", audio_dir());
        let graph = discctl::parser::from_cli(
            "bluebook",
            Some(&[pattern]),
            None,
            Some(&data_dir()),
            "HW Test BlueBook",
            false,
        )
        .unwrap();
        let plan = discctl::planner::plan(&graph).unwrap();
        println!("{}", serde_json::to_string_pretty(&plan).unwrap());
        assert_eq!(plan.steps.len(), 3);
    }

    // ── Destructive: actual burns (opt-in) ───────────────────────────────

    #[test]
    fn redbook_burn() {
        if !burn_tests_enabled() {
            println!("Skipped. Set DISCCTL_ENABLE_BURN_TESTS=1 to run.");
            return;
        }
        let pattern = format!("{}/*.wav", audio_dir());
        let graph =
            discctl::parser::from_cli("redbook", Some(&[pattern]), None, None, "HW Burn RedBook", false)
                .unwrap();
        let plan = discctl::planner::plan(&graph).unwrap();
        discctl::backend::execute(&graph, &plan, &device(), true).unwrap();
        println!("Red Book burn complete.");
    }

    #[test]
    fn datacd_burn() {
        if !burn_tests_enabled() {
            println!("Skipped. Set DISCCTL_ENABLE_BURN_TESTS=1 to run.");
            return;
        }
        let graph =
            discctl::parser::from_cli("datacd", None, None, Some(&data_dir()), "HW Burn DataCD", false)
                .unwrap();
        let plan = discctl::planner::plan(&graph).unwrap();
        discctl::backend::execute(&graph, &plan, &device(), true).unwrap();
        println!("Data CD burn complete.");
    }

    #[test]
    fn bluebook_burn() {
        if !burn_tests_enabled() {
            println!("Skipped. Set DISCCTL_ENABLE_BURN_TESTS=1 to run.");
            return;
        }
        let pattern = format!("{}/*.wav", audio_dir());
        let graph = discctl::parser::from_cli(
            "bluebook",
            Some(&[pattern]),
            None,
            Some(&data_dir()),
            "HW Burn BlueBook",
            false,
        )
        .unwrap();
        let plan = discctl::planner::plan(&graph).unwrap();
        discctl::backend::execute(&graph, &plan, &device(), true).unwrap();
        println!("Blue Book burn complete.");
    }

    #[test]
    fn cdrw_blank_and_redbook_burn() {
        if !burn_tests_enabled() {
            println!("Skipped. Set DISCCTL_ENABLE_BURN_TESTS=1 to run.");
            return;
        }
        use discctl::backend::device as dev;
        // Only run on CD-RW
        if dev::detect_media_type(&device()).unwrap() != dev::DiscMediaType::CdRw {
            println!("Skipped: not a CD-RW disc.");
            return;
        }
        println!("Blanking CD-RW...");
        dev::blank_cdrw(&device(), "fast", true).unwrap();
        println!("Burning Red Book after blank...");
        let pattern = format!("{}/*.wav", audio_dir());
        let graph =
            discctl::parser::from_cli("redbook", Some(&[pattern]), None, None, "HW Blank+Burn", false)
                .unwrap();
        let plan = discctl::planner::plan(&graph).unwrap();
        discctl::backend::execute(&graph, &plan, &device(), true).unwrap();
        println!("CD-RW blank+burn complete.");
    }
}
