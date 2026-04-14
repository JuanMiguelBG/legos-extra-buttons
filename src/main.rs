use rusb::{Context, DeviceHandle, UsbContext};
use std::time::{Duration};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use evdev::{Device, InputEvent, EventType, Key};
use clap::{Parser, ArgEnum};

const INTERFACE: u8 = 6;
const ENDPOINT: u8 = 0x86;

const TIMEOUT: Duration = Duration::from_millis(1000);
const PACKET_SIZE: usize = 32;

// ==============================
// 🎮 MODE
// ==============================
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ArgEnum)]
enum Mode {
    Normal,
    SteamDeck,
}

// ==============================
// 📥 CLI
// ==============================
#[derive(Parser)]
#[clap(author, version, about = "LEGOS Extra Buttons Handler")]
struct Args {
    #[clap(short = 'e', long = "ep")]
    evdev_path: Option<String>,

    #[clap(long, default_value = "Lenovo Legion Go S")]
    device_name: String,

    #[clap(long, default_value = "0x1a86")]
    vid: String,

    #[clap(long, default_value = "0xe310")]
    pid: String,

    #[clap(short, long, default_value = "5")]
    ignore: u32,

    #[clap(short, long, arg_enum, default_value = "normal")]
    mode: Mode,

    #[clap(long)]
    verbose: bool,

    #[clap(long)]
    trace: bool,

    #[clap(long)]
    raw: bool,
}

// ==============================
// 🔧 HEX PARSER
// ==============================
fn parse_u16_hex(input: &str) -> Result<u16, Box<dyn std::error::Error>> {
    let s = input.trim();

    let value = if s.starts_with("0x") {
        u16::from_str_radix(&s[2..], 16)?
    } else {
        u16::from_str_radix(s, 16)?
    };

    Ok(value)
}

// ==============================
// 🔍 EVDEV AUTO DETECT
// ==============================
fn find_evdev_device(
    device_name: &str,
    vid: u16,
    pid: u16,
) -> Result<String, Box<dyn std::error::Error>> {

    for entry in std::fs::read_dir("/dev/input")? {
        let entry = entry?;
        let path = entry.path();

        let path_str = match path.to_str() {
            Some(s) => s,
            None => continue,
        };

        if !path_str.contains("event") {
            continue;
        }

        let dev = match Device::open(path_str) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let id = dev.input_id();

        if id.vendor() != vid || id.product() != pid {
            continue;
        }

        if let Some(name) = dev.name() {
            if !device_name.is_empty() && name != device_name {
                continue;
            }
        }

        return Ok(path_str.to_string());
    }

    Err(format!(
        "Device not found (name='{}', vid=0x{:04x}, pid=0x{:04x})",
        device_name, vid, pid
    ).into())
}

// ==============================
// 🔌 USB OPEN
// ==============================
fn open_device(context: &Context, vid: u16, pid: u16) -> Option<DeviceHandle<Context>> {
    let devices = context.devices().ok()?;

    for device in devices.iter() {
        let desc = device.device_descriptor().ok()?;

        if desc.vendor_id() == vid && desc.product_id() == pid {
            return device.open().ok();
        }
    }

    None
}

// ==============================
// 🎮 EMIT EVDEV
// ==============================
fn emit_evdev(
    dev: &mut Device,
    legion: u8,
    qa: u8,
    y2: u8,
    y1: u8,
    mode: Mode,
) -> Result<(), Box<dyn std::error::Error>> {

    let mut events = Vec::new();

    let (legion_key, qa_key) = match mode {
        Mode::SteamDeck => (Key::BTN_SELECT, Key::BTN_START),
        Mode::Normal => (Key::BTN_BASE, Key::BTN_MODE),
    };

    events.push(InputEvent::new(EventType::KEY, legion_key.code(), legion as i32));
    events.push(InputEvent::new(EventType::KEY, qa_key.code(), qa as i32));
    events.push(InputEvent::new(EventType::KEY, Key::BTN_TRIGGER_HAPPY5.code(), y2 as i32));
    events.push(InputEvent::new(EventType::KEY, Key::BTN_TRIGGER_HAPPY7.code(), y1 as i32));

    events.push(InputEvent::new(EventType::SYNCHRONIZATION, 0, 0));

    dev.send_events(&events)?;

    Ok(())
}

// ==============================
// 🚀 MAIN
// ==============================
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let vid = parse_u16_hex(&args.vid)?;
    let pid = parse_u16_hex(&args.pid)?;

    let context = Context::new()?;

    let handle = open_device(&context, vid, pid)
                    .expect("Can't open USB device");

    handle.set_auto_detach_kernel_driver(true).ok();
    handle.claim_interface(INTERFACE)?;

    println!("USB OK");

    let evdev_path = match args.evdev_path {
        Some(p) => p,
        None => {
            let detected = find_evdev_device(&args.device_name, vid, pid)?;
            println!(
                "Auto-detected '{}' VID=0x{:04x} PID=0x{:04x} -> {}",
                args.device_name, vid, pid, detected
            );
            detected
        }
    };

    let mut dev = Device::open(&evdev_path)?;
    println!("evdev open: {}", evdev_path);

    println!("Mode: {:?}", args.mode);

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })?;

    let mut data = [0u8; PACKET_SIZE];
    let mut prev = [0u8; 4];
    let mut counter: u32 = 0;

    println!("Reading HID...\n");

    while running.load(Ordering::SeqCst) {
        match handle.read_interrupt(ENDPOINT, &mut data, TIMEOUT) {
            Ok(len) => {
                counter += 1;
                let ignored = counter % args.ignore != 0;

                if ignored && !args.raw {
                    continue;
                }

                if len < 3 {
                    continue;
                }

                let b0 = data[0];
                let b2 = data[2];

                let quick_access = (b0 >> 1) & 1;
                let legion       = (b0 >> 0) & 1;

                let y2 = (b2 >> 1) & 1;
                let y1 = (b2 >> 0) & 1;

                let current = [legion, quick_access, y2, y1];

                if current != prev {
                    emit_evdev(&mut dev, legion, quick_access, y2, y1, args.mode)?;
                    prev = current;
                }
            }

            Err(rusb::Error::Timeout) => {}

            Err(e) => {
                eprintln!("USB error: {:?}", e);
                break;
            }
        }
    }

    println!("Exiting...");
    Ok(())
}
