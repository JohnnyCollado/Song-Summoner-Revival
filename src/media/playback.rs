/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Playing a song from the user's library.
//!
//! Symphonia decodes the file a little at a time and the PCM goes to an
//! OpenAL source through a short queue of buffers, the same audio stack
//! touchHLE uses for the game's own sound, so it works on every platform.
//! The player has its own OpenAL context, like AudioQueue does; the guest's
//! OpenAL functions make their context current again on every call.
//!
//! Nothing here runs by itself: [Player::pump] must be called regularly
//! (MPMusicPlayerController does it from the run loop) to refill the queue.

use crate::audio::openal as al;
use crate::audio::openal::al_types::*;
use crate::audio::openal::{OpenAL, OpenALManager};
use crate::frameworks::audio_toolbox::LazyALContext;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::formats::FormatReader;
use symphonia::core::io::{MediaSource, MediaSourceStream};

// Not in touchHLE's OpenAL bindings.
const AL_GAIN: ALenum = 0x100A;
const AL_SAMPLE_OFFSET: ALenum = 0x1025;

/// Buffers in flight. With [BUFFER_SECONDS] each, that's how far ahead of
/// the speaker the decoder runs.
const QUEUE_LENGTH: usize = 4;
const BUFFER_SECONDS: f64 = 0.25;

/// What [Player::pump] saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PumpResult {
    /// Nothing loaded.
    Idle,
    Playing,
    Paused,
    /// The song played to its end (reported once).
    Finished,
}

pub struct Player {
    al: LazyALContext,
    state: State,
}

#[derive(Default)]
struct State {
    source: Option<ALuint>,
    spare_buffers: Vec<ALuint>,
    /// Frames in each buffer still queued on the source, oldest first.
    queued_frames: VecDeque<u64>,
    /// Frames in buffers that have finished playing.
    frames_done: u64,
    stream: Option<Stream>,
    song_id: u64,
    paused: bool,
    volume: f32,
}

impl Default for Player {
    fn default() -> Self {
        Player {
            al: LazyALContext::default(),
            state: State {
                volume: 1.0,
                ..Default::default()
            },
        }
    }
}

