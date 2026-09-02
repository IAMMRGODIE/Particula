//! Headless CLAP probe for ParticulaCloud.
//!
//! Loads the built plugin, feeds a known stereo signal through the Main
//! stereo in/out for ~2.5 s, and reports whether the wet path produces any
//! energy beyond the dry passthrough — reproducing what a DAW would do
//! without the DAW.
//!
//! Run: cargo run --release -p particula_plugin --example clap_probe -- <path-to-clap-or-dll>

use clack_host::factory::plugin::PluginFactory;
use clack_host::prelude::*;
use std::error::Error;

struct ProbeHost;
impl HostHandlers for ProbeHost {
    type Shared<'a> = ();
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
    fn declare_extensions(_: &mut HostExtensions<Self>, _: &Self::Shared<'_>) {}
}

const SR: f64 = 48_000.0;
const FRAMES: usize = 2_048;
const BLOCKS: usize = 60; // ~2.5 s

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/release/ParticulaCloud.clap".to_string());
    println!("loading {path}");

    let bundle = unsafe { PluginBundle::load(&path)? };
    let factory = bundle.get_factory::<PluginFactory>().unwrap();
    let desc = factory.plugin_descriptor(0).ok_or("no descriptor 0")?;
    let plugin_id = desc.id().ok_or("no plugin id")?;
    println!("descriptor: {} \"{}\"", plugin_id.to_str()?, desc.name().unwrap_or(c"").to_str()?);

    let host_info = HostInfo::new(
        "particula-probe",
        "particula",
        "https://example.invalid",
        env!("CARGO_PKG_VERSION"),
    )?;
    let mut instance = PluginInstance::<ProbeHost>::new(
        |_| (),
        |_| (),
        &bundle,
        plugin_id,
        &host_info,
    )
    .map_err(|e| format!("instantiate failed: {e:?}"))?;

    let config = PluginAudioConfiguration {
        sample_rate: SR,
        min_frames_count: FRAMES as u32,
        max_frames_count: FRAMES as u32,
    };
    let mut processor = instance
        .activate(|_, _| (), config)
        .map_err(|e| format!("activate failed: {e:?}"))?
        .start_processing()
        .map_err(|e| format!("start processing failed: {e:?}"))?;

    // One stereo port: a single Vec laid out as [L.., R..].
    let mut input: Vec<f32> = vec![0.0; 2 * FRAMES];
    let mut output: Vec<f32> = vec![0.0; 2 * FRAMES];
    let mut input_ports = AudioPorts::with_capacity(2, 1);
    let mut output_ports = AudioPorts::with_capacity(2, 1);

    let mut in_peak = 0.0_f32;
    let mut out_peak = 0.0_f32;
    let mut out_sq = 0.0_f64;
    let mut steady: u64 = 0;
    for block in 0..BLOCKS {
        // Known input: DC 0.4 + soft 220 Hz wobble on both channels.
        for i in 0..FRAMES {
            let s = 0.4
                + 0.08 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / SR as f32).sin();
            input[i] = s;
            input[FRAMES + i] = s;
            in_peak = in_peak.max(s.abs());
        }

        let ins = input_ports.with_input_buffers(std::iter::once(AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                input.chunks_mut(FRAMES).map(|c| InputChannel {
                    buffer: &mut c[..FRAMES],
                    is_constant: true,
                }),
            ),
        }));
        let mut outs = output_ports.with_output_buffers(std::iter::once(AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                output.chunks_mut(FRAMES).map(|c| &mut c[..FRAMES]),
            ),
        }));

        processor.process(
            &ins,
            &mut outs,
            &InputEvents::empty(),
            &mut OutputEvents::void(),
            Some(steady),
            None,
        )?;
        steady += FRAMES as u64;

        for &s in output.iter() {
            out_peak = out_peak.max(s.abs());
            out_sq += s as f64 * s as f64;
        }
        if block == 0 || block == BLOCKS - 1 {
            println!("block {block}: out[..4] = {:?}", &output[..4]);
        }
    }

    let out_rms = (out_sq / (BLOCKS * 2 * FRAMES) as f64).sqrt();
    println!("in_peak  = {in_peak:.4}");
    println!("out_peak = {out_peak:.4}");
    println!("out_rms  = {out_rms:.4}");
    if out_peak > in_peak + 1e-3 {
        println!("RESULT: wet path produces extra energy (effect audible).");
    } else {
        println!("RESULT: output <~ input -> wet path silent in the CLAP context.");
    }
    Ok(())
}