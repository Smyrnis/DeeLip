use super::*;
use deelip_config::SipAccount;

#[test]
fn account_codecs_honors_configured_order() {
    let acc = SipAccount { codec_order: vec!["pcma".into(), "pcmu".into()], ..Default::default() };
    assert_eq!(account_codecs(&acc), vec![AudioCodec::Pcma, AudioCodec::Pcmu]);
}

#[test]
fn account_codecs_falls_back_when_list_is_empty() {
    let acc = SipAccount { codec_order: vec![], ..Default::default() };
    assert_eq!(account_codecs(&acc).len(), ALL_CODECS.len());
}

#[test]
fn account_codecs_skips_unrecognized_entries() {
    let acc = SipAccount { codec_order: vec!["opus".into(), "carrier-pigeon".into()], ..Default::default() };
    assert_eq!(account_codecs(&acc), vec![AudioCodec::Opus]);
}

fn test_addr() -> std::net::SocketAddr {
    "192.0.2.1:40000".parse().unwrap()
}

#[test]
fn resolve_video_media_without_srtp_or_relay() {
    let (media, ready) = resolve_video_media(40000, None, None, None, VideoCodec::H264, test_addr(), None, false);
    assert_eq!(media.local_rtp, 40000);
    assert_eq!(media.codec, VideoCodec::H264);
    assert!(media.local_srtp.is_none());
    assert!(media.relay.is_none());
    assert!(media.ice.is_none());
    assert_eq!(ready.codec, VideoCodec::H264);
    assert_eq!(ready.local_rtp, 40000);
    assert_eq!(ready.remote_rtp, test_addr());
    assert!(ready.srtp.is_none());
    assert!(ready.relay.is_none());
}

#[test]
fn resolve_video_media_with_srtp_on_both_sides_derives_a_session() {
    let local = SrtpParams::generate();
    let remote = SrtpParams::generate();
    let (media, ready) = resolve_video_media(
        40000,
        Some(local.clone()),
        None,
        None,
        VideoCodec::H264,
        test_addr(),
        Some(remote.clone()),
        true,
    );
    assert_eq!(media.local_srtp, Some(local.clone()));
    let session = ready.srtp.expect("both sides offered SRTP");
    assert_eq!(session.local, local);
    assert_eq!(session.remote, remote);
}

#[test]
fn resolve_video_media_missing_remote_srtp_falls_back_to_plaintext() {
    let local = SrtpParams::generate();
    let (_media, ready) = resolve_video_media(
        40000,
        Some(local),
        None,
        None,
        VideoCodec::H264,
        test_addr(),
        None, // remote didn't offer a=crypto
        true, // we wanted SRTP
    );
    assert!(ready.srtp.is_none(), "no remote crypto line -- must fall back to plaintext, not error");
}

#[tokio::test]
async fn try_answer_with_ice_raw_returns_none_when_disabled() {
    let network = NetworkConfig::default();
    let result = try_answer_with_ice_raw(&network, false, Some("ufrag"), Some("pwd"), &["cand".to_string()]).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn try_answer_with_ice_raw_returns_none_without_remote_ice_params() {
    let network = NetworkConfig::default();
    // enabled=true, but no ufrag/pwd/candidates -- same as an offer that
    // never signaled ICE support at all.
    assert!(try_answer_with_ice_raw(&network, true, None, None, &[]).await.is_none());
    assert!(try_answer_with_ice_raw(&network, true, Some("ufrag"), Some("pwd"), &[]).await.is_none());
}

#[tokio::test]
async fn finish_ice_connect_raw_returns_none_when_gather_never_happened() {
    // `gathered: None` means ICE wasn't attempted on our side at all --
    // must short-circuit before touching the network regardless of the
    // remote's own ICE params.
    let result = finish_ice_connect_raw(None, true, Some("ufrag"), Some("pwd"), &["cand".to_string()]).await;
    assert!(result.is_none());
}