impl Player {
    /// The song being played (or paused), 0 if none.
    pub fn song_id(&self) -> u64 {
        if self.state.stream.is_some() {
            self.state.song_id
        } else {
            0
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.state.stream.is_some()
    }

    pub fn is_paused(&self) -> bool {
        self.state.paused
    }

    /// Start playing `file` from `start_at` seconds.
    pub fn start(
        &mut self,
        manager: &mut OpenALManager,
        file: File,
        song_id: u64,
        start_at: f64,
    ) -> Result<(), String> {
        self.stop(manager);
        let mut stream = Stream::open(file)?;
        if start_at > 0.0 {
            stream.skip_seconds(start_at);
        }
        let start_frame = (start_at.max(0.0) * f64::from(stream.rate)) as u64;
        log!(
            "media: playing {:016X} ({} Hz, {} ch) from {:.1}s",
            song_id,
            stream.rate,
            stream.channels,
            start_at
        );
        let ctx = self.al.make_al_context_current(manager);
        let state = &mut self.state;
        state.stream = Some(stream);
        state.song_id = song_id;
        state.paused = false;
        state.frames_done = start_frame;
        let mut source = 0;
        unsafe {
            ctx.GenSources(1, &mut source);
            ctx.Sourcef(source, AL_GAIN, state.volume);
        }
        state.source = Some(source);
        refill(&ctx, state);
        unsafe { ctx.SourcePlay(source) };
        Ok(())
    }

    pub fn stop(&mut self, manager: &mut OpenALManager) {
        if self.state.source.is_none() && self.state.stream.is_none() {
            return;
        }
        let ctx = self.al.make_al_context_current(manager);
        let state = &mut self.state;
        if let Some(source) = state.source.take() {
            unsafe {
                ctx.SourceStop(source);
                // Stopped sources mark every buffer processed, so they can
                // all be unqueued.
                let mut queued = 0;
                ctx.GetSourcei(source, al::AL_BUFFERS_QUEUED, &mut queued);
                for _ in 0..queued {
                    let mut buffer = 0;
                    ctx.SourceUnqueueBuffers(source, 1, &mut buffer);
                    state.spare_buffers.push(buffer);
                }
                ctx.DeleteSources(1, &source);
            }
        }
        state.queued_frames.clear();
        state.frames_done = 0;
        state.stream = None;
        state.paused = false;
    }

    pub fn pause(&mut self, manager: &mut OpenALManager) {
        if let Some(source) = self.state.source {
            let ctx = self.al.make_al_context_current(manager);
            unsafe { ctx.SourcePause(source) };
            self.state.paused = true;
        }
    }

    pub fn resume(&mut self, manager: &mut OpenALManager) {
        if let Some(source) = self.state.source {
            let ctx = self.al.make_al_context_current(manager);
            unsafe { ctx.SourcePlay(source) };
            self.state.paused = false;
        }
    }

    pub fn volume(&self) -> f32 {
        self.state.volume
    }

    pub fn set_volume(&mut self, manager: &mut OpenALManager, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        self.state.volume = volume;
        if let Some(source) = self.state.source {
            let ctx = self.al.make_al_context_current(manager);
            unsafe { ctx.Sourcef(source, AL_GAIN, volume) };
        }
    }

    /// Seconds into the song that are audible now.
    pub fn position(&mut self, manager: &mut OpenALManager) -> f64 {
        let Some(rate) = self.state.stream.as_ref().map(|s| s.rate) else {
            return 0.0;
        };
        let mut offset = 0;
        if let Some(source) = self.state.source {
            let ctx = self.al.make_al_context_current(manager);
            unsafe { ctx.GetSourcei(source, AL_SAMPLE_OFFSET, &mut offset) };
        }
        (self.state.frames_done + offset.max(0) as u64) as f64 / f64::from(rate)
    }

    /// Recycle played buffers, decode more, and notice the end of the song.
    pub fn pump(&mut self, manager: &mut OpenALManager) -> PumpResult {
        let Some(source) = self.state.source else {
            return PumpResult::Idle;
        };
        let finished = {
            let ctx = self.al.make_al_context_current(manager);
            let state = &mut self.state;
            unqueue_processed(&ctx, state, source);
            if state.paused {
                return PumpResult::Paused;
            }
            refill(&ctx, state);
            let mut al_state = 0;
            unsafe { ctx.GetSourcei(source, al::AL_SOURCE_STATE, &mut al_state) };
            if al_state == al::AL_PLAYING {
                false
            } else if state.queued_frames.is_empty() {
                // Out of data and nothing left to decode.
                true
            } else {
                // The decoder fell behind and OpenAL ran dry: carry on.
                unsafe { ctx.SourcePlay(source) };
                false
            }
        };
        if finished {
            log!("media: finished {:016X}", self.state.song_id);
            self.stop(manager);
            PumpResult::Finished
        } else {
            PumpResult::Playing
        }
    }
}

fn unqueue_processed(ctx: &OpenAL<'_>, state: &mut State, source: ALuint) {
    let mut processed = 0;
    unsafe { ctx.GetSourcei(source, al::AL_BUFFERS_PROCESSED, &mut processed) };
    for _ in 0..processed.max(0) {
        let mut buffer = 0;
        unsafe { ctx.SourceUnqueueBuffers(source, 1, &mut buffer) };
        state.spare_buffers.push(buffer);
        state.frames_done += state.queued_frames.pop_front().unwrap_or(0);
    }
}

/// Top the source's queue back up to [QUEUE_LENGTH] buffers.
fn refill(ctx: &OpenAL<'_>, state: &mut State) {
    let Some(source) = state.source else {
        return;
    };
    while state.queued_frames.len() < QUEUE_LENGTH {
        let Some(stream) = state.stream.as_mut() else {
            return;
        };
        let Some(pcm) = stream.read_seconds(BUFFER_SECONDS) else {
            return;
        };
        let frames = (pcm.len() / 2 / stream.out_channels as usize) as u64;
        let format = if stream.out_channels == 1 {
            al::AL_FORMAT_MONO16
        } else {
            al::AL_FORMAT_STEREO16
        };
        let buffer = state.spare_buffers.pop().unwrap_or_else(|| {
            let mut buffer = 0;
            unsafe { ctx.GenBuffers(1, &mut buffer) };
            buffer
        });
        unsafe {
            ctx.BufferData(
                buffer,
                format,
                pcm.as_ptr() as *const ALvoid,
                pcm.len() as ALsizei,
                stream.rate as ALsizei,
            );
            ctx.SourceQueueBuffers(source, 1, &buffer);
        }
        state.queued_frames.push_back(frames);
    }
}

/// The file minus a leading ID3v2 tag. Symphonia is built without ID3
/// support, and a tag holding cover art can be big enough to hide the first
/// MP3 frame from its format probe.
struct SkipPrefix {
    file: File,
    start: u64,
    len: u64,
}

impl SkipPrefix {
    fn new(mut file: File) -> std::io::Result<SkipPrefix> {
        let total = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;
        let mut header = [0u8; 10];
        let start = if file.read_exact(&mut header).is_ok() && &header[..3] == b"ID3" {
            let size = header[6..10]
                .iter()
                .fold(0u64, |acc, &b| (acc << 7) | u64::from(b & 0x7f));
            let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
            (10 + size + footer).min(total)
        } else {
            0
        };
        file.seek(SeekFrom::Start(start))?;
        Ok(SkipPrefix {
            file,
            start,
            len: total - start,
        })
    }
}

impl Read for SkipPrefix {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for SkipPrefix {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let absolute = match pos {
            SeekFrom::Start(p) => self.file.seek(SeekFrom::Start(self.start + p))?,
            other => self.file.seek(other)?,
        };
        if absolute < self.start {
            self.file.seek(SeekFrom::Start(self.start))?;
            return Ok(0);
        }
        Ok(absolute - self.start)
    }
}

impl MediaSource for SkipPrefix {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

/// An open, decoding song.
struct Stream {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    rate: u32,
    /// Channels in the file.
    channels: u32,
    /// Channels sent to OpenAL: 1 or 2. Extra channels are dropped.
    out_channels: u32,
    /// Decoded PCM not yet handed out (16-bit, `out_channels` interleaved).
    pending: Vec<u8>,
    ended: bool,
}

impl Stream {
    fn open(file: File) -> Result<Stream, String> {
        let source = SkipPrefix::new(file).map_err(|e| e.to_string())?;
        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        let mut reader = symphonia::default::get_probe()
            .probe(
                &Default::default(),
                mss,
                Default::default(),
                Default::default(),
            )
            .map_err(|e| format!("unsupported file ({e})"))?;
        let (track_id, mut decoder) = {
            let track = reader
                .tracks()
                .iter()
                .find(|t| t.codec_params.as_ref().and_then(|p| p.audio()).is_some())
                .ok_or("no audio track")?;
            let params = track.codec_params.as_ref().unwrap().audio().unwrap();
            let decoder = symphonia::default::get_codecs()
                .make_audio_decoder(params, &Default::default())
                .map_err(|e| format!("no decoder ({e})"))?;
            (track.id, decoder)
        };

        // The sample rate and channel count are only certain once a packet
        // has been decoded, so decode the first one now.
        let mut stream_info = None;
        let mut first = Vec::new();
        while stream_info.is_none() {
            let packet = match reader.next_packet() {
                Ok(Some(packet)) => packet,
                _ => return Err("no audio in file".to_string()),
            };
            if packet.track_id() != track_id {
                continue;
            }
            let Ok(decoded) = decoder.decode(&packet) else {
                continue;
            };
            let spec = decoded.spec();
            let rate = spec.rate();
            let channels = spec.channels().count() as u32;
            decoded.copy_bytes_to_vec_interleaved_as::<i16>(&mut first);
            stream_info = Some((rate, channels));
        }
        let (rate, channels) = stream_info.unwrap();
        if rate == 0 || channels == 0 {
            return Err("bad stream format".to_string());
        }
        let mut stream = Stream {
            reader,
            decoder,
            track_id,
            rate,
            channels,
            out_channels: channels.min(2),
            pending: Vec::new(),
            ended: false,
        };
        stream.push_pcm(&first);
        Ok(stream)
    }

