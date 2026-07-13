use crate::{BridgeError, BridgeResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DspMode {
    Simple,
    Advanced,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EqualizerBandType {
    NotSet,
    LowPass,
    HighPass,
    Notch,
    Bell,
    LowShelf,
    HighShelf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EqualizerBandState {
    pub band: u8,
    pub band_type: EqualizerBandType,
    pub gain_db: f32,
    pub frequency_hz: f32,
    pub q: f32,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EqualizerProfile {
    pub mode: DspMode,
    pub bands: Vec<EqualizerBandState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EqualizerState {
    pub active_mode: DspMode,
    pub simple: EqualizerProfile,
    pub advanced: EqualizerProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompressorProfile {
    pub mode: DspMode,
    pub enabled: bool,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_gain_db: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompressorState {
    pub active_mode: DspMode,
    pub simple: CompressorProfile,
    pub advanced: CompressorProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpanderProfile {
    pub mode: DspMode,
    pub enabled: bool,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpanderState {
    pub active_mode: DspMode,
    pub simple: ExpanderProfile,
    pub advanced: ExpanderProfile,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoiseSuppressionStyle {
    Off,
    Adaptive,
    Snapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoiseSuppressionState {
    pub enabled: bool,
    pub style: NoiseSuppressionStyle,
    pub amount_percent: f32,
    pub sensitivity_db: f32,
    pub adapt_time_ms: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BassEnhancementState {
    pub enabled: bool,
    pub preset: u8,
    pub amount: f32,
    pub drive: f32,
    pub mix_percent: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub threshold_db: f32,
    pub knee: f32,
    pub makeup_gain_db: f32,
    pub ratio: f32,
    pub cutoff_hz: f32,
    pub q: f32,
    pub lower_cutoff_hz: f32,
    pub lower_q: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeEsserState {
    pub enabled: bool,
    pub amount_percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExciterState {
    pub enabled: bool,
    pub amount_percent: f32,
    pub frequency_hz: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnhancementSuiteState {
    pub bass: BassEnhancementState,
    pub de_esser: DeEsserState,
    pub exciter: ExciterState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HeadphoneEqBand {
    Bass,
    Mids,
    Treble,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeadphoneEqBandState {
    pub band: HeadphoneEqBand,
    pub enabled: bool,
    pub amount_db: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubwooferState {
    pub enabled: bool,
    pub amount: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeadphoneEqualizerState {
    pub bands: Vec<HeadphoneEqBandState>,
    #[serde(default)]
    pub subwoofer: SubwooferState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MicrophoneDspSnapshot {
    pub equalizer: EqualizerState,
    pub compressor: CompressorState,
    pub expander: ExpanderState,
    pub noise_suppression: NoiseSuppressionState,
    pub enhancement_suite: EnhancementSuiteState,
    pub headphone_equalizer: HeadphoneEqualizerState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DspWriteModule {
    Equalizer,
    Compressor,
    Expander,
    NoiseSuppression,
    EnhancementSuite,
    HeadphoneEqualizer,
}

impl DspWriteModule {
    pub const ALL: [Self; 6] = [
        Self::Equalizer,
        Self::Compressor,
        Self::Expander,
        Self::NoiseSuppression,
        Self::EnhancementSuite,
        Self::HeadphoneEqualizer,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Equalizer => "equalizer",
            Self::Compressor => "compressor",
            Self::Expander => "expander",
            Self::NoiseSuppression => "noise_suppression",
            Self::EnhancementSuite => "enhancement_suite",
            Self::HeadphoneEqualizer => "headphone_equalizer",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "module", content = "state", rename_all = "snake_case")]
pub enum MicrophoneDspUpdate {
    Equalizer(EqualizerState),
    Compressor(CompressorState),
    Expander(ExpanderState),
    NoiseSuppression(NoiseSuppressionState),
    EnhancementSuite(EnhancementSuiteState),
    HeadphoneEqualizer(HeadphoneEqualizerState),
}

impl MicrophoneDspUpdate {
    pub const fn module(&self) -> DspWriteModule {
        match self {
            Self::Equalizer(_) => DspWriteModule::Equalizer,
            Self::Compressor(_) => DspWriteModule::Compressor,
            Self::Expander(_) => DspWriteModule::Expander,
            Self::NoiseSuppression(_) => DspWriteModule::NoiseSuppression,
            Self::EnhancementSuite(_) => DspWriteModule::EnhancementSuite,
            Self::HeadphoneEqualizer(_) => DspWriteModule::HeadphoneEqualizer,
        }
    }

    pub fn validate(&self) -> BridgeResult<()> {
        match self {
            Self::Equalizer(state) => validate_equalizer(state),
            Self::Compressor(state) => validate_compressor(state),
            Self::Expander(state) => validate_expander(state),
            Self::NoiseSuppression(state) => validate_noise_suppression(state),
            Self::EnhancementSuite(state) => validate_enhancement_suite(state),
            Self::HeadphoneEqualizer(state) => validate_headphone_equalizer(state),
        }
    }

    pub fn matches_snapshot(&self, snapshot: &MicrophoneDspSnapshot) -> bool {
        match self {
            Self::Equalizer(expected) => eq_state_close(expected, &snapshot.equalizer),
            Self::Compressor(expected) => compressor_state_close(expected, &snapshot.compressor),
            Self::Expander(expected) => expander_state_close(expected, &snapshot.expander),
            Self::NoiseSuppression(expected) => {
                noise_state_close(expected, &snapshot.noise_suppression)
            }
            Self::EnhancementSuite(expected) => {
                enhancement_state_close(expected, &snapshot.enhancement_suite)
            }
            Self::HeadphoneEqualizer(expected) => {
                headphone_eq_close(expected, &snapshot.headphone_equalizer)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MicrophoneDspWriteResult {
    pub module: DspWriteModule,
    pub verified: bool,
    pub snapshot: MicrophoneDspSnapshot,
}

fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidValue(message.into())
}

fn in_range(label: &str, value: f32, minimum: f32, maximum: f32) -> BridgeResult<()> {
    if value.is_finite() && (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(invalid(format!(
            "{label} must be finite and between {minimum} and {maximum}"
        )))
    }
}

fn validate_equalizer(state: &EqualizerState) -> BridgeResult<()> {
    for (expected_mode, profile) in [
        (DspMode::Simple, &state.simple),
        (DspMode::Advanced, &state.advanced),
    ] {
        if profile.mode != expected_mode {
            return Err(invalid("equalizer profile mode does not match its slot"));
        }
        if profile.bands.len() != 8 {
            return Err(invalid(
                "equalizer profiles must contain exactly eight bands",
            ));
        }
        for (index, band) in profile.bands.iter().enumerate() {
            if band.band != index as u8 + 1 {
                return Err(invalid("equalizer bands must be ordered 1 through 8"));
            }
            in_range("equalizer gain", band.gain_db, -12.0, 12.0)?;
            in_range("equalizer frequency", band.frequency_hz, 20.0, 20_000.0)?;
            in_range("equalizer Q", band.q, 0.1, 10.0)?;
        }
    }
    Ok(())
}

fn validate_compressor(state: &CompressorState) -> BridgeResult<()> {
    for (expected_mode, profile) in [
        (DspMode::Simple, &state.simple),
        (DspMode::Advanced, &state.advanced),
    ] {
        if profile.mode != expected_mode {
            return Err(invalid("compressor profile mode does not match its slot"));
        }
        in_range("compressor threshold", profile.threshold_db, -50.0, 0.0)?;
        in_range("compressor ratio", profile.ratio, 1.0, 16.0)?;
        in_range("compressor attack", profile.attack_ms, 1.0, 2_000.0)?;
        in_range("compressor release", profile.release_ms, 1.0, 2_000.0)?;
        in_range("compressor make-up gain", profile.makeup_gain_db, 0.0, 12.0)?;
    }
    Ok(())
}

fn validate_expander(state: &ExpanderState) -> BridgeResult<()> {
    for (expected_mode, profile) in [
        (DspMode::Simple, &state.simple),
        (DspMode::Advanced, &state.advanced),
    ] {
        if profile.mode != expected_mode {
            return Err(invalid("expander profile mode does not match its slot"));
        }
        in_range("expander threshold", profile.threshold_db, -90.0, 0.0)?;
        in_range("expander ratio", profile.ratio, 1.0, 10.0)?;
        in_range("expander attack", profile.attack_ms, 1.0, 2_000.0)?;
        in_range("expander release", profile.release_ms, 1.0, 2_000.0)?;
    }
    Ok(())
}

fn validate_noise_suppression(state: &NoiseSuppressionState) -> BridgeResult<()> {
    in_range("noise suppression amount", state.amount_percent, 0.0, 100.0)?;
    in_range(
        "noise suppression sensitivity",
        state.sensitivity_db,
        -120.0,
        -60.0,
    )?;
    in_range(
        "noise suppression adaptation time",
        state.adapt_time_ms,
        100.0,
        5_000.0,
    )
}

fn validate_enhancement_suite(state: &EnhancementSuiteState) -> BridgeResult<()> {
    let bass = &state.bass;
    if !(1..=4).contains(&bass.preset) {
        return Err(invalid("bass enhancement preset must be between 1 and 4"));
    }
    in_range("bass amount", bass.amount, 0.0, 10.0)?;
    in_range("bass drive", bass.drive, 0.0, 32.0)?;
    in_range("bass mix", bass.mix_percent, 0.0, 100.0)?;
    in_range("bass attack", bass.attack_ms, 1.0, 2_000.0)?;
    in_range("bass release", bass.release_ms, 1.0, 2_000.0)?;
    in_range("bass threshold", bass.threshold_db, -50.0, 0.0)?;
    in_range("bass knee", bass.knee, 0.0, 5.0)?;
    in_range("bass make-up gain", bass.makeup_gain_db, 0.0, 12.0)?;
    in_range("bass ratio", bass.ratio, 0.0, 16.0)?;
    in_range("bass cutoff", bass.cutoff_hz, 0.0, 160.0)?;
    in_range("bass Q", bass.q, 0.0, 16.0)?;
    in_range("bass lower cutoff", bass.lower_cutoff_hz, 0.0, 160.0)?;
    in_range("bass lower Q", bass.lower_q, 0.0, 16.0)?;
    in_range("de-esser amount", state.de_esser.amount_percent, 0.0, 100.0)?;
    in_range("exciter amount", state.exciter.amount_percent, 0.0, 100.0)?;
    in_range(
        "exciter frequency",
        state.exciter.frequency_hz,
        0.0,
        5_000.0,
    )
}

fn validate_headphone_equalizer(state: &HeadphoneEqualizerState) -> BridgeResult<()> {
    if state.bands.len() != 3 {
        return Err(invalid(
            "headphone equalizer must contain bass, mids, and treble",
        ));
    }
    for (expected, band) in [
        HeadphoneEqBand::Bass,
        HeadphoneEqBand::Mids,
        HeadphoneEqBand::Treble,
    ]
    .into_iter()
    .zip(&state.bands)
    {
        if band.band != expected {
            return Err(invalid(
                "headphone EQ bands must be ordered bass, mids, treble",
            ));
        }
        in_range("headphone EQ amount", band.amount_db, -12.0, 12.0)?;
    }
    if state.subwoofer.amount > 10 {
        return Err(invalid("subwoofer amount must be between 0 and 10"));
    }
    Ok(())
}

fn close(left: f32, right: f32) -> bool {
    (left - right).abs() <= 0.01
}

fn eq_state_close(left: &EqualizerState, right: &EqualizerState) -> bool {
    left.active_mode == right.active_mode
        && [&left.simple, &left.advanced]
            .into_iter()
            .zip([&right.simple, &right.advanced])
            .all(|(left, right)| {
                left.mode == right.mode
                    && left.bands.len() == right.bands.len()
                    && left.bands.iter().zip(&right.bands).all(|(left, right)| {
                        left.band == right.band
                            && left.band_type == right.band_type
                            && left.enabled == right.enabled
                            && close(left.gain_db, right.gain_db)
                            && close(left.frequency_hz, right.frequency_hz)
                            && close(left.q, right.q)
                    })
            })
}

fn compressor_state_close(left: &CompressorState, right: &CompressorState) -> bool {
    left.active_mode == right.active_mode
        && [&left.simple, &left.advanced]
            .into_iter()
            .zip([&right.simple, &right.advanced])
            .all(|(left, right)| {
                left.mode == right.mode
                    && left.enabled == right.enabled
                    && close(left.threshold_db, right.threshold_db)
                    && close(left.ratio, right.ratio)
                    && close(left.attack_ms, right.attack_ms)
                    && close(left.release_ms, right.release_ms)
                    && close(left.makeup_gain_db, right.makeup_gain_db)
            })
}

fn expander_state_close(left: &ExpanderState, right: &ExpanderState) -> bool {
    left.active_mode == right.active_mode
        && [&left.simple, &left.advanced]
            .into_iter()
            .zip([&right.simple, &right.advanced])
            .all(|(left, right)| {
                left.mode == right.mode
                    && left.enabled == right.enabled
                    && close(left.threshold_db, right.threshold_db)
                    && close(left.ratio, right.ratio)
                    && close(left.attack_ms, right.attack_ms)
                    && close(left.release_ms, right.release_ms)
            })
}

fn noise_state_close(left: &NoiseSuppressionState, right: &NoiseSuppressionState) -> bool {
    left.enabled == right.enabled
        && left.style == right.style
        && close(left.amount_percent, right.amount_percent)
        && close(left.sensitivity_db, right.sensitivity_db)
        && close(left.adapt_time_ms, right.adapt_time_ms)
}

fn enhancement_state_close(left: &EnhancementSuiteState, right: &EnhancementSuiteState) -> bool {
    let left_bass = &left.bass;
    let right_bass = &right.bass;
    left_bass.enabled == right_bass.enabled
        && left_bass.preset == right_bass.preset
        && close(left_bass.amount, right_bass.amount)
        && close(left_bass.drive, right_bass.drive)
        && close(left_bass.mix_percent, right_bass.mix_percent)
        && close(left_bass.attack_ms, right_bass.attack_ms)
        && close(left_bass.release_ms, right_bass.release_ms)
        && close(left_bass.threshold_db, right_bass.threshold_db)
        && close(left_bass.knee, right_bass.knee)
        && close(left_bass.makeup_gain_db, right_bass.makeup_gain_db)
        && close(left_bass.ratio, right_bass.ratio)
        && close(left_bass.cutoff_hz, right_bass.cutoff_hz)
        && close(left_bass.q, right_bass.q)
        && close(left_bass.lower_cutoff_hz, right_bass.lower_cutoff_hz)
        && close(left_bass.lower_q, right_bass.lower_q)
        && left.de_esser.enabled == right.de_esser.enabled
        && close(left.de_esser.amount_percent, right.de_esser.amount_percent)
        && left.exciter.enabled == right.exciter.enabled
        && close(left.exciter.amount_percent, right.exciter.amount_percent)
        && close(left.exciter.frequency_hz, right.exciter.frequency_hz)
}

fn headphone_eq_close(left: &HeadphoneEqualizerState, right: &HeadphoneEqualizerState) -> bool {
    left.subwoofer == right.subwoofer
        && left.bands.len() == right.bands.len()
        && left.bands.iter().zip(&right.bands).all(|(left, right)| {
            left.band == right.band
                && left.enabled == right.enabled
                && close(left.amount_db, right.amount_db)
        })
}
