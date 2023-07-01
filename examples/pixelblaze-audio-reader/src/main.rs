extern crate core;
#[macro_use]
extern crate text_io;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use cpal::{BufferSize, Device, InputCallbackInfo, SampleFormat, SampleRate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use spectrum_analyzer::{FrequencyLimit, samples_fft_to_spectrum};
use spectrum_analyzer::scaling::divide_by_N;
use spectrum_analyzer::windows::hann_window;

use pixelblaze_rs::sensor::{AudioData, SensorClient};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(short = 'f', long, default_value = "1024")]
    frame_samples: u32,

    #[arg(short = 'r', long, default_value = "48000")]
    sample_rate_hz: u32,

    targets: Vec<SocketAddr>,
}

///! This is a simple utility for accepting audio input, performing frequency analysis, then
///! shipping the analysis off to a pixelblaze. It's tested on OSX, if you get it working on
///! other platforms please let me know! It has not been tested with multiple targets (yet).
///!
///! If there are multiple input devices, you'll be prompted to select one. This can usually
///! find the builtin mic if one exists.
///! In order to ingest the active audio from your computer, you'll need to create a loopback
///! audio device. The process for this varies by OS:
///!
///! OSX:
///! One pay-what-you-can tool that offers this is https://existential.audio/blackhole/.
///! You'll also need to set up a MIDI output:
///! https://github.com/ExistentialAudio/BlackHole/wiki/Multi-Output-Device
///!
///! Windows/Linux: Contributions welcome
///!
///! Running this example:
///!  1. Set up the loopback audio device as outlined above (optional)
///!  2. Power on your Pixelblaze and select a sound-reactive pattern
///!  3. Start this program. In the pixelblaze-audio-reader directory, open a terminal and execute:
///!       `cargo run --package pixelblaze-audio-reader --bin pixelblaze-audio-reader --release 192.168.4.1:1889`
///!     Replacing 192.168.4.1 with the IP of your Pixelblaze, be sure to include the port number.
///!  4. You'll be prompted to choose an input device, select the mic or loopback device you set up
///!     in step (1). If it does not appear, the CPAL docs are likely a good first debugging tool.
///!  5. If everything is working smoothly, you should start seeing debugging printouts and the
///!     Pixelblaze should begin using the frequency analysis data.
///!     - If it fails to reach your Pixelblaze, make sure you're on the same network, have the
///!       right IP, and are including the port in the program invocation.
///!     - If the program seems to be gathering data and successfully sending it off, but the
///!       Pixelblaze is not using the data, remove any sensor boards attached to it or change
///!        the sensor input source to "Remote".
///!
///! If you run into issues or have tuning suggestions, please contact the author.
fn main() {
    let cli = Cli::parse();
    let host = cpal::default_host();

    let devices: Vec<Device> = host.input_devices()
        .expect("Couldn't get input devices")
        .collect();

    let input_device = match devices.len() {
        0 => panic!("No input devices found!"),
        1 => devices.first().expect("That definitely has a first"),
        _ => {
            let device_map: HashMap<String, &Device> = devices.iter().enumerate()
                .map(|(idx, device)| {
                    println!("  {}: {}", idx, device.name().unwrap_or("No name available".to_string()));
                    (idx.to_string(), device.clone())
                })
                .collect();
            print!("Multiple input devices available, select by number: ");
            loop {
                let line: String = read!("{}\n");
                let line = line.trim();
                match device_map.get(line) {
                    None => print!("No device found for '{}', select by number: ", line),
                    Some(device) => break device.clone(),
                }
            }
        }
    };

    println!("Found input device: {}",
             input_device.name().unwrap_or("Name not found".to_string()));

    let sample_rate = SampleRate(cli.sample_rate_hz);
    let config = input_device.supported_input_configs()
        .expect("Couldn't get supported input configs")
        .filter(|config| config.sample_format() == SampleFormat::F32)
        .filter(|config|
            config.min_sample_rate() <= sample_rate && sample_rate <= config.max_sample_rate())
        .map(|config| config.with_sample_rate(sample_rate))
        .nth(0)
        .expect("No fitting input configuration found");

    let mut client = SensorClient::new(89);
    cli.targets.iter().for_each(|target| client.add_target(target.clone()));

    let mut frame_count: u64 = 0;
    let mut last_frame = Instant::now();
    let mut padding_buf = Vec::new();
    let to_spectrum_fn = move |audio: &[f32], _: &InputCallbackInfo| {
        let audio = if audio.len().count_ones() == 1 {
            audio
        } else {
            // The default windows machinery doesn't always send a full buffer.
            // Might as well pad instead of panicking
            let new_len = audio.len().next_power_of_two();
            let mut padding = vec![0.0_f32; new_len - audio.len()];
            padding_buf.clear();
            padding_buf.extend_from_slice(audio);
            padding_buf.append(&mut padding);
            &padding_buf[..]
        };

        let hann_window = hann_window(audio);
        let latest_spectrum = samples_fft_to_spectrum(
            &hann_window,
            cli.sample_rate_hz,
            FrequencyLimit::All,
            Some(&divide_by_N),
        ).unwrap();

        let energy_avg = latest_spectrum.average();
        let (max_freq, max_freq_magnitude) = latest_spectrum.max();
        let trimmed: Vec<f32> = latest_spectrum.data().iter()
            .take_while(|(freq, _)| freq.val() < 10_000f32)
            .map(|(_, freq_val)| freq_val.val())
            .collect();
        let bucketed: Vec<u16> = trimmed.chunks(trimmed.len() / 32)
            .take(32)
            .map(|c| c.iter().sum())
            .map(|flt: f32| (flt.clamp(0.0, 1.0).to_scaled_u16()))
            .collect();

        frame_count += 1;
        let frame_delay = Instant::now().duration_since(last_frame);
        last_frame = Instant::now();
        if frame_count % 50 == 0 {
            println!(
                "Sent {} frames. ({}ms/frame) frame size = {}. spectrum[1].freq = {}Hz, spectrum[{}].freq={}Hz. bucketed[0]={}, bucketed[{}] = {}",
                frame_count,
                frame_delay.as_millis(),
                audio.len(),
                latest_spectrum.data()[1].0.val(),
                latest_spectrum.data().len() - 1,
                latest_spectrum.data().last().expect("No last bucket?").0.val(),
                bucketed[0],
                bucketed.len() - 1,
                bucketed.last().expect("No last bucket?")
            );
        }

        let audio = AudioData {
            freq_buckets: bucketed,
            energy_avg: energy_avg.val().to_scaled_u16(),
            max_freq_magnitude: max_freq_magnitude.val().to_scaled_u16(),
            max_freq: max_freq.val().to_scaled_u16(),
        };

        if let Err(err) = client.send_frame(&audio, &[0; 3], 0, &[0; 5]) {
            eprintln!("Failed to send frame: {:?}", err)
        }
    };

    let mut config = config.config();
    config.buffer_size = BufferSize::Fixed(cli.frame_samples);
    println!("Found config: {:?}", config);

    let stream = input_device.build_input_stream(
        &config,
        to_spectrum_fn,
        |err| eprintln!("an error occurred on stream: {}", err),
        None,
    ).expect("Build input stream failed");
    stream.play().expect("Play failed");

    loop {
        thread::sleep(Duration::from_secs(1))
    }
}

trait Shortable {
    fn to_scaled_u16(self) -> u16;
}

impl Shortable for f32 {
    fn to_scaled_u16(self) -> u16 {
        (self * u16::MAX as f32) as u16
    }
}