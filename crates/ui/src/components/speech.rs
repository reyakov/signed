//! Dictation: a state that turns speech into text, a button that drives the
//! session, and a waveform that shows what the microphone hears.
//!
//! The application owns where the text goes: subscribe to [`SpeechEvent`] and
//! insert the final transcript where it belongs. Recognition and capture are
//! both replaceable seams; without further setup the state uses [`Microphone`]
//! and has no recognizer, so [`SpeechButton`] renders nothing until the
//! application provides one.
//!
//! # Examples
//!
//! ```no_run
//! use ui::speech::{SpeechButton, SpeechRecognizer, SpeechState};
//!
//! # use gpui::{App, AppContext as _};
//! # fn bind<R>(cx: &mut App, recognizer: R)
//! # where
//! #     R: SpeechRecognizer + 'static,
//! # {
//! // Create the state once and hold its entity in the owning view.
//! let speech = cx.new(|cx| SpeechState::new(cx).recognizer(recognizer));
//!
//! let dictation = SpeechButton::new(&speech);
//! # }
//! ```

#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, Div, ElementId, Entity, EventEmitter, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Subscription, Task, WeakEntity, Window, div, relative, rems,
};

use crate::components::common::{ControlSizing, Sizable, StyledExt as _, focus_ring, h_flex};
use crate::components::spinner::Spinner;
use crate::components::tooltip::Tooltip;
use crate::icons::{IconRole, icon};
use crate::theme::{ActiveTheme as _, DISABLED_OPACITY, SPEECH_WAVEFORM_WIDTH, Size};

/// One input-level entry per this much of captured audio.
const LEVEL_BUCKET: Duration = Duration::from_millis(80);
/// The level history a waveform draws, one entry per [`LEVEL_BUCKET`].
const MAX_LEVELS: usize = 128;
/// Amplitude below which background noise reads as silence.
const NOISE_GATE: f32 = 0.03;
/// How much of a full-scale peak maps to a full bar.
const LEVEL_SCALE: f32 = 0.5;
/// How fast a peak falls back before the next bucket's level.
const LEVEL_FALL: f32 = 0.72;
/// How long a session waits for the recognizer's final result once stopped.
const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(3);
/// The bars [`SpeechButton`] shows while capturing.
const BUTTON_BARS: usize = 4;
/// The share of the bar height a silent bar keeps, so silence reads as a dot.
const DOT_HEIGHT: f32 = 0.14;
/// The bar height reduced motion holds at.
const STILL_HEIGHT: f32 = 0.35;

/// The PCM layout a recognizer consumes: interleaved 16-bit samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    sample_rate: u32,
    channels: u8,
}

impl AudioFormat {
    /// Creates a format at the given sample rate and channel count.
    pub fn new(sample_rate: u32, channels: u8) -> Self {
        Self {
            sample_rate,
            channels: channels.max(1),
        }
    }

    /// Samples per second per channel.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The number of interleaved channels.
    pub fn channels(&self) -> u8 {
        self.channels
    }
}

impl Default for AudioFormat {
    /// 16 kHz mono, what most recognizers consume.
    fn default() -> Self {
        Self::new(16_000, 1)
    }
}

/// Why a dictation session failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeechError {
    message: SharedString,
}

impl SpeechError {
    /// A recognizer failed to start or to report a result.
    pub fn recognizer<E>(error: E) -> Self
    where
        E: std::fmt::Display,
    {
        Self::new(format!("Speech recognizer error: {error}"))
    }

    /// Audio capture failed to start or to deliver samples.
    pub fn input<E>(error: E) -> Self
    where
        E: std::fmt::Display,
    {
        Self::new(format!("Audio input error: {error}"))
    }

    fn new(message: String) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SpeechError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SpeechError {}

/// Turns captured audio into text.
pub trait SpeechRecognizer {
    /// The PCM layout [`RecognitionSession::push_audio`] delivers.
    fn audio_format(&self) -> AudioFormat {
        AudioFormat::default()
    }

    /// Whether the recognizer can run right now.
    fn is_available(&self, _cx: &App) -> bool {
        true
    }

