use beacn_lib::audio::BeacnAudioDevice;
use beacn_lib::audio::messages::Message;
use beacn_lib::audio::messages::bass_enhancement::{
    BassAmount, BassCutoff, BassDrive, BassEnhancement, BassKnee, BassPreset, BassQ, BassRatio,
    BassThreshold,
};
use beacn_lib::audio::messages::compressor::{
    Compressor, CompressorMode, CompressorRatio, CompressorThreshold,
};
use beacn_lib::audio::messages::deesser::DeEsser;
use beacn_lib::audio::messages::equaliser::{
    EQBand, EQBandType, EQFrequency, EQGain, EQMode, EQQ, Equaliser,
};
use beacn_lib::audio::messages::exciter::{Exciter, ExciterFreq};
use beacn_lib::audio::messages::expander::{
    Expander, ExpanderMode, ExpanderRatio, ExpanderThreshold,
};
use beacn_lib::audio::messages::headphone_equaliser::{HPEQType, HPEQValue, HeadphoneEQ};
use beacn_lib::audio::messages::subwoofer::Subwoofer;
use beacn_lib::audio::messages::suppressor::{
    Suppressor, SuppressorSensitivity, SuppressorStyle, SupressorAdaptTime,
};
use beacn_lib::types::{MakeUpGain, Percent, TimeFrame};
use studiobridge_core::{
    BassEnhancementState, BridgeError, BridgeResult, CompressorProfile, CompressorState,
    DeEsserState, DspMode, EnhancementSuiteState, EqualizerBandState, EqualizerBandType,
    EqualizerProfile, EqualizerState, ExciterState, ExpanderProfile, ExpanderState,
    HeadphoneEqBand, HeadphoneEqBandState, HeadphoneEqualizerState, MicrophoneDspSnapshot,
    MicrophoneDspUpdate, NoiseSuppressionState, NoiseSuppressionStyle, SubwooferState,
};

pub(crate) fn read_microphone_dsp(
    device: &dyn BeacnAudioDevice,
) -> BridgeResult<MicrophoneDspSnapshot> {
    Ok(MicrophoneDspSnapshot {
        equalizer: read_equalizer(device)?,
        compressor: read_compressor(device)?,
        expander: read_expander(device)?,
        noise_suppression: read_noise_suppression(device)?,
        enhancement_suite: read_enhancement_suite(device)?,
        headphone_equalizer: read_headphone_equalizer(device)?,
    })
}

fn read_equalizer(device: &dyn BeacnAudioDevice) -> BridgeResult<EqualizerState> {
    let active_mode = match execute(device, Message::Equaliser(Equaliser::GetMode))? {
        Message::Equaliser(Equaliser::Mode(mode)) => eq_mode(mode),
        response => return Err(unexpected("equalizer mode", response)),
    };
    Ok(EqualizerState {
        active_mode,
        simple: read_equalizer_profile(device, EQMode::Simple)?,
        advanced: read_equalizer_profile(device, EQMode::Advanced)?,
    })
}

fn read_equalizer_profile(
    device: &dyn BeacnAudioDevice,
    mode: EQMode,
) -> BridgeResult<EqualizerProfile> {
    let bands = [
        (1, EQBand::Band1),
        (2, EQBand::Band2),
        (3, EQBand::Band3),
        (4, EQBand::Band4),
        (5, EQBand::Band5),
        (6, EQBand::Band6),
        (7, EQBand::Band7),
        (8, EQBand::Band8),
    ]
    .into_iter()
    .map(|(number, band)| read_equalizer_band(device, mode, band, number))
    .collect::<BridgeResult<Vec<_>>>()?;
    Ok(EqualizerProfile {
        mode: eq_mode(mode),
        bands,
    })
}

