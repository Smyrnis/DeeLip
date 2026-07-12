use std::sync::{Arc, Mutex};
use std::time::Duration;

use deelip_config::CallDirection;
use deelip_media::video_capture::{self, CaptureHandle};
use deelip_media::video_engine::{VideoConferenceLeg, VideoEngine};
use deelip_media::{ConferenceLeg, MediaEngine, MediaEngineOptions, RecordingOptions, ZrtpParams};
use deelip_sip::zrtp::Role;
use deelip_sip::VideoMediaReady;

use crate::app::{DeelipApp, VideoCallState};
use crate::strings::t;

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
        self.start_video_internal(video, None, None);
    }

    /// Shared by `start_video` (ordinary call, `second_leg`/`existing_camera`
    /// both `None`) and `start_conference` (fans the same camera out to a
    /// second remote party). `existing_camera`, if given, is reused instead
    /// of opening a fresh capture -- only one physical camera is ever
    /// captured from at a time, so a conference reuses whichever handle the
    /// merged call already had running rather than closing and reopening it.
    fn start_video_internal(
        &mut self, primary: VideoMediaReady, second_leg: Option<VideoMediaReady>,
        existing_camera: Option<CaptureHandle>,
    ) {
        let width = self.config.audio.video_capture_width;
        let height = self.config.audio.video_capture_height;
        let fps = self.config.audio.video_fps;
        let bitrate_bps = self.config.audio.video_bitrate_bps;

        let camera: Option<CaptureHandle> = existing_camera.or_else(|| {
            let camera_name = self.config.audio.camera_device.clone();
            let camera_index = camera_name
                .as_deref()
                .and_then(video_capture::find_camera_by_name)
                .or_else(|| video_capture::list_cameras().into_iter().next().map(|(idx, _)| idx));
            camera_index.and_then(|idx| match video_capture::start_capture(idx, width, height, fps) {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::warn!("Camera capture unavailable, sending no video: {e:#}");
                    None
                }
            })
        });
        let frame_source = camera.as_ref().map(CaptureHandle::frame_slot).unwrap_or_else(|| Arc::new(Mutex::new(None)));

        let conference_leg = second_leg.map(|v| VideoConferenceLeg {
            local_rtp_port: v.local_rtp,
            remote_rtp: v.remote_rtp,
            srtp: v.srtp,
            relay: v.relay,
        });

        let engine = self.rt.block_on(VideoEngine::start(
            primary.local_rtp,
            primary.remote_rtp,
            primary.srtp,
            primary.relay,
            frame_source,
            fps,
            bitrate_bps,
            conference_leg,
        ));
        match engine {
            Ok(engine) => {
                self.video = Some(VideoCallState {
                    engine,
                    camera,
                    remote: Default::default(),
                    remote2: Default::default(),
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
            self.rt.block_on(async { tokio::time::sleep(Duration::from_millis(300)).await });
        }

        if let Some(engine) = self.media.take() {
            self.rt.block_on(engine.stop());
        }
        // Stop whichever call's `VideoEngine` was running (only the focused
        // call ever has one) but keep its camera handle -- a conference
        // still only captures from one physical camera, fanned out to both
        // legs, so there's no reason to close and reopen it if it's already
        // running (see `start_video_internal`'s `existing_camera` param).
        let existing_camera = self.video.take().and_then(|v| {
            self.rt.block_on(v.engine.stop());
            v.camera
        });

        let call_id0 = self.calls[0].call_id.clone();
        let media0 = self.calls[0].media.clone();
        let media1 = self.calls[1].media.clone();
        let video0 = media0.video.clone();
        let video1 = media1.video.clone();
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

                // Both legs negotiated video -> real 2-remote-party
                // conference video (fan-out, see `video_engine.rs`'s doc
                // comment). Only one did -> single-leg video for whichever
                // leg has it, same as an ordinary call; the other leg
                // simply has no video, nothing to bridge. Neither -> no
                // video state at all, unchanged from before this existed.
                match (video0, video1) {
                    (Some(v0), Some(v1)) => self.start_video_internal(v0, Some(v1), existing_camera),
                    (Some(v0), None) => self.start_video_internal(v0, None, existing_camera),
                    (None, Some(v1)) => self.start_video_internal(v1, None, existing_camera),
                    (None, None) => {}
                }
            }
            Err(e) => tracing::error!("Conference MediaEngine failed: {e}"),
        }
    }
}