    /// Opens a session.
    ///
    /// # Errors
    ///
    /// Errors when the recognizer cannot open a session.
    fn start(
        &self,
        sink: SpeechSink,
        cx: &mut App,
    ) -> Result<Box<dyn RecognitionSession>, SpeechError>;
}

/// A recognition session opened by [`SpeechRecognizer::start`].
pub trait RecognitionSession {
    /// Delivers interleaved 16-bit PCM in the recognizer's [`SpeechRecognizer::audio_format`].
    fn push_audio(&mut self, samples: &[i16], cx: &mut App);

    /// The user stopped talking.
    fn finish(&mut self, cx: &mut App);
}

/// Reports recognition results for the active session.
#[derive(Clone)]
pub struct SpeechSink {
    state: WeakEntity<SpeechState>,
    generation: u64,
}

impl SpeechSink {
    /// The recognizer accepts audio; the status moves to `Recording`.
    pub fn ready(&self, cx: &mut App) {
        self.defer(cx, |state, cx| state.mark_ready(cx));
    }

    /// Replaces the hypothesis of the phrase being spoken.
    pub fn hypothesis<T>(&self, text: T, cx: &mut App)
    where
        T: Into<SharedString> + 'static,
    {
        self.defer(cx, move |state, cx| {
            state.report_hypothesis(text.into(), cx)
        });
    }

    /// Commits a recognized phrase to the transcript.
    pub fn phrase<T>(&self, text: T, cx: &mut App)
    where
        T: Into<SharedString> + 'static,
    {
        self.defer(cx, move |state, cx| state.report_phrase(text.into(), cx));
    }

    /// Ends the session normally with what has been recognized so far.
    pub fn finish(&self, cx: &mut App) {
        self.defer(cx, |state, cx| state.report_finish(cx));
    }

    /// Ends the session with a failure.
    pub fn error(&self, error: SpeechError, cx: &mut App) {
        self.defer(cx, move |state, cx| state.report_error(error, cx));
    }

    fn defer<F>(&self, cx: &mut App, apply: F)
    where
        F: FnOnce(&mut SpeechState, &mut Context<SpeechState>) + 'static,
    {
        let state = self.state.clone();
        let generation = self.generation;
        cx.defer(move |cx| {
            // A report arriving after the state dropped is a normal lifecycle outcome.
            if let Some(state) = state.upgrade() {
                state.update(cx, |state, cx| state.apply(generation, cx, apply));
            }
        });
    }
}

/// Captures audio for a session.
pub trait AudioInput {
    /// Starts capture in `format`, pushing samples to `sink`; dropping the
    /// returned `Subscription` stops capture.
    ///
    /// # Errors
    ///
    /// Errors when capture cannot start; no samples are delivered.
    fn start(
        &self,
        format: AudioFormat,
        sink: AudioSink,
        cx: &mut App,
    ) -> Result<Subscription, SpeechError>;
}

/// Delivers captured audio to the active session.
#[derive(Clone)]
pub struct AudioSink {
    state: WeakEntity<SpeechState>,
    generation: u64,
}

impl AudioSink {
    /// Pushes interleaved 16-bit PCM in the recognizer's format.
    pub fn push<S>(&self, samples: S, cx: &mut App)
    where
        S: Into<Vec<i16>> + 'static,
    {
        let state = self.state.clone();
        let generation = self.generation;
        cx.defer(move |cx| {
            // Audio arriving after the state dropped is a normal lifecycle outcome.
            if let Some(state) = state.upgrade() {
                state.update(cx, |state, cx| {
                    state.receive_audio(generation, samples.into(), cx)
                });
            }
        });
    }
}

/// Where a dictation session is in its life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpeechStatus {
    /// No session is running.
    #[default]
    Idle,
    /// Audio is captured while the recognizer connects.
    Connecting,
    /// Audio is captured and recognized.
    Recording,
    /// Capture stopped; waiting for the final result.
    Stopping,
}

impl SpeechStatus {
    /// True for every status but `Idle`.
    pub fn is_active(self) -> bool {
        self != Self::Idle
    }

    /// True while audio is captured.
    pub fn is_capturing(self) -> bool {
        matches!(self, Self::Connecting | Self::Recording)
    }
}