fn read_equalizer_band(
    device: &dyn BeacnAudioDevice,
    mode: EQMode,
    band: EQBand,
    number: u8,
) -> BridgeResult<EqualizerBandState> {
    let band_type = match execute(device, Message::Equaliser(Equaliser::GetType(mode, band)))? {
        Message::Equaliser(Equaliser::Type(m, b, value)) if m == mode && b == band => {
            eq_band_type(value)
        }
        response => return Err(unexpected("equalizer band type", response)),
    };
    let gain_db = match execute(device, Message::Equaliser(Equaliser::GetGain(mode, band)))? {
        Message::Equaliser(Equaliser::Gain(m, b, value)) if m == mode && b == band => value.0,
        response => return Err(unexpected("equalizer gain", response)),
    };
    let frequency_hz = match execute(
        device,
        Message::Equaliser(Equaliser::GetFrequency(mode, band)),
    )? {
        Message::Equaliser(Equaliser::Frequency(m, b, value)) if m == mode && b == band => value.0,
        response => return Err(unexpected("equalizer frequency", response)),
    };
    let q = match execute(device, Message::Equaliser(Equaliser::GetQ(mode, band)))? {
        Message::Equaliser(Equaliser::Q(m, b, value)) if m == mode && b == band => value.0,
        response => return Err(unexpected("equalizer Q", response)),
    };
    let enabled = match execute(
        device,
        Message::Equaliser(Equaliser::GetEnabled(mode, band)),
    )? {
        Message::Equaliser(Equaliser::Enabled(m, b, value)) if m == mode && b == band => value,
        response => return Err(unexpected("equalizer enabled state", response)),
    };
    Ok(EqualizerBandState {
        band: number,
        band_type,
        gain_db,
        frequency_hz,
        q,
        enabled,
    })
}

fn read_compressor(device: &dyn BeacnAudioDevice) -> BridgeResult<CompressorState> {
    let active_mode = match execute(device, Message::Compressor(Compressor::GetMode))? {
        Message::Compressor(Compressor::Mode(mode)) => compressor_mode(mode),
        response => return Err(unexpected("compressor mode", response)),
    };
    Ok(CompressorState {
        active_mode,
        simple: read_compressor_profile(device, CompressorMode::Simple)?,
        advanced: read_compressor_profile(device, CompressorMode::Advanced)?,
    })
}

fn read_compressor_profile(
    device: &dyn BeacnAudioDevice,
    mode: CompressorMode,
) -> BridgeResult<CompressorProfile> {
    let attack_ms = match execute(device, Message::Compressor(Compressor::GetAttack(mode)))? {
        Message::Compressor(Compressor::Attack(m, value)) if m == mode => value.0,
        response => return Err(unexpected("compressor attack", response)),
    };
    let release_ms = match execute(device, Message::Compressor(Compressor::GetRelease(mode)))? {
        Message::Compressor(Compressor::Release(m, value)) if m == mode => value.0,
        response => return Err(unexpected("compressor release", response)),
    };
    let threshold_db = match execute(device, Message::Compressor(Compressor::GetThreshold(mode)))? {
        Message::Compressor(Compressor::Threshold(m, value)) if m == mode => value.0,
        response => return Err(unexpected("compressor threshold", response)),
    };
    let ratio = match execute(device, Message::Compressor(Compressor::GetRatio(mode)))? {
        Message::Compressor(Compressor::Ratio(m, value)) if m == mode => value.0,
        response => return Err(unexpected("compressor ratio", response)),
    };
    let makeup_gain_db =
        match execute(device, Message::Compressor(Compressor::GetMakeupGain(mode)))? {
            Message::Compressor(Compressor::MakeupGain(m, value)) if m == mode => value.0,
            response => return Err(unexpected("compressor makeup gain", response)),
        };
    let enabled = match execute(device, Message::Compressor(Compressor::GetEnabled(mode)))? {
        Message::Compressor(Compressor::Enabled(m, value)) if m == mode => value,
        response => return Err(unexpected("compressor enabled state", response)),
    };
    Ok(CompressorProfile {
        mode: compressor_mode(mode),
        enabled,
        threshold_db,
        ratio,
        attack_ms,
        release_ms,
        makeup_gain_db,
    })
}

