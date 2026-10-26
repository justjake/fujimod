//! EXR II board constructors for this crate's models.
use fujimod_emulator_exr2_plan::{
    Model,
    construction::{Builder, Built, Constructors, decode},
};
use fujimod_emulator_exr2_resources::wire;
use fujimod_emulator_exr2_serial::dma;
use fujimod_emulator_parts_io::audio_sink::AudioSink;

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("serial-audio-duplex", serial_audio_duplex);
    constructors.add("serial-audio-receiver", serial_audio_receiver);
    constructors.add("serial-audio-transmitter", serial_audio_transmitter);
    constructors.add("serial-audio", serial_audio);
}

fn serial_audio_duplex(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        id,
        windows,
        resources,
        ..
    } = builder;
    let Model::SerialAudioDuplex { options, routes } = model else {
        return Err("constructor called for another model kind".into());
    };
    let expected = [
        (dma::audio::BASE, 0x40),
        (crate::BASES[0], 0x1000),
        (crate::BASES[1], 0x1000),
    ];
    if windows.len() != expected.len()
        || windows
            .iter()
            .zip(expected)
            .any(|(window, (base, size))| window.base != base || window.size != size)
    {
        return Err("audio duplex requires its DMA channel and both audio banks".into());
    }
    Ok(Built::Device(Box::new(crate::duplex::Duplex::new(
        decode(options)?,
        wire!(resources, id, routes, IrqRoutes),
    )?)))
}

fn serial_audio_receiver(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        id,
        windows,
        resources,
        ..
    } = builder;
    let Model::SerialAudioReceiver { options, routes } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::receive::Options = decode(options)?;
        let expected = [
            (dma::audio::BASE, 0x40),
            (crate::BASES[0], 0x1000),
            (crate::BASES[1], 0x1000),
        ];
        if windows.len() != expected.len()
            || windows
                .iter()
                .zip(expected)
                .any(|(w, (base, size))| w.base != base || w.size != size)
        {
            return Err("audio receiver requires its DMA channel and both audio banks".into());
        }
        Box::new(crate::receive::Receiver::new(
            options,
            wire!(resources, id, routes, IrqRoutes),
        )?)
    }))
}

fn serial_audio_transmitter(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        id,
        windows,
        resources,
        ..
    } = builder;
    let Model::SerialAudioTransmitter {
        options,
        sinks,
        routes,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::transmit::Options = decode(options)?;
        let sinks: [fujimod_emulator_parts_io::audio_sink::Options; 2] = decode(sinks)?;
        let expected = [
            (dma::audio::BASE, 0x40),
            (crate::BASES[0], 0x1000),
            (crate::BASES[1], 0x1000),
        ];
        if windows.len() != expected.len()
            || windows
                .iter()
                .zip(expected)
                .any(|(w, (base, size))| w.base != base || w.size != size)
        {
            return Err("audio transmitter requires its DMA channel and both audio banks".into());
        }
        Box::new(crate::transmit::Transmitter::new(
            options,
            sinks.map(AudioSink::new),
            wire!(resources, id, routes, IrqRoutes),
        )?)
    }))
}

fn serial_audio(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::SerialAudio { options, sinks } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::Options = decode(options)?;
        let sinks: [fujimod_emulator_parts_io::audio_sink::Options; 2] = decode(sinks)?;
        Box::new(crate::SerialAudio::new(options, sinks.map(AudioSink::new)))
    }))
}
