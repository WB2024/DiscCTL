/// Hardware integration tests — require a physical CD-R drive and blank disc.
///
/// Run with:
///   cargo test --features hardware_tests -- --test-thread=1
///
/// Set DISCCTL_TEST_DEVICE to override the default /dev/sr0.
#[cfg(feature = "hardware_tests")]
mod hardware {
    use std::env;

    fn test_device() -> String {
        env::var("DISCCTL_TEST_DEVICE").unwrap_or_else(|_| "/dev/sr0".to_string())
    }

    fn test_audio_dir() -> String {
        env::var("DISCCTL_TEST_AUDIO_DIR")
            .expect("Set DISCCTL_TEST_AUDIO_DIR to a directory of 44100Hz 16-bit stereo WAV files")
    }

    fn test_data_dir() -> String {
        env::var("DISCCTL_TEST_DATA_DIR")
            .expect("Set DISCCTL_TEST_DATA_DIR to a directory of files to burn as data")
    }

    #[test]
    fn device_exists() {
        let dev = test_device();
        assert!(
            std::path::Path::new(&dev).exists(),
            "Device {} not found",
            dev
        );
    }

    #[test]
    fn disc_state_is_detectable() {
        use discctl::backend::device;
        let dev = test_device();
        let state = device::query_disc_state(&dev);
        assert!(state.is_ok(), "query_disc_state failed: {:?}", state);
        println!("Disc state: {:?}", state.unwrap());
    }

    #[test]
    fn media_type_is_detectable() {
        use discctl::backend::device;
        let dev = test_device();
        let media = device::detect_media_type(&dev);
        assert!(media.is_ok(), "detect_media_type failed: {:?}", media);
        println!("Media type: {:?}", media.unwrap());
    }

    #[test]
    fn redbook_dry_run_succeeds() {
        use discctl::{parser, planner};
        let audio_dir = test_audio_dir();
        let pattern = format!("{}/*.wav", audio_dir);
        let graph = parser::from_cli("redbook", Some(&[pattern]), None, "Hardware Test").unwrap();
        let plan = planner::plan(&graph).unwrap();
        println!("Plan: {}", serde_json::to_string_pretty(&plan).unwrap());
        // dry-run: plan only, no actual burn
    }

    #[test]
    fn bluebook_dry_run_succeeds() {
        use discctl::{parser, planner};
        let audio_dir = test_audio_dir();
        let data_dir = test_data_dir();
        let pattern = format!("{}/*.wav", audio_dir);
        let graph = parser::from_cli(
            "bluebook",
            Some(&[pattern]),
            Some(&data_dir),
            "Hardware Test BlueBook",
        )
        .unwrap();
        let plan = planner::plan(&graph).unwrap();
        println!("Plan: {}", serde_json::to_string_pretty(&plan).unwrap());
    }

    /// DESTRUCTIVE: actually burns a Red Book audio CD.
    /// Only runs when DISCCTL_ENABLE_BURN_TESTS=1 is set.
    #[test]
    fn redbook_burn() {
        if env::var("DISCCTL_ENABLE_BURN_TESTS").as_deref() != Ok("1") {
            println!("Skipping burn test. Set DISCCTL_ENABLE_BURN_TESTS=1 to enable.");
            return;
        }
        use discctl::{backend, parser, planner};
        let dev = test_device();
        let audio_dir = test_audio_dir();
        let pattern = format!("{}/*.wav", audio_dir);
        let graph =
            parser::from_cli("redbook", Some(&[pattern]), None, "Hardware Burn Test").unwrap();
        let plan = planner::plan(&graph).unwrap();
        backend::execute(&graph, &plan, &dev, true).unwrap();
    }

    /// DESTRUCTIVE: actually burns a Blue Book (CD Extra) disc.
    #[test]
    fn bluebook_burn() {
        if env::var("DISCCTL_ENABLE_BURN_TESTS").as_deref() != Ok("1") {
            println!("Skipping burn test. Set DISCCTL_ENABLE_BURN_TESTS=1 to enable.");
            return;
        }
        use discctl::{backend, parser, planner};
        let dev = test_device();
        let audio_dir = test_audio_dir();
        let data_dir = test_data_dir();
        let pattern = format!("{}/*.wav", audio_dir);
        let graph = parser::from_cli(
            "bluebook",
            Some(&[pattern]),
            Some(&data_dir),
            "Hardware Burn Test BlueBook",
        )
        .unwrap();
        let plan = planner::plan(&graph).unwrap();
        backend::execute(&graph, &plan, &dev, true).unwrap();
    }
}