fn read_expander(device: &dyn BeacnAudioDevice) -> BridgeResult<ExpanderState> {
    let active_mode = match execute(device, Message::Expander(Expander::GetMode))? {
        Message::Expander(Expander::Mode(mode)) => expander_mode(mode),
        response => return Err(unexpected("expander mode", response)),
    };
    Ok(ExpanderState {
        active_mode,
        simple: read_expander_profile(device, ExpanderMode::Simple)?,
        advanced: read_expander_profile(device, ExpanderMode::Advanced)?,
    })
}

fn read_expander_profile(
    device: &dyn BeacnAudioDevice,
    mode: ExpanderMode,
) -> BridgeResult<ExpanderProfile> {
    let threshold_db = match execute(device, Message::Expander(Expander::GetThreshold(mode)))? {
        Message::Expander(Expander::Threshold(m, value)) if m == mode => value.0,
        response => return Err(unexpected("expander threshold", response)),
    };
    let ratio = match execute(device, Message::Expander(Expander::GetRatio(mode)))? {
        Message::Expander(Expander::Ratio(m, value)) if m == mode => value.0,
        response => return Err(unexpected("expander ratio", response)),
    };
    let enabled = match execute(device, Message::Expander(Expander::GetEnabled(mode)))? {
        Message::Expander(Expander::Enabled(m, value)) if m == mode => value,
        response => return Err(unexpected("expander enabled state", response)),
    };
    let attack_ms = match execute(device, Message::Expander(Expander::GetAttack(mode)))? {
        Message::Expander(Expander::Attack(m, value)) if m == mode => value.0,
        response => return Err(unexpected("expander attack", response)),
    };
    let release_ms = match execute(device, Message::Expander(Expander::GetRelease(mode)))? {
        Message::Expander(Expander::Release(m, value)) if m == mode => value.0,
        response => return Err(unexpected("expander release", response)),
    };
    Ok(ExpanderProfile {
        mode: expander_mode(mode),
        enabled,
        threshold_db,
        ratio,
        attack_ms,
        release_ms,
    })
}

fn read_noise_suppression(device: &dyn BeacnAudioDevice) -> BridgeResult<NoiseSuppressionState> {
    let enabled = match execute(device, Message::Suppressor(Suppressor::GetEnabled))? {
        Message::Suppressor(Suppressor::Enabled(value)) => value,
        response => return Err(unexpected("noise suppression enabled state", response)),
    };
    let amount_percent = match execute(device, Message::Suppressor(Suppressor::GetAmount))? {
        Message::Suppressor(Suppressor::Amount(value)) => value.0,
        response => return Err(unexpected("noise suppression amount", response)),
    };
    let style = match execute(device, Message::Suppressor(Suppressor::GetStyle))? {
        Message::Suppressor(Suppressor::Style(value)) => suppression_style(value),
        response => return Err(unexpected("noise suppression style", response)),
    };
    let sensitivity_db = match execute(device, Message::Suppressor(Suppressor::GetSensitivity))? {
        Message::Suppressor(Suppressor::Sensitivity(value)) => value.0,
        response => return Err(unexpected("noise suppression sensitivity", response)),
    };
    let adapt_time_ms = match execute(device, Message::Suppressor(Suppressor::GetAdaptTime))? {
        Message::Suppressor(Suppressor::AdaptTime(value)) => value.0,
        response => return Err(unexpected("noise suppression adaptation time", response)),
    };
    Ok(NoiseSuppressionState {
        enabled,
        style,
        amount_percent,
        sensitivity_db,
        adapt_time_ms,
    })
}

fn read_enhancement_suite(device: &dyn BeacnAudioDevice) -> BridgeResult<EnhancementSuiteState> {
    Ok(EnhancementSuiteState {
        bass: read_bass_enhancement(device)?,
        de_esser: read_de_esser(device)?,
        exciter: read_exciter(device)?,
    })
}