/// What a session reports.
#[derive(Clone, Debug, PartialEq)]
pub enum SpeechEvent {
    /// A session started and audio is being captured.
    Started,
    /// The transcript changed while the user speaks.
    Partial(SharedString),
    /// The session ended normally. The text may be empty.
    Final(SharedString),
    /// The session was cancelled and its transcript discarded.
    Cancelled,
    /// The session failed and ended.
    Error(SpeechError),
}

/// Owns a dictation session.
pub struct SpeechState {
    recognizer: Option<Box<dyn SpeechRecognizer>>,
    input: Option<Box<dyn AudioInput>>,
    stop_timeout: Duration,
    format: AudioFormat,
    status: SpeechStatus,
    phrases: Vec<SharedString>,
    hypothesis: Option<SharedString>,
    levels: Vec<f32>,
    bucket: Vec<i16>,
    session: Option<Box<dyn RecognitionSession>>,
    capture: Option<Subscription>,
    generation: u64,
    stop_task: Option<Task<()>>,
}

impl EventEmitter<SpeechEvent> for SpeechState {}

impl SpeechState {
    /// Creates an idle state without a recognizer or input.
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            recognizer: None,
            input: None,
            stop_timeout: DEFAULT_STOP_TIMEOUT,
            format: AudioFormat::default(),
            status: SpeechStatus::Idle,
            phrases: Vec::new(),
            hypothesis: None,
            levels: Vec::new(),
            bucket: Vec::new(),
            session: None,
            capture: None,
            generation: 0,
            stop_task: None,
        }
    }

    /// Sets the recognizer every session runs with.
    pub fn recognizer<R>(mut self, recognizer: R) -> Self
    where
        R: SpeechRecognizer + 'static,
    {
        self.recognizer = Some(Box::new(recognizer));
        self
    }

    /// Sets the audio source every session captures with.
    pub fn input<I>(mut self, input: I) -> Self
    where
        I: AudioInput + 'static,
    {
        self.input = Some(Box::new(input));
        self
    }

    /// How long `stop` waits for the recognizer's final result.
    pub fn stop_timeout(mut self, timeout: Duration) -> Self {
        self.stop_timeout = timeout;
        self
    }

    /// Where the session is.
    pub fn status(&self) -> SpeechStatus {
        self.status
    }

    /// Whether a session can run on this platform.
    pub fn has_recognizer(&self) -> bool {
        self.recognizer.is_some() && (self.input.is_some() || Self::has_default_input())
    }

    /// Whether the recognizer reports itself ready to run.
    pub fn is_available(&self, cx: &App) -> bool {
        self.recognizer
            .as_ref()
            .is_some_and(|recognizer| recognizer.is_available(cx))
    }

    /// The whole transcript so far.
    pub fn transcript(&self) -> SharedString {
        let mut transcript = String::new();

        for phrase in &self.phrases {
            transcript.push_str(phrase);
        }

        if let Some(hypothesis) = &self.hypothesis {
            transcript.push_str(hypothesis);
        }

        transcript.into()
    }

    /// The recent input levels in `0.0..=1.0`, oldest first, one per 80 ms of audio.
    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    /// Starts a session. Does nothing while one is running.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.status.is_active() {
            return;
        }

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;

        let Some(recognizer) = self.recognizer.as_ref() else {
            self.fail("No speech recognizer is set up on this platform.", cx);
            return;
        };

        if !recognizer.is_available(cx) {
            self.fail("The speech recognizer is not available right now.", cx);
            return;
        }

        self.format = recognizer.audio_format();
        let sink = SpeechSink {
            state: cx.entity().downgrade(),
            generation,
        };

        let session = match recognizer.start(sink, cx) {
            Ok(session) => session,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };

        let audio_sink = AudioSink {
            state: cx.entity().downgrade(),
            generation,
        };

        let capture = match self.capture_input(self.format, audio_sink, cx) {
            Ok(capture) => capture,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };

        self.session = Some(session);
        self.capture = Some(capture);
        self.phrases.clear();
        self.hypothesis = None;
        self.levels.clear();
        self.bucket.clear();
        self.status = SpeechStatus::Connecting;
        cx.emit(SpeechEvent::Started);
        cx.notify();
    }

    /// Ends the session and emits `Final` once the result is in, at the latest after the stop timeout.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.status.is_capturing() {
            return;
        }
        self.capture = None;
        self.status = SpeechStatus::Stopping;
        cx.notify();

        let generation = self.generation;
        let stop_timeout = self.stop_timeout;

        if let Some(mut session) = self.session.take() {
            session.finish(cx);
        }

        self.stop_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(stop_timeout).await;
            // The state may drop while the timeout is still pending.
            this.update(cx, |state, cx| state.apply_stop_timeout(generation, cx))
                .ok();
        }));
    }

    /// Cancels the session at once and discards its transcript.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.status.is_active() {
            return;
        }
        self.capture = None;
        self.session = None;
        self.stop_task = None;
        self.reset();
        cx.emit(SpeechEvent::Cancelled);
        cx.notify();
    }

    /// `start` when idle, otherwise `stop`.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.status == SpeechStatus::Idle {
            self.start(cx);
        } else {
            self.stop(cx);
        }
    }

    fn has_default_input() -> bool {
        cfg!(not(target_arch = "wasm32"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn default_input() -> Option<Box<dyn AudioInput>> {
        Some(Box::new(Microphone))
    }

    #[cfg(target_arch = "wasm32")]
    fn default_input() -> Option<Box<dyn AudioInput>> {
        None
    }

    fn capture_input(
        &self,
        format: AudioFormat,
        sink: AudioSink,
        cx: &mut App,
    ) -> Result<Subscription, SpeechError> {
        if let Some(input) = &self.input {
            return input.start(format, sink, cx);
        }
        match Self::default_input() {
            Some(input) => input.start(format, sink, cx),
            None => Err(SpeechError::input(
                "No audio input is available on this platform; provide one with `SpeechState::input`.",
            )),
        }
    }

    fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.status = SpeechStatus::Idle;
        self.phrases.clear();
        self.hypothesis = None;
        self.levels.clear();
        self.bucket.clear();
    }

    /// Ends a failed start without ever emitting `Started`.
    fn fail<M>(&mut self, message: M, cx: &mut Context<Self>)
    where
        M: std::fmt::Display,
    {
        self.capture = None;
        self.session = None;
        self.reset();
        cx.emit(SpeechEvent::Error(SpeechError::new(message.to_string())));
        cx.notify();
    }

    /// Ends the session normally, emitting `Final` with the transcript so far.
    fn finalize(&mut self, cx: &mut Context<Self>) {
        self.capture = None;
        self.session = None;
        self.stop_task = None;
        let transcript = self.transcript();
        self.reset();
        cx.emit(SpeechEvent::Final(transcript));
        cx.notify();
    }

    fn apply<F>(&mut self, generation: u64, cx: &mut Context<Self>, apply: F)
    where
        F: FnOnce(&mut Self, &mut Context<Self>),
    {
        if self.generation != generation {
            return;
        }
        apply(self, cx);
    }

    fn mark_ready(&mut self, cx: &mut Context<Self>) {
        if self.status == SpeechStatus::Connecting {
            self.status = SpeechStatus::Recording;
            cx.notify();
        }
    }

    fn report_hypothesis(&mut self, text: SharedString, cx: &mut Context<Self>) {
        if !self.status.is_active() {
            return;
        }
        self.hypothesis = Some(text);
        cx.emit(SpeechEvent::Partial(self.transcript()));
        cx.notify();
    }

    fn report_phrase(&mut self, text: SharedString, cx: &mut Context<Self>) {
        if !self.status.is_active() {
            return;
        }
        self.phrases.push(text);
        self.hypothesis = None;
        cx.emit(SpeechEvent::Partial(self.transcript()));
        cx.notify();
    }

    fn report_finish(&mut self, cx: &mut Context<Self>) {
        if self.status.is_active() {
            self.finalize(cx);
        }
    }

    fn report_error(&mut self, error: SpeechError, cx: &mut Context<Self>) {
        if !self.status.is_active() {
            return;
        }
        self.capture = None;
        self.session = None;
        self.stop_task = None;
        self.reset();
        cx.emit(SpeechEvent::Error(error));
        cx.notify();
    }

    fn apply_stop_timeout(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.generation == generation && self.status == SpeechStatus::Stopping {
            self.finalize(cx);
        }
    }

    fn receive_audio(&mut self, generation: u64, samples: Vec<i16>, cx: &mut Context<Self>) {
        if self.generation != generation || !self.status.is_capturing() {
            return;
        }
        if let Some(session) = self.session.as_mut() {
            session.push_audio(&samples, cx);
        }
        if self.update_levels(&samples) {
            cx.notify();
        }
    }

    /// Fills one level per bucket; returns whether any level was recorded.
    fn update_levels(&mut self, samples: &[i16]) -> bool {
        let channels = self.format.channels() as usize;
        let bucket_len = ((self.format.sample_rate() as f32 * LEVEL_BUCKET.as_secs_f32()) as usize
            * channels)
            .max(channels);
        self.bucket.extend_from_slice(samples);

        let mut recorded = false;
        while self.bucket.len() >= bucket_len {
            let peak = self
                .bucket
                .drain(..bucket_len)
                .map(|sample| sample.unsigned_abs() as f32 / i16::MAX as f32)
                .fold(0.0f32, f32::max);
            let previous = self.levels.last().copied().unwrap_or(0.0);
            let level = ((peak - NOISE_GATE).max(0.0) / LEVEL_SCALE)
                .min(1.0)
                .max(previous * LEVEL_FALL);
            self.levels.push(level);
            if self.levels.len() > MAX_LEVELS {
                self.levels.remove(0);
            }
            recorded = true;
        }
        recorded
    }
}

/// Starts and stops a dictation session.
#[derive(IntoElement)]
#[must_use]
pub struct SpeechButton {
    speech: Entity<SpeechState>,
    size: Size,
    disabled: bool,
    show_when_unsupported: bool,
    style: StyleRefinement,
}

impl SpeechButton {
    /// Creates the button driving the given state.
    pub fn new(speech: &Entity<SpeechState>) -> Self {
        Self {
            speech: speech.clone(),
            size: Size::default(),
            disabled: false,
            show_when_unsupported: false,
            style: StyleRefinement::default(),
        }
    }

    /// Shows the button even when dictation is unsupported, rendered disabled.
    pub fn show_when_unsupported(mut self, show: bool) -> Self {
        self.show_when_unsupported = show;
        self
    }

    /// Disables the button for application reasons, such as a readonly field.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl gpui::Styled for SpeechButton {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for SpeechButton {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

/// The waveform bar elements for a dictation control's level history.
fn waveform_bars<'a>(
    levels: &'a [f32],
    size: Size,
    cx: &'a App,
    bar_color: impl Fn(f32) -> Hsla + 'a,
) -> impl IntoIterator<Item = Div> + 'a {
    let theme = cx.theme();
    let bar_height = move |level: f32| {
        if theme.accessibility.reduce_motion {
            STILL_HEIGHT
        } else {
            DOT_HEIGHT + (1.0 - DOT_HEIGHT) * level
        }
    };

    levels.iter().map(move |level| {
        div()
            .flex_none()
            .w(rems(size.dense_icon_rems().0 / 4.0))
            .h(relative(bar_height(*level)))
            .rounded(theme.radius.full)
            .bg(bar_color(*level))
    })
}

