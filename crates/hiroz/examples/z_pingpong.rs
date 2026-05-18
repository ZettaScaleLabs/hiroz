#[cfg(not(test))]
use std::{fs::File, path::PathBuf};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};

#[cfg(not(test))]
use clap::Parser;
#[cfg(not(test))]
use csv::Writer;
use hiroz::{Builder, Result, ZBuf, context::ZContextBuilder};
use hiroz_msgs::std_msgs::ByteMultiArray;
use zenoh_buffers::buffer::{Buffer, SplitBuffer};

#[cfg_attr(not(test), derive(Parser))]
#[derive(Debug)]
pub struct Args {
    #[cfg_attr(
        not(test),
        arg(short, long, default_value = "ping", help = "Mode: ping or pong")
    )]
    pub mode: String,
    #[cfg_attr(
        not(test),
        arg(short, long, default_value = "64", help = "Payload size in bytes")
    )]
    pub payload: usize,
    #[cfg_attr(
        not(test),
        arg(short, long, default_value = "10", help = "Frequency in Hz")
    )]
    pub frequency: usize,
    #[cfg_attr(
        not(test),
        arg(short, long, default_value = "100", help = "Number of samples")
    )]
    pub sample: usize,
    #[cfg_attr(
        not(test),
        arg(short, long, default_value = "", help = "Log file path")
    )]
    pub log: String,
}

fn get_percentile(data: &[u64], percentile: f64) -> u64 {
    if data.is_empty() {
        return 0;
    }
    let idx = ((percentile * data.len() as f64).round() as usize).min(data.len() - 1);
    data[idx]
}

fn print_statistics(mut rtts: Vec<u64>) {
    rtts.sort();
    println!("\nRTT stats (nanoseconds):");
    println!("Min : {}", rtts.first().unwrap());
    println!("p05 : {}", get_percentile(&rtts, 0.05));
    println!("p25 : {}", get_percentile(&rtts, 0.25));
    println!("p50 : {}", get_percentile(&rtts, 0.50));
    println!("p75 : {}", get_percentile(&rtts, 0.75));
    println!("p95 : {}", get_percentile(&rtts, 0.95));
    println!("Max : {}", rtts.last().unwrap());
}

#[cfg(not(test))]
#[derive(Debug)]
struct DataLogger {
    payload: usize,
    frequency: usize,
    path: PathBuf,
}

#[cfg(not(test))]
impl DataLogger {
    fn write(&self, data: Vec<u64>) -> Result<()> {
        let file = File::create(&self.path)?;
        let mut wtr = Writer::from_writer(file);
        wtr.write_record(
            ["Frequency", "Payload", "RTT"]
                .iter()
                .map(|x| x.to_string()),
        )?;

        for val in data {
            wtr.write_record(
                [self.frequency, self.payload, val as _]
                    .iter()
                    .map(|x| x.to_string()),
            )?;
            wtr.flush()?;
        }
        Ok(())
    }
}

pub fn run_ping(ctx: hiroz::context::ZContext, args: &Args) -> Result<()> {
    let node = ctx.create_node("ping_node").build()?;
    let zpub = node.create_pub::<ByteMultiArray>("ping").build()?;
    let zsub = node.create_sub::<ByteMultiArray>("pong").build()?;
    let period = Duration::from_secs_f64(1.0 / args.frequency as f64);
    let finished = Arc::new(AtomicBool::new(false));
    let c_finished = finished.clone();

    println!(
        "Freq: {} Hz, Payload: {} bytes, Samples: {}",
        &args.frequency, &args.payload, &args.sample
    );

    #[cfg(not(test))]
    let logger = if args.log.is_empty() {
        None
    } else {
        Some(DataLogger {
            frequency: args.frequency,
            payload: args.payload,
            path: PathBuf::from(args.log.clone()),
        })
    };

    let start = Instant::now();
    let sample_count = args.sample;
    std::thread::spawn(move || {
        let mut rtts = Vec::with_capacity(sample_count);
        while rtts.len() < sample_count {
            if let Ok(msg) = zsub.recv() {
                let data_bytes = msg.data.contiguous();
                let sent_time = u64::from_le_bytes(data_bytes[0..8].try_into().unwrap());
                let rtt = start.elapsed().as_nanos() as u64 - sent_time;
                rtts.push(rtt);
            }
        }
        #[cfg(not(test))]
        if let Some(logger) = logger {
            logger.write(rtts.clone()).expect("Failed to write the log");
        }
        print_statistics(rtts);
        c_finished.store(true, SeqCst);
    });

    while !finished.load(SeqCst) {
        let now = start.elapsed().as_nanos() as u64;

        // Create buffer with timestamp embedded (no clone needed - ZBuf takes ownership)
        let mut buffer = vec![0xAA; args.payload];
        buffer[0..8].copy_from_slice(&now.to_le_bytes());

        let msg = ByteMultiArray {
            data: ZBuf::from(buffer),
            ..Default::default()
        };
        zpub.publish(&msg)?;
        std::thread::sleep(period);
    }
    Ok(())
}

pub fn run_pong(ctx: hiroz::context::ZContext) -> Result<()> {
    let node = ctx.create_node("pong_node").build()?;
    let zsub = node.create_sub::<ByteMultiArray>("ping").build()?;
    let zpub = node.create_pub::<ByteMultiArray>("pong").build()?;

    println!("Pong begin looping...");

    let mut message_count = 0u64;
    let mut last_print_time = Instant::now();

    loop {
        let msg = zsub.recv()?;
        message_count += 1;

        let data_bytes = msg.data.contiguous();
        let last_timestamp = u64::from_le_bytes(data_bytes[0..8].try_into().unwrap());
        let last_payload_size = msg.data.len();

        zpub.publish(&msg)?;

        let current_time = Instant::now();
        if current_time.duration_since(last_print_time) >= Duration::from_secs(2) {
            println!(
                "Pong status: received {} messages (last payload: {} bytes, last timestamp: {} ns)",
                message_count, last_payload_size, last_timestamp
            );
            last_print_time = current_time;
        }
    }
}

#[cfg(not(test))]
fn main() -> Result<()> {
    let args = Args::parse();

    if args.mode != "ping" && args.mode != "pong" {
        eprintln!("Invalid mode: {}. Must be 'ping' or 'pong'", args.mode);
        std::process::exit(1);
    }

    let ctx = ZContextBuilder::default().with_logging_enabled().build()?;
    match args.mode.as_str() {
        "ping" => run_ping(ctx, &args),
        "pong" => run_pong(ctx),
        _ => unreachable!(),
    }
}