fn read_bass_enhancement(device: &dyn BeacnAudioDevice) -> BridgeResult<BassEnhancementState> {
    macro_rules! read_bass {
        ($request:expr, $variant:path, $label:literal) => {
            match execute(device, Message::BassEnhancement($request))? {
                Message::BassEnhancement($variant(value)) => value.0,
                response => return Err(unexpected($label, response)),
            }
        };
    }
    let enabled = match execute(
        device,
        Message::BassEnhancement(BassEnhancement::GetEnabled),
    )? {
        Message::BassEnhancement(BassEnhancement::Enabled(value)) => value,
        response => return Err(unexpected("bass enhancement enabled state", response)),
    };
    let preset = match execute(device, Message::BassEnhancement(BassEnhancement::GetPreset))? {
        Message::BassEnhancement(BassEnhancement::Preset(value)) => bass_preset(value),
        response => return Err(unexpected("bass enhancement preset", response)),
    };
    Ok(BassEnhancementState {
        enabled,
        preset,
        drive: read_bass!(
            BassEnhancement::GetDrive,
            BassEnhancement::Drive,
            "bass drive"
        ),
        mix_percent: read_bass!(BassEnhancement::GetMix, BassEnhancement::Mix, "bass mix"),
        amount: read_bass!(
            BassEnhancement::GetAmount,
            BassEnhancement::Amount,
            "bass amount"
        ),
        attack_ms: read_bass!(
            BassEnhancement::GetAttack,
            BassEnhancement::Attack,
            "bass attack"
        ),
        release_ms: read_bass!(
            BassEnhancement::GetRelease,
            BassEnhancement::Release,
            "bass release"
        ),
        threshold_db: read_bass!(
            BassEnhancement::GetThreshold,
            BassEnhancement::Threshold,
            "bass threshold"
        ),
        knee: read_bass!(BassEnhancement::GetKnee, BassEnhancement::Knee, "bass knee"),
        makeup_gain_db: read_bass!(
            BassEnhancement::GetMakeupGain,
            BassEnhancement::MakeupGain,
            "bass makeup gain"
        ),
        ratio: read_bass!(
            BassEnhancement::GetRatio,
            BassEnhancement::Ratio,
            "bass ratio"
        ),
        cutoff_hz: read_bass!(
            BassEnhancement::GetCutoff,
            BassEnhancement::Cutoff,
            "bass cutoff"
        ),
        q: read_bass!(BassEnhancement::GetQ, BassEnhancement::Q, "bass Q"),
        lower_cutoff_hz: read_bass!(
            BassEnhancement::GetLowerCutoff,
            BassEnhancement::LowerCutoff,
            "bass lower cutoff"
        ),
        lower_q: read_bass!(
            BassEnhancement::GetLowerQ,
            BassEnhancement::LowerQ,
            "bass lower Q"
        ),
    })
}

fn read_de_esser(device: &dyn BeacnAudioDevice) -> BridgeResult<DeEsserState> {
    let amount_percent = match execute(device, Message::DeEsser(DeEsser::GetAmount))? {
        Message::DeEsser(DeEsser::Amount(value)) => value.0,
        response => return Err(unexpected("de-esser amount", response)),
    };
    let enabled = match execute(device, Message::DeEsser(DeEsser::GetEnabled))? {
        Message::DeEsser(DeEsser::Enabled(value)) => value,
        response => return Err(unexpected("de-esser enabled state", response)),
    };
    Ok(DeEsserState {
        enabled,
        amount_percent,
    })
}