impl RenderOnce for SpeechButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let unsupported = !self.speech.read(cx).has_recognizer();
        if unsupported && !self.show_when_unsupported {
            return div().into_any_element();
        }

        let (status, unavailable, capturing, bars) = {
            let state = self.speech.read(cx);
            let status = state.status();
            let levels = state.levels();
            let tail_start = levels.len().saturating_sub(BUTTON_BARS);
            (
                status,
                unsupported || !state.is_available(cx),
                status.is_capturing(),
                levels[tail_start..].to_vec(),
            )
        };

        let stopping = status == SpeechStatus::Stopping;
        let disabled = self.disabled || unavailable || stopping;

        let theme = cx.theme();
        let colors = theme.colors;
        let size = self.size;
        let id: ElementId = ("speech-button", self.speech.entity_id()).into();

        let group = SharedString::from(format!(
            "speech-button-{}",
            self.speech.entity_id().as_u64()
        ));

        let label: SharedString = if unavailable {
            "Dictation is unavailable".into()
        } else if capturing {
            "Stop dictation".into()
        } else {
            "Start dictation".into()
        };

        let foreground = colors.foreground;
        let bar_color = |_| theme.ramps.primary.base;

        h_flex()
            .id(id.clone())
            .group(group.clone())
            .role(Role::Button)
            .justify_center()
            .control_height(size)
            .control_dense_padding_x(size)
            .gap_0p5()
            .rounded(theme.radius.control)
            .text_color(foreground)
            .when(capturing, |button| button.bg(colors.muted))
            .refine_style(&self.style)
            .when(disabled, |button| button.opacity(DISABLED_OPACITY))
            .aria_label(label.clone())
            .tooltip(Tooltip::text(label))
            .child(if stopping {
                Spinner::new(("speech-spinner", self.speech.entity_id()))
                    .with_size(self.size)
                    .color(foreground)
                    .into_any_element()
            } else if capturing {
                h_flex()
                    .id(("speech-bars", self.speech.entity_id()))
                    .h(size.icon_rems())
                    .items_end()
                    .gap_0p5()
                    .children(waveform_bars(&bars, size, cx, bar_color))
                    .group_hover(group.clone(), |style| style.invisible())
                    .into_any_element()
            } else {
                icon(IconRole::Microphone)
                    .size(size.icon_rems())
                    .color(foreground)
                    .into_any_element()
            })
            .when(capturing, |button| {
                button.child(
                    div()
                        .invisible()
                        .group_hover(group, |style| style.visible())
                        .child(
                            icon(IconRole::Stop)
                                .size(size.icon_rems())
                                .color(foreground),
                        ),
                )
            })
            .when(!disabled, |button| {
                let speech = self.speech.clone();
                button
                    .cursor_pointer()
                    .focusable()
                    .tab_stop(true)
                    .hover(move |style| style.bg(colors.muted_hover))
                    .active(move |style| style.bg(theme.ramps.secondary.pressed))
                    .focus_visible(move |style| style.shadow(focus_ring(colors.ring)))
                    .on_click(move |_, _, cx| speech.update(cx, |state, cx| state.toggle(cx)))
            })
            .into_any_element()
    }
}

