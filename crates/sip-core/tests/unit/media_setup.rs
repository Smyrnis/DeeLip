use super::*;
use deelip_config::SipAccount;

#[test]
fn account_codecs_honors_configured_order() {
    let acc = SipAccount {
        codec_order: vec!["pcma".into(), "pcmu".into()],
        ..Default::default()
    };
    assert_eq!(
        account_codecs(&acc),
        vec![AudioCodec::Pcma, AudioCodec::Pcmu]
    );
}

#[test]
fn account_codecs_falls_back_when_list_is_empty() {
    let acc = SipAccount {
        codec_order: vec![],
        ..Default::default()
    };
    assert_eq!(account_codecs(&acc).len(), ALL_CODECS.len());
}

#[test]
fn account_codecs_skips_unrecognized_entries() {
    let acc = SipAccount {
        codec_order: vec!["opus".into(), "carrier-pigeon".into()],
        ..Default::default()
    };
    assert_eq!(account_codecs(&acc), vec![AudioCodec::Opus]);
}
