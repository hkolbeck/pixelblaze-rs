use std::{mem, thread};
use std::net::SocketAddr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;

use clap::Parser;
use cpal::{SampleFormat, SampleRate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use realfft::num_complex::{Complex, Complex32};
use realfft::num_traits::Zero;
use realfft::RealFftPlanner;

use pixelblaze_rs::sensor::SensorClient;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(short = 'f', long, default_value = "2048")]
    frame_samples: usize,

    #[arg(short = 'r', long, default_value = "48000")]
    sample_rate_hz: u32,

    targets: Vec<SocketAddr>,
}

struct FrameCollector {
    frame: Vec<f32>,
    frame_target_len: usize,
    frame_sender: Sender<Vec<f32>>,
}

impl FrameCollector {
    fn new(samples: usize, client: SensorClient) -> (FrameCollector, JoinHandle<()>) {
        let (tx, rx) = channel();
        let join_handle =
            thread::spawn(move || handle_frames(samples, client, rx));
        (
            FrameCollector {
                frame: Vec::with_capacity(samples),
                frame_target_len: samples,
                frame_sender: tx,
            },
            join_handle
        )
    }

    fn add_samples(&mut self, samples: &[f32]) {
        if self.frame.len() + samples.len() < self.frame_target_len {
            self.frame.extend_from_slice(samples);
            return;
        }

        let taken = self.frame_target_len - self.frame.len();
        let shrunk = &samples[..taken];
        self.frame.extend_from_slice(shrunk);

        // Assuming samples.len < frame.len() #YOLO
        let mut new_frame = Vec::with_capacity(self.frame_target_len);
        new_frame.extend_from_slice(&samples[taken..]);

        let old_frame = mem::replace(&mut self.frame, new_frame);
        self.frame_sender.send(old_frame).expect("Receiver has hung up");
    }
}

fn handle_frames(frame_len: usize, client: SensorClient, receiver: Receiver<Vec<f32>>) {
    let mut planner = RealFftPlanner::new();
    let fft = planner.plan_fft_forward(frame_len);
    let mut scratch = vec![Complex::zero(); frame_len];
    receiver.into_iter().for_each(|frame| {
        let mut complex: Vec<Complex<f32>> = frame.into_iter()
            .map(|v| Complex32 { re: v, im: 0.0 })
            .collect();
        fft.process_with_scratch(&mut complex, &mut scratch);

        let amplitudes: Vec<f32> = complex.iter()
            .take(frame_len / 2)
            .map(|c| (c.im * c.im + c.re * c.re).sqrt() / frame_len.into())
            .collect();

        let mut sum: f32 = 0.0;
        let mut maxFreqIdx = usize::MAX;
        let mut maxFreqMagnitude: f32 = 0.0;
        for (idx, bucket) in amplitudes.iter().enumerate() {
            sum += bucket;
            if *bucket > maxFreqMagnitude {
                maxFreqIdx = idx;
                maxFreqMagnitude = *bucket;
            }
        }

        let buckets: Vec<u16> = amplitudes.chunks(frame_len / 32)
            .take(32)
            .map(|chunk| chunk.iter().sum())
            .map(|val: f32| )
            .collect();
    })
}


fn main() {
    let cli = Cli::parse();
    let host = cpal::default_host();

    let input_device = host.input_devices()
        .expect("Couldn't get input devices")
        .nth(0)
        .expect("Couldn't get first input option");
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
    let (mut collector, join_handle) = FrameCollector::new(cli.frame_samples, client);

    let stream = input_device.build_input_stream(
        &config.into(),
        move |data, _| collector.add_samples(data),
        |err| eprintln!("an error occurred on stream: {}", err),
        None,
    ).expect("Build input stream failed");
    stream.play().expect("Play failed");

    join_handle.join();
}