/// A live waveform of what a dictation session hears.
#[derive(IntoElement)]
#[must_use]
pub struct SpeechWaveform {
    speech: Entity<SpeechState>,
    size: Size,
    style: gpui::StyleRefinement,
}

impl SpeechWaveform {
    /// Creates the waveform drawn from the given state.
    pub fn new(speech: &Entity<SpeechState>) -> Self {
        Self {
            speech: speech.clone(),
            size: Size::default(),
            style: gpui::StyleRefinement::default(),
        }
    }
}

impl Sizable for SpeechWaveform {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl Styled for SpeechWaveform {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for SpeechWaveform {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.speech.read(cx).status().is_capturing() {
            return div().into_any_element();
        }
        let levels = {
            let state = self.speech.read(cx);
            state.levels().to_vec()
        };

        let theme = cx.theme();
        let colors = theme.colors;
        let size = self.size;
        let bar_color = |level: f32| {
            if level <= 0.0 {
                colors.muted_foreground
            } else {
                theme.ramps.primary.base
            }
        };

        h_flex()
            .id(("speech-waveform", self.speech.entity_id()))
            .items_end()
            .justify_end()
            .gap_0p5()
            .h(size.icon_rems())
            .w(SPEECH_WAVEFORM_WIDTH)
            .overflow_hidden()
            .children(waveform_bars(&levels, size, cx, bar_color))
            .refine_style(&self.style)
            .into_any_element()
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// The built-in input.
pub struct Microphone;

#[cfg(not(target_arch = "wasm32"))]
impl AudioInput for Microphone {
    fn start(
        &self,
        format: AudioFormat,
        sink: AudioSink,
        cx: &mut App,
    ) -> Result<Subscription, SpeechError> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| SpeechError::input("No microphone is connected."))?;

