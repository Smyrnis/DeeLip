use std::sync::{Arc, Mutex};
use std::time::Duration;

use deelip_config::CallDirection;
use deelip_media::video_capture::{self, CaptureHandle};
use deelip_media::video_engine::VideoEngine;
use deelip_media::{ConferenceLeg, MediaEngine, MediaEngineOptions, RecordingOptions, ZrtpParams};
use deelip_sip::zrtp::Role;
use deelip_sip::VideoMediaReady;

use crate::app::{DeelipApp, VideoCallState};
use crate::strings::t;

/// Hardcoded video call parameters -- SDP negotiates codec/SRTP/ICE only,
/// never resolution/framerate/bitrate; no per-account Settings surface for
/// these yet (disclosed scope cut, same spirit as "H.264 only").
const VIDEO_CAPTURE_WIDTH: u32 = 640;
const VIDEO_CAPTURE_HEIGHT: u32 = 480;
const VIDEO_FPS: u32 = 15;
const VIDEO_BITRATE_BPS: u32 = 500_000;

impl DeelipApp {
    /// Builds `ZrtpParams` for `calls[idx]` if its account has ZRTP enabled
    /// -- `Role::Initiator` for a call we placed (we sent the INVITE),
    /// `Role::Responder` for one we answered, matching
    /// `deelip_sip::zrtp::engine`'s module doc. Lazily generates and
    /// persists this installation's ZID on first use.
    fn zrtp_params_for(&mut self, idx: usize) -> Option<ZrtpParams> {
        let call = &self.calls[idx];
        if !self.accounts[call.account].account.wants_zrtp() {
            return None;
        }
        let role = match call.direction {
            CallDirection::Outbound => Role::Initiator,
            CallDirection::Inbound => Role::Responder,
        };
        match self.config.zrtp_zid_bytes(&self.db) {
            Ok(local_zid) => Some(ZrtpParams { role, local_zid }),
            Err(e) => {
                tracing::error!("Failed to load/generate ZRTP ZID: {e}");
                None
            }
        }
    }

    /// Start (or restart, on resume) media for `calls[idx]`, using its own
    /// already-negotiated `CallMediaReady` -- `SipStack` resolved codec/SRTP/
    /// ICE/TURN before the call ever connected, so there's no SDP to parse
    /// or endpoint to (re-)resolve here. Marks it `focused_call` on success.
    pub(crate) fn start_media(&mut self, idx: usize) {
        let call_id = self.calls[idx].call_id.clone();
        let media = self.calls[idx].media.clone();
        let rt = self.rt.clone();
        let input_device = self.config.audio.input_device.clone();
        let output_device = self.config.audio.output_device.clone();
        let zrtp = self.zrtp_params_for(idx);
        let engine = rt.block_on(MediaEngine::start(MediaEngineOptions {
            local_rtp_port: media.local_rtp,
            remote_rtp: media.remote_rtp,
            codec: media.codec,
            dtmf_pt: media.dtmf_type,
            cn_pt: media.cn_type,
            srtp: media.srtp,
            relay: media.relay,
            echo_cancellation: self.config.audio.echo_cancellation,
            agc_enabled: self.config.audio.agc_enabled,
            input_device: input_device.as_deref(),
            output_device: output_device.as_deref(),
            recording: RecordingOptions {
                enabled: self.calls[idx].recording_enabled,
                format: self.config.recording_format,
                dir_override: self.config.recordings_dir_override.clone(),
            },
            call_id: &call_id,
            second_leg: None,
            zrtp,
        }));
        match engine {
            Ok(e) => {
                self.media = Some(e);
                self.focused_call = Some(idx);
                if let Some(video) = media.video {
                    self.start_video(video);
                }
            }
            Err(e) => {
                tracing::error!("MediaEngine failed: {e}");
            }
        }
    }

    /// Start a `VideoEngine` for a negotiated video leg, alongside the
    /// audio `MediaEngine` `start_media` just started -- always additive,
    /// never a reason to fail the call itself (mirrors `sip-core`'s own
    /// "video is additive" rationale for its negotiation side). Tries the
    /// configured (or first enumerated) camera; if none is available or it
    /// fails to open, still starts the engine with an empty frame source,
    /// so this side can receive and display the remote party's video even
    /// without a working camera of its own.
    fn start_video(&mut self, video: VideoMediaReady) {
        let camera_name = self.config.audio.camera_device.clone();
        let camera_index = camera_name
            .as_deref()
            .and_then(video_capture::find_camera_by_name)
            .or_else(|| video_capture::list_cameras().into_iter().next().map(|(idx, _)| idx));

        let camera: Option<CaptureHandle> = camera_index.and_then(|idx| {
            match video_capture::start_capture(idx, VIDEO_CAPTURE_WIDTH, VIDEO_CAPTURE_HEIGHT, VIDEO_FPS) {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::warn!("Camera capture unavailable, sending no video: {e:#}");
                    None
                }
            }
        });
        let frame_source = camera
            .as_ref()
            .map(CaptureHandle::frame_slot)
            .unwrap_or_else(|| Arc::new(Mutex::new(None)));

