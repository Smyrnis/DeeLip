use super::*;

#[test]
fn keeps_default_and_real_devices() {
    assert!(!is_irrelevant_alsa_device("default"));
    assert!(!is_irrelevant_alsa_device("hw:CARD=Generic,DEV=0"));
    assert!(!is_irrelevant_alsa_device("front:CARD=Generic,DEV=0"));
    assert!(!is_irrelevant_alsa_device("pulse"));
}

#[test]
fn excludes_surround_devices() {
    assert!(is_irrelevant_alsa_device("surround21:CARD=Generic,DEV=0"));
    assert!(is_irrelevant_alsa_device("surround40:CARD=Generic,DEV=0"));
    assert!(is_irrelevant_alsa_device("surround41:CARD=Generic,DEV=0"));
    assert!(is_irrelevant_alsa_device("surround50:CARD=Generic,DEV=0"));
    assert!(is_irrelevant_alsa_device("surround51:CARD=Generic,DEV=0"));
    assert!(is_irrelevant_alsa_device("surround71:CARD=Generic,DEV=0"));
}

#[test]
fn excludes_digital_passthrough_devices() {
    assert!(is_irrelevant_alsa_device("iec958:CARD=Generic,DEV=0"));
    assert!(is_irrelevant_alsa_device("spdif:CARD=Generic,DEV=0"));
}

#[test]
fn is_case_insensitive() {
    assert!(is_irrelevant_alsa_device("Surround40:CARD=Generic,DEV=0"));
}