fn read_exciter(device: &dyn BeacnAudioDevice) -> BridgeResult<ExciterState> {
    let amount_percent = match execute(device, Message::Exciter(Exciter::GetAmount))? {
        Message::Exciter(Exciter::Amount(value)) => value.0,
        response => return Err(unexpected("exciter amount", response)),
    };
    let frequency_hz = match execute(device, Message::Exciter(Exciter::GetFrequency))? {
        Message::Exciter(Exciter::Frequency(value)) => value.0,
        response => return Err(unexpected("exciter frequency", response)),
    };
    let enabled = match execute(device, Message::Exciter(Exciter::GetEnabled))? {
        Message::Exciter(Exciter::Enabled(value)) => value,
        response => return Err(unexpected("exciter enabled state", response)),
    };
    Ok(ExciterState {
        enabled,
        amount_percent,
        frequency_hz,
    })
}

fn read_headphone_equalizer(
    device: &dyn BeacnAudioDevice,
) -> BridgeResult<HeadphoneEqualizerState> {
    let bands = [
        (HPEQType::Bass, HeadphoneEqBand::Bass),
        (HPEQType::Mids, HeadphoneEqBand::Mids),
        (HPEQType::Treble, HeadphoneEqBand::Treble),
    ]
    .into_iter()
    .map(|(kind, band)| {
        let enabled = match execute(device, Message::HeadphoneEQ(HeadphoneEQ::GetEnabled(kind)))? {
            Message::HeadphoneEQ(HeadphoneEQ::Enabled(k, value)) if k == kind => value,
            response => return Err(unexpected("headphone EQ enabled state", response)),
        };
        let amount_db = match execute(device, Message::HeadphoneEQ(HeadphoneEQ::GetAmount(kind)))? {
            Message::HeadphoneEQ(HeadphoneEQ::Amount(k, value)) if k == kind => value.0,
            response => return Err(unexpected("headphone EQ amount", response)),
        };
        Ok(HeadphoneEqBandState {
            band,
            enabled,
            amount_db,
        })
    })
    .collect::<BridgeResult<Vec<_>>>()?;
    Ok(HeadphoneEqualizerState {
        bands,
        subwoofer: read_subwoofer_state(device)?,
    })
}

fn read_subwoofer_state(device: &dyn BeacnAudioDevice) -> BridgeResult<SubwooferState> {
    let enabled = match execute(device, Message::Subwoofer(Subwoofer::GetEnabled))? {
        Message::Subwoofer(Subwoofer::Enabled(value)) => value,
        response => return Err(unexpected("subwoofer enabled state", response)),
    };
    let amount = match execute(device, Message::Subwoofer(Subwoofer::GetAmount))? {
        Message::Subwoofer(Subwoofer::Amount(value)) => value.0 as u8,
        response => return Err(unexpected("subwoofer amount", response)),
    };
    Ok(SubwooferState { enabled, amount })
}

pub(crate) fn write_microphone_dsp(
    device: &dyn BeacnAudioDevice,
    update: &MicrophoneDspUpdate,
) -> BridgeResult<MicrophoneDspSnapshot> {
    update.validate()?;
    match update {
        MicrophoneDspUpdate::Equalizer(state) => write_equalizer(device, state)?,
        MicrophoneDspUpdate::Compressor(state) => write_compressor(device, state)?,
        MicrophoneDspUpdate::Expander(state) => write_expander(device, state)?,
        MicrophoneDspUpdate::NoiseSuppression(state) => write_noise_suppression(device, state)?,
        MicrophoneDspUpdate::EnhancementSuite(state) => write_enhancement_suite(device, state)?,
        MicrophoneDspUpdate::HeadphoneEqualizer(state) => write_headphone_equalizer(device, state)?,
    }
    let snapshot = read_microphone_dsp(device)?;
    if !update.matches_snapshot(&snapshot) {
        return Err(BridgeError::Backend(format!(
            "{} write failed read-back verification",
            update.module().as_str()
        )));
    }
    Ok(snapshot)
}