        let engine = self.rt.block_on(VideoEngine::start(
            video.local_rtp,
            video.remote_rtp,
            video.srtp,
            video.relay,
            frame_source,
            VIDEO_FPS,
            VIDEO_BITRATE_BPS,
        ));
        match engine {
            Ok(engine) => {
                self.video = Some(VideoCallState {
                    engine,
                    camera,
                    remote: Default::default(),
                    local: Default::default(),
                });
            }
            Err(e) => tracing::error!("VideoEngine failed: {e:#}"),
        }
    }

    /// Merge the two currently-connected calls into a local 3-way
    /// conference: stops the single-leg `MediaEngine` and starts a
    /// conference-mode one bridging both remote parties into the same
    /// mic/speaker pair. Needs no new SIP signaling -- both remote parties
    /// stay in an ordinary 2-party call with DeeLip; only local audio
    /// mixing changes.
    pub(crate) fn start_conference(&mut self) {
        if self.calls.len() != 2 {
            return;
        }

        // Any held leg was put on hold with a=sendonly, telling the far end
        // to stop sending us audio -- send a real resume re-INVITE
        // (a=sendrecv) so it actually resumes before we start mixing it
        // in, or that leg would come through silent even though we're now
        // "listening" locally (this is exactly the case for a call held
        // as part of the attended-transfer consultation flow, and equally
        // for an ordinary call-waiting pair where one side is on hold).
        let mut resumed = false;
        if self.calls[0].is_held {
            self.send_resume(0);
            resumed = true;
        }
        if self.calls[1].is_held {
            self.send_resume(1);
            resumed = true;
        }
        if resumed {
            // Fire-and-forget like hold/resume already is everywhere else in
            // this codebase, but this one case is more timing-sensitive than
            // usual: we're about to tear down and rebuild the whole engine
            // right after, so give the far end a brief moment to actually
            // process the re-INVITE and resume sending before we do (same
            // precedent as `hangup_before_exit`'s post-BYE grace sleep).
            // (See `hangup_before_exit` for why this must be an async block,
            // not a bare `tokio::time::sleep(...)` argument.)
            self.rt
                .block_on(async { tokio::time::sleep(Duration::from_millis(300)).await });
        }

        if let Some(engine) = self.media.take() {
            self.rt.block_on(engine.stop());
        }
        // Conferencing stays video-free (see `video_engine.rs`'s own doc
        // comment) -- drop any video leg one of the merged calls had
        // instead of leaking its task.
        if let Some(v) = self.video.take() {
            self.rt.block_on(v.engine.stop());
        }

        let call_id0 = self.calls[0].call_id.clone();
        let media0 = self.calls[0].media.clone();
        let media1 = self.calls[1].media.clone();
        let rt = self.rt.clone();
        let input_device = self.config.audio.input_device.clone();
        let output_device = self.config.audio.output_device.clone();

        let leg2 = ConferenceLeg {
            local_rtp_port: media1.local_rtp,
            remote_rtp: media1.remote_rtp,
            codec: media1.codec,
            dtmf_pt: media1.dtmf_type,
            srtp: media1.srtp,
            relay: media1.relay,
        };

        let engine = rt.block_on(MediaEngine::start(MediaEngineOptions {
            local_rtp_port: media0.local_rtp,
            remote_rtp: media0.remote_rtp,
            codec: media0.codec,
            dtmf_pt: media0.dtmf_type,
            cn_pt: media0.cn_type,
            srtp: media0.srtp,
            relay: media0.relay,
            echo_cancellation: self.config.audio.echo_cancellation,
            agc_enabled: self.config.audio.agc_enabled,
            input_device: input_device.as_deref(),
            output_device: output_device.as_deref(),
            recording: RecordingOptions {
                enabled: self.calls[0].recording_enabled,
                format: self.config.recording_format,
                dir_override: self.config.recordings_dir_override.clone(),
            },
            call_id: &call_id0,
            second_leg: Some(leg2),
            // ZRTP isn't supported for conference calls -- see
            // `MediaEngineOptions::zrtp`'s doc comment.
            zrtp: None,
        }));
        match engine {
            Ok(e) => {
                self.media = Some(e);
                self.focused_call = Some(0);
                self.calls[0].is_held = false;
                self.calls[1].is_held = false;
                self.in_conference = true;
                self.attended_transfer_original = None;
                self.status_line = t("status.in_conference_line");
            }
            Err(e) => tracing::error!("Conference MediaEngine failed: {e}"),
        }
    }
}
