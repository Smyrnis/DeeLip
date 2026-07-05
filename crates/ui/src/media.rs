use std::time::Duration;

use deelip_media::{ConferenceLeg, MediaEngine};

use crate::app::DeelipApp;

impl DeelipApp {
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
        let engine = rt.block_on(MediaEngine::start(
            media.local_rtp,
            media.remote_rtp,
            media.codec,
            media.dtmf_type,
            media.srtp,
            media.relay,
            self.config.audio.echo_cancellation,
            input_device.as_deref(),
            output_device.as_deref(),
            self.config.recording_enabled,
            &call_id,
            None,
        ));
        match engine {
            Ok(e) => {
                self.media = Some(e);
                self.focused_call = Some(idx);
            }
            Err(e) => {
                tracing::error!("MediaEngine failed: {e}");
            }
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

        let engine = rt.block_on(MediaEngine::start(
            media0.local_rtp,
            media0.remote_rtp,
            media0.codec,
            media0.dtmf_type,
            media0.srtp,
            media0.relay,
            self.config.audio.echo_cancellation,
            input_device.as_deref(),
            output_device.as_deref(),
            self.config.recording_enabled,
            &call_id0,
            Some(leg2),
        ));
        match engine {
            Ok(e) => {
                self.media = Some(e);
                self.focused_call = Some(0);
                self.calls[0].is_held = false;
                self.calls[1].is_held = false;
                self.in_conference = true;
                self.attended_transfer_original = None;
                self.status_line = "In conference".into();
            }
            Err(e) => tracing::error!("Conference MediaEngine failed: {e}"),
        }
    }
}