    /// Append interleaved 16-bit PCM with `self.channels` channels to
    /// `pending`, keeping only the first two channels.
    fn push_pcm(&mut self, pcm: &[u8]) {
        if self.channels == self.out_channels {
            self.pending.extend_from_slice(pcm);
            return;
        }
        let frame_bytes = self.channels as usize * 2;
        let keep = self.out_channels as usize * 2;
        for frame in pcm.chunks_exact(frame_bytes) {
            self.pending.extend_from_slice(&frame[..keep]);
        }
    }

    /// Decode one more packet into `pending`. False at the end of the file.
    fn decode_packet(&mut self, scratch: &mut Vec<u8>) -> bool {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                // End of file, or an I/O error we can't recover from.
                Ok(None) | Err(_) => return false,
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            // A corrupt packet is skipped rather than ending the song.
            let Ok(decoded) = self.decoder.decode(&packet) else {
                continue;
            };
            scratch.clear();
            decoded.copy_bytes_to_vec_interleaved_as::<i16>(scratch);
            let pcm = std::mem::take(scratch);
            self.push_pcm(&pcm);
            *scratch = pcm;
            return true;
        }
    }

    /// About `seconds` of PCM, or `None` once the song is used up.
    fn read_seconds(&mut self, seconds: f64) -> Option<Vec<u8>> {
        let frame_bytes = self.out_channels as usize * 2;
        let want = ((seconds * f64::from(self.rate)) as usize).max(1) * frame_bytes;
        let mut scratch = Vec::new();
        while self.pending.len() < want && !self.ended {
            if !self.decode_packet(&mut scratch) {
                self.ended = true;
            }
        }
        if self.pending.is_empty() {
            return None;
        }
        let take = want.min(self.pending.len());
        let take = take - take % frame_bytes;
        if take == 0 {
            self.pending.clear();
            return None;
        }
        Some(self.pending.drain(..take).collect())
    }

    /// Decode and throw away the first `seconds`.
    fn skip_seconds(&mut self, seconds: f64) {
        let frame_bytes = self.out_channels as usize * 2;
        let mut to_skip = (seconds * f64::from(self.rate)) as usize * frame_bytes;
        let mut scratch = Vec::new();
        while to_skip > 0 {
            if self.pending.is_empty() && (self.ended || !self.decode_packet(&mut scratch)) {
                self.ended = true;
                return;
            }
            let n = to_skip.min(self.pending.len());
            self.pending.drain(..n);
            to_skip -= n;
        }
    }
}