fn write_equalizer(device: &dyn BeacnAudioDevice, state: &EqualizerState) -> BridgeResult<()> {
    for profile in [&state.simple, &state.advanced] {
        let mode = to_eq_mode(profile.mode);
        for band in &profile.bands {
            let hardware_band = to_eq_band(band.band)?;
            execute(
                device,
                Message::Equaliser(Equaliser::Type(
                    mode,
                    hardware_band,
                    to_eq_band_type(band.band_type),
                )),
            )?;
            execute(
                device,
                Message::Equaliser(Equaliser::Gain(mode, hardware_band, EQGain(band.gain_db))),
            )?;
            execute(
                device,
                Message::Equaliser(Equaliser::Frequency(
                    mode,
                    hardware_band,
                    EQFrequency(band.frequency_hz),
                )),
            )?;
            execute(
                device,
                Message::Equaliser(Equaliser::Q(mode, hardware_band, EQQ(band.q))),
            )?;
            execute(
                device,
                Message::Equaliser(Equaliser::Enabled(mode, hardware_band, band.enabled)),
            )?;
        }
    }
    execute(
        device,
        Message::Equaliser(Equaliser::Mode(to_eq_mode(state.active_mode))),
    )?;
    Ok(())
}

fn write_compressor(device: &dyn BeacnAudioDevice, state: &CompressorState) -> BridgeResult<()> {
    for profile in [&state.simple, &state.advanced] {
        let mode = to_compressor_mode(profile.mode);
        for message in [
            Compressor::Attack(mode, TimeFrame(profile.attack_ms)),
            Compressor::Release(mode, TimeFrame(profile.release_ms)),
            Compressor::Threshold(mode, CompressorThreshold(profile.threshold_db)),
            Compressor::Ratio(mode, CompressorRatio(profile.ratio)),
            Compressor::MakeupGain(mode, MakeUpGain(profile.makeup_gain_db)),
            Compressor::Enabled(mode, profile.enabled),
        ] {
            execute(device, Message::Compressor(message))?;
        }
    }
    execute(
        device,
        Message::Compressor(Compressor::Mode(to_compressor_mode(state.active_mode))),
    )?;
    Ok(())
}

fn write_expander(device: &dyn BeacnAudioDevice, state: &ExpanderState) -> BridgeResult<()> {
    for profile in [&state.simple, &state.advanced] {
        let mode = to_expander_mode(profile.mode);
        for message in [
            Expander::Threshold(mode, ExpanderThreshold(profile.threshold_db)),
            Expander::Ratio(mode, ExpanderRatio(profile.ratio)),
            Expander::Attack(mode, TimeFrame(profile.attack_ms)),
            Expander::Release(mode, TimeFrame(profile.release_ms)),
            Expander::Enabled(mode, profile.enabled),
        ] {
            execute(device, Message::Expander(message))?;
        }
    }
    execute(
        device,
        Message::Expander(Expander::Mode(to_expander_mode(state.active_mode))),
    )?;
    Ok(())
}

fn write_noise_suppression(
    device: &dyn BeacnAudioDevice,
    state: &NoiseSuppressionState,
) -> BridgeResult<()> {
    for message in [
        Suppressor::Amount(Percent(state.amount_percent)),
        Suppressor::Style(to_suppression_style(state.style)),
        Suppressor::Sensitivity(SuppressorSensitivity(state.sensitivity_db)),
        Suppressor::AdaptTime(SupressorAdaptTime(state.adapt_time_ms)),
        Suppressor::Enabled(state.enabled),
    ] {
        execute(device, Message::Suppressor(message))?;
    }
    Ok(())
}