        let config = device.default_input_config().map_err(SpeechError::input)?;
        let stream_config = cpal::StreamConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };
        let device_channels = stream_config.channels.max(1) as usize;

        let (sender, receiver) = flume::unbounded::<Result<Vec<i16>, String>>();
        let converter = Arc::new(Mutex::new(Resampler::new(
            config.sample_rate() as f32 / format.sample_rate().max(1) as f32,
        )));
        let error_handler = {
            let sender = sender.clone();
            move |error: cpal::Error| {
                log::warn!("microphone stream error: {error}");
                if sender.send(Err(error.to_string())).is_err() {
                    log::debug!("speech session closed before its stream error arrived");
                }
            }
        };

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                stream_config,
                Microphone::callback(
                    |sample: f32| sample,
                    device_channels,
                    format,
                    converter.clone(),
                    sender.clone(),
                ),
                error_handler,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                stream_config,
                Microphone::callback(
                    |sample: i16| sample as f32 / i16::MAX as f32,
                    device_channels,
                    format,
                    converter.clone(),
                    sender.clone(),
                ),
                error_handler,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                stream_config,
                Microphone::callback(
                    |sample: u16| (sample as f32 - u16::MAX as f32 / 2.0) / (u16::MAX as f32 / 2.0),
                    device_channels,
                    format,
                    converter.clone(),
                    sender.clone(),
                ),
                error_handler,
                None,
            ),
            sample_format => {
                return Err(SpeechError::input(format!(
                    "Unsupported microphone sample format: {sample_format}"
                )));
            }
        }
        .map_err(SpeechError::input)?;

        stream.play().map_err(SpeechError::input)?;

        let task = cx.spawn(async move |cx| {
            while let Ok(message) = receiver.recv_async().await {
                match message {
                    Ok(samples) => cx.update(|cx| sink.push(samples, cx)),
                    Err(message) => {
                        let error = SpeechError::input(message);
                        let state = sink.state.clone();
                        let generation = sink.generation;
                        cx.update(move |cx| {
                            if let Some(state) = state.upgrade() {
                                state.update(cx, |state, cx| {
                                    state.apply(generation, cx, |state, cx| {
                                        state.report_error(error, cx)
                                    })
                                });
                            }
                        });
                    }
                }
            }
        });

        Ok(Subscription::new(move || {
            drop(stream);
            drop(task);
        }))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Microphone {
    fn callback<T>(
        to_sample: fn(T) -> f32,
        device_channels: usize,
        format: AudioFormat,
        converter: Arc<Mutex<Resampler>>,
        sender: flume::Sender<Result<Vec<i16>, String>>,
    ) -> impl FnMut(&[T], &cpal::InputCallbackInfo) + Send + 'static
    where
        T: Copy + 'static,
    {
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            // A panic while resampling poisons the lock; the phase is
            // approximate anyway, so recover and keep the stream alive.
            let mut converter = match converter.lock() {
                Ok(converter) => converter,
                Err(poisoned) => {
                    log::warn!("microphone resampler lock was poisoned; recovering");
                    poisoned.into_inner()
                }
            };

            let mut mono = Vec::with_capacity(data.len() / device_channels + 1);
            for frame in data.chunks(device_channels) {
                let total: f32 = frame.iter().map(|sample| to_sample(*sample)).sum();
                mono.push(total / frame.len() as f32);
            }

            let mut resampled = Vec::with_capacity(mono.len());
            converter.resample(&mono, &mut resampled);

            let channels = format.channels() as usize;
            let mut samples = Vec::with_capacity(resampled.len() * channels);

            for value in resampled {
                let sample = (value * i16::MAX as f32)
                    .round()
                    .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                for _ in 0..channels {
                    samples.push(sample);
                }
            }

            if sender.send(Ok(samples)).is_err() {
                log::debug!("speech session closed before its audio arrived");
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// A minimal linear resampler that carries its phase across callbacks.
struct Resampler {
    step: f32,
    phase: f32,
    last: Option<f32>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Resampler {
    fn new(step: f32) -> Self {
        Self {
            step,
            phase: 0.0,
            last: None,
        }
    }

    fn resample(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        let mut phase = self.phase;
        while phase < input.len() as f32 {
            let index = (phase as usize).min(input.len() - 1);
            let fraction = phase - index as f32;
            let previous = if index == 0 {
                self.last.unwrap_or(input[0])
            } else {
                input[index - 1]
            };
            output.push(previous + (input[index] - previous) * fraction);
            phase += self.step;
        }
        self.phase = phase - input.len() as f32;
        self.last = Some(input[input.len() - 1]);
    }
}