fn write_enhancement_suite(
    device: &dyn BeacnAudioDevice,
    state: &EnhancementSuiteState,
) -> BridgeResult<()> {
    let bass = &state.bass;
    for message in [
        BassEnhancement::Preset(to_bass_preset(bass.preset)?),
        BassEnhancement::Amount(BassAmount(bass.amount)),
        BassEnhancement::Drive(BassDrive(bass.drive)),
        BassEnhancement::Mix(Percent(bass.mix_percent)),
        BassEnhancement::Attack(TimeFrame(bass.attack_ms)),
        BassEnhancement::Release(TimeFrame(bass.release_ms)),
        BassEnhancement::Threshold(BassThreshold(bass.threshold_db)),
        BassEnhancement::Knee(BassKnee(bass.knee)),
        BassEnhancement::MakeupGain(MakeUpGain(bass.makeup_gain_db)),
        BassEnhancement::Ratio(BassRatio(bass.ratio)),
        BassEnhancement::Cutoff(BassCutoff(bass.cutoff_hz)),
        BassEnhancement::Q(BassQ(bass.q)),
        BassEnhancement::LowerCutoff(BassCutoff(bass.lower_cutoff_hz)),
        BassEnhancement::LowerQ(BassQ(bass.lower_q)),
        BassEnhancement::Enabled(bass.enabled),
    ] {
        execute(device, Message::BassEnhancement(message))?;
    }
    for message in [
        DeEsser::Amount(Percent(state.de_esser.amount_percent)),
        DeEsser::Enabled(state.de_esser.enabled),
    ] {
        execute(device, Message::DeEsser(message))?;
    }
    for message in [
        Exciter::Amount(Percent(state.exciter.amount_percent)),
        Exciter::Frequency(ExciterFreq(state.exciter.frequency_hz)),
        Exciter::Enabled(state.exciter.enabled),
    ] {
        execute(device, Message::Exciter(message))?;
    }
    Ok(())
}

fn write_headphone_equalizer(
    device: &dyn BeacnAudioDevice,
    state: &HeadphoneEqualizerState,
) -> BridgeResult<()> {
    for band in &state.bands {
        let kind = to_headphone_eq_band(band.band);
        execute(
            device,
            Message::HeadphoneEQ(HeadphoneEQ::Amount(kind, HPEQValue(band.amount_db))),
        )?;
        execute(
            device,
            Message::HeadphoneEQ(HeadphoneEQ::Enabled(kind, band.enabled)),
        )?;
    }
    let current_subwoofer = read_subwoofer_state(device)?;
    if current_subwoofer.amount != state.subwoofer.amount {
        for message in Subwoofer::get_amount_messages(state.subwoofer.amount) {
            execute(device, message)?;
        }
    }
    if current_subwoofer.enabled != state.subwoofer.enabled {
        execute(
            device,
            Message::Subwoofer(Subwoofer::Enabled(state.subwoofer.enabled)),
        )?;
    }
    Ok(())
}

fn execute(device: &dyn BeacnAudioDevice, message: Message) -> BridgeResult<Message> {
    device.handle_message(message).map_err(|error| match error {
        beacn_lib::BeacnError::Usb(error) => {
            BridgeError::BackendUnavailable(format!("BEACN USB error: {error}"))
        }
        beacn_lib::BeacnError::Other(error) => BridgeError::Backend(error.to_string()),
    })
}

fn unexpected(label: &str, response: Message) -> BridgeError {
    BridgeError::Backend(format!("unexpected {label} response: {response:?}"))
}

fn eq_mode(mode: EQMode) -> DspMode {
    match mode {
        EQMode::Simple => DspMode::Simple,
        EQMode::Advanced => DspMode::Advanced,
    }
}

fn compressor_mode(mode: CompressorMode) -> DspMode {
    match mode {
        CompressorMode::Simple => DspMode::Simple,
        CompressorMode::Advanced => DspMode::Advanced,
    }
}

fn expander_mode(mode: ExpanderMode) -> DspMode {
    match mode {
        ExpanderMode::Simple => DspMode::Simple,
        ExpanderMode::Advanced => DspMode::Advanced,
    }
}

fn eq_band_type(band_type: EQBandType) -> EqualizerBandType {
    match band_type {
        EQBandType::NotSet => EqualizerBandType::NotSet,
        EQBandType::LowPassFilter => EqualizerBandType::LowPass,
        EQBandType::HighPassFilter => EqualizerBandType::HighPass,
        EQBandType::NotchFilter => EqualizerBandType::Notch,
        EQBandType::BellBand => EqualizerBandType::Bell,
        EQBandType::LowShelf => EqualizerBandType::LowShelf,
        EQBandType::HighShelf => EqualizerBandType::HighShelf,
    }
}

fn suppression_style(style: SuppressorStyle) -> NoiseSuppressionStyle {
    match style {
        SuppressorStyle::Off => NoiseSuppressionStyle::Off,
        SuppressorStyle::Adaptive => NoiseSuppressionStyle::Adaptive,
        SuppressorStyle::Snapshot => NoiseSuppressionStyle::Snapshot,
    }
}

fn bass_preset(preset: BassPreset) -> u8 {
    match preset {
        BassPreset::Preset1 => 1,
        BassPreset::Preset2 => 2,
        BassPreset::Preset3 => 3,
        BassPreset::Preset4 => 4,
    }
}

fn to_eq_mode(mode: DspMode) -> EQMode {
    match mode {
        DspMode::Simple => EQMode::Simple,
        DspMode::Advanced => EQMode::Advanced,
    }
}

fn to_compressor_mode(mode: DspMode) -> CompressorMode {
    match mode {
        DspMode::Simple => CompressorMode::Simple,
        DspMode::Advanced => CompressorMode::Advanced,
    }
}

fn to_expander_mode(mode: DspMode) -> ExpanderMode {
    match mode {
        DspMode::Simple => ExpanderMode::Simple,
        DspMode::Advanced => ExpanderMode::Advanced,
    }
}

fn to_eq_band(number: u8) -> BridgeResult<EQBand> {
    match number {
        1 => Ok(EQBand::Band1),
        2 => Ok(EQBand::Band2),
        3 => Ok(EQBand::Band3),
        4 => Ok(EQBand::Band4),
        5 => Ok(EQBand::Band5),
        6 => Ok(EQBand::Band6),
        7 => Ok(EQBand::Band7),
        8 => Ok(EQBand::Band8),
        _ => Err(BridgeError::InvalidValue(format!(
            "invalid equalizer band: {number}"
        ))),
    }
}

fn to_eq_band_type(band_type: EqualizerBandType) -> EQBandType {
    match band_type {
        EqualizerBandType::NotSet => EQBandType::NotSet,
        EqualizerBandType::LowPass => EQBandType::LowPassFilter,
        EqualizerBandType::HighPass => EQBandType::HighPassFilter,
        EqualizerBandType::Notch => EQBandType::NotchFilter,
        EqualizerBandType::Bell => EQBandType::BellBand,
        EqualizerBandType::LowShelf => EQBandType::LowShelf,
        EqualizerBandType::HighShelf => EQBandType::HighShelf,
    }
}

fn to_suppression_style(style: NoiseSuppressionStyle) -> SuppressorStyle {
    match style {
        NoiseSuppressionStyle::Off => SuppressorStyle::Off,
        NoiseSuppressionStyle::Adaptive => SuppressorStyle::Adaptive,
        NoiseSuppressionStyle::Snapshot => SuppressorStyle::Snapshot,
    }
}

fn to_bass_preset(preset: u8) -> BridgeResult<BassPreset> {
    match preset {
        1 => Ok(BassPreset::Preset1),
        2 => Ok(BassPreset::Preset2),
        3 => Ok(BassPreset::Preset3),
        4 => Ok(BassPreset::Preset4),
        _ => Err(BridgeError::InvalidValue(format!(
            "invalid bass enhancement preset: {preset}"
        ))),
    }
}

fn to_headphone_eq_band(band: HeadphoneEqBand) -> HPEQType {
    match band {
        HeadphoneEqBand::Bass => HPEQType::Bass,
        HeadphoneEqBand::Mids => HPEQType::Mids,
        HeadphoneEqBand::Treble => HPEQType::Treble,
    }
}
