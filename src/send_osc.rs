use crate::AppMessage;
use crate::utility::error_alert;

use fltk::prelude::*;
use std::thread;
use std::error::Error;
use std::sync::mpsc;
use std::string::ToString;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::iter::Iterator;

extern crate rosc;
use rosc::encoder;
use rosc::{OscMessage, OscPacket, OscType};
use std::net::{SocketAddrV4, UdpSocket};
use std::time::Duration;

// TODO: To cut down on repetition in these enums: Either use something like strum. Or make your own macro maybe?
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Color {
    Grayscale,
    #[default]
    Indexed,
}

impl FromStr for Color {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Grayscale" => Ok(Self::Grayscale),
            "Indexed" => Ok(Self::Indexed),
            _ => Err(format!("Couldn't parse as {}: {}", std::any::type_name::<Self>(), s)),
        }
    }
}

impl ToString for Color {
    fn to_string(&self) -> String {
        format!("{:?}", self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PixFmt {
    Auto(Color),
    Bpp1(Color),
    Bpp2(Color),
    Bpp4(Color),
    Bpp8(Color),
}

impl Default for PixFmt {
    fn default() -> Self {
        PixFmt::Auto(Color::Indexed)
    }
}

impl ToString for PixFmt {
    fn to_string(&self) -> String {
        format!("{:?}", self)
    }
}

impl FromStr for PixFmt {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Auto"            => Ok(Self::Auto(Default::default())),
            "Bpp1"            => Ok(Self::Bpp1(Default::default())),
            "Bpp2"            => Ok(Self::Bpp2(Default::default())),
            "Bpp4"            => Ok(Self::Bpp4(Default::default())),
            "Bpp8"            => Ok(Self::Bpp8(Default::default())),
            "Auto(Indexed)"   => Ok(Self::Auto(Color::Indexed)),
            "Auto(Grayscale)" => Ok(Self::Auto(Color::Grayscale)),
            "Bpp1(Indexed)"   => Ok(Self::Bpp1(Color::Indexed)),
            "Bpp2(Indexed)"   => Ok(Self::Bpp2(Color::Indexed)),
            "Bpp4(Indexed)"   => Ok(Self::Bpp4(Color::Indexed)),
            "Bpp8(Indexed)"   => Ok(Self::Bpp8(Color::Indexed)),
            "Bpp1(Grayscale)" => Ok(Self::Bpp1(Color::Grayscale)),
            "Bpp2(Grayscale)" => Ok(Self::Bpp2(Color::Grayscale)),
            "Bpp4(Grayscale)" => Ok(Self::Bpp4(Color::Grayscale)),
            "Bpp8(Grayscale)" => Ok(Self::Bpp8(Color::Grayscale)),
            _ => Err(format!("Couldn't parse as {}: {}", std::any::type_name::<Self>(), s)),
        }
    }
}

impl PixFmt {
    pub const VALUES: [PixFmt; 10] = [
        PixFmt::Auto(Color::Indexed),
        PixFmt::Auto(Color::Grayscale),
        PixFmt::Bpp1(Color::Indexed),
        PixFmt::Bpp2(Color::Indexed),
        PixFmt::Bpp4(Color::Indexed),
        PixFmt::Bpp8(Color::Indexed),
        PixFmt::Bpp1(Color::Grayscale),
        PixFmt::Bpp2(Color::Grayscale),
        PixFmt::Bpp4(Color::Grayscale),
        PixFmt::Bpp8(Color::Grayscale),
    ];

    pub fn into_iter() -> core::array::IntoIter<PixFmt, 10> {
        Self::VALUES.into_iter()
    }
}

fn create_progressbar_window(
    appmsg: &mpsc::Sender<AppMessage>,
) -> Result<(Arc<AtomicBool>, fltk::window::Window, fltk::misc::Progress),
            Box<dyn Error>> {

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(fltk::window::Window, fltk::misc::Progress)>();

    // New windows need to be created on the main thread, so we message the main thread
    appmsg.send({
        let cancel_flag = Arc::clone(&cancel_flag);
        AppMessage::CreateWindow(
            400, 200, "Sending OSC".to_string(),
            Box::new(move |win| -> Result<(), Box<dyn Error>> {
                let col = fltk::group::Flex::default_fill().column();

                let mut progressbar = fltk::misc::Progress::default_fill();
                progressbar.set_minimum(0.0);
                progressbar.set_maximum(100.0);
                progressbar.set_value(0.0);

                win.set_callback({
                    let cancel_flag = Arc::clone(&cancel_flag);
                    move |_win| {
                        if fltk::app::event() == fltk::enums::Event::Close {
                            println!("Send OSC window got Event::close");
                            cancel_flag.store(true, Ordering::Relaxed);
                        }
                    }
                });

                let mut cancel_btn = fltk::button::Button::default().with_label("Cancel");
                cancel_btn.set_callback({
                    let cancel_flag = Arc::clone(&cancel_flag);
                    move |_btn| {
                        println!("Send OSC window cancel button pressed");
                        cancel_flag.store(true, Ordering::Relaxed);
                    }
                });

                col.end();

                tx.send((win.clone(), progressbar))?;

                Ok(())
            })
        )
    })?;
    fltk::app::awake();

    let (win, progressbar) = rx.recv()?;

    Ok((cancel_flag, win, progressbar))
}

#[derive(Debug, Clone, Default)]
pub struct SendOSCOpts {
    pub pixfmt: PixFmt,
    pub msgs_per_second: f64,
    pub linesync: bool,
    pub rle_compression: bool,
}

pub fn send_osc(
    appmsg: &mpsc::Sender<AppMessage>,
    indexes: &Vec::<u8>,
    palette: &Vec::<quantizr::Color>,
    width: u32,
    height: u32,
    options: SendOSCOpts,
) -> Result<(), Box<dyn Error>> {
    if indexes.len() == 0 || width == 0 || height == 0 {
        return Err("indexes, width or height are 0 and they should't be".into());
    }

    if indexes.len() != (width as usize) * (height as usize) {
        return Err("width and height not matching length of indexes array".into());
    }

    let host_addr = SocketAddrV4::from_str("127.0.0.1:9002")?;
    let to_addr = SocketAddrV4::from_str("127.0.0.1:9000")?;
    let sock = UdpSocket::bind(host_addr)?;

    let sleep_time = 1.0/options.msgs_per_second;

    const OSC_PREFIX: &'static str = "/avatar/parameters/PixelSendCRT";

    const BYTES_PER_SEND: usize = 16;
    const PALETTE_COLORS_PER_SEND: usize = (BYTES_PER_SEND-1)/3; // -1 because 1 byte is used up as a command byte

    // Defines for communication with the shader
    const SETPIXEL_COMMAND: u8 = 0x80;
    const PALETTEWRITE_COMMAND: u8 = 0xc0;
    const BITDEPTH_PIXEL: u8 = 2;
    const PALETTECTRL_PIXEL: u8 = 3;
    const PALETTEWRIDX_PIXEL: u8 = 4;
    const COMPRESSIONCTRL_PIXEL: u8 = 5;

    // Get the bitdepth and whether we should be indexed or grayscale from pixfmt
    // TODO: Perhaps it would've made more sense with a regular old struct for
    //       pixfmt. then we wouldn't need to pick it apart like this.
    let (bitdepth, color) = match options.pixfmt {
        PixFmt::Auto(col) => (
            match palette.len() {
                ..=2     => 1,
                ..=4     => 2,
                ..=16    => 4,
                ..=256   => 8,
                _ => return Err("Too large palette".into()),
            },
            col,
        ),
        PixFmt::Bpp1(col) => (1, col),
        PixFmt::Bpp2(col) => (2, col),
        PixFmt::Bpp4(col) => (4, col),
        PixFmt::Bpp8(col) => (8, col),
    };

    // TODO: de-duplicate code with save_png
    // We need to do the conversion per line, because it might happen
    // that the width doesn't divide evenly when we are using 4bpp,
    // 2bpp or 1bpp modes. In that case each line must be padded out
    // some pixels.
    let mut indexes: Vec<u8> = match bitdepth {
        1 =>
            indexes
            .chunks_exact(width.try_into()?)
            .flat_map(|line|
                      line.chunks(8)
                      .map(|p|
                           p.get(0).map_or(0, |v| (v & 0b1) << 7) |
                           p.get(1).map_or(0, |v| (v & 0b1) << 6) |
                           p.get(2).map_or(0, |v| (v & 0b1) << 5) |
                           p.get(3).map_or(0, |v| (v & 0b1) << 4) |
                           p.get(4).map_or(0, |v| (v & 0b1) << 3) |
                           p.get(5).map_or(0, |v| (v & 0b1) << 2) |
                           p.get(6).map_or(0, |v| (v & 0b1) << 1) |
                           p.get(7).map_or(0, |v| (v & 0b1) << 0))
            ).collect(),
        2 =>
            indexes
            .chunks_exact(width.try_into()?)
            .flat_map(|line|
                      line.chunks(4)
                      .map(|p|
                           p.get(0).map_or(0, |v| (v & 0b11) << 6) |
                           p.get(1).map_or(0, |v| (v & 0b11) << 4) |
                           p.get(2).map_or(0, |v| (v & 0b11) << 2) |
                           p.get(3).map_or(0, |v| (v & 0b11) << 0))
            ).collect(),
        4 =>
            indexes
            .chunks_exact(width.try_into()?)
            .flat_map(|line|
                      line.chunks(2)
                      .map(|p|
                           p.get(0).map_or(0, |v| (v & 0b1111) << 4) |
                           p.get(1).map_or(0, |v| (v & 0b1111) << 0))
            ).collect(),
        8 => indexes.clone(),
        _ => panic!("This should be unreachable"),
    };

    // Optionally apply RLE compression
    if options.rle_compression {
        let mut result: Vec<u8> = Vec::new();

/*
        let mut count: u8 = 1;
        let mut current_value: u8 = indexes[0];
        for &value in &indexes[1..] {
            // determine whether or not we are at the end two bytes of a
            // BYTES_PER_SEND chunk and then simply put two bytes as is, because
            // we cannot fit an escaped RLE sequence thingamajig here
            if (result.len() % BYTES_PER_SEND) >= (BYTES_PER_SEND - 2) {
                result.push(value);
                count = 1;
                current_value = value;
                // FIXME: I think we might start counting for a value that's already been inserted here?
            } else if value == current_value {
                if let Some(x) = count.checked_add(1) {
                    count = x;
                } else {
                    // We can no longer fit the count in a single byte if we are to go on, we are forced to start anew
                    result.push(current_value);
                    result.push(current_value);
                    result.push(count);
                    count = 1;
                }
            } else {
                if count > 1 {
                    result.push(current_value);
                    result.push(current_value);
                    result.push(count);
                    current_value = value;
                    count = 1;
                } else {
                    result.push(count);
                }
            }
        }
        if count > 1 {          // FIXME: Verify that this truly works despite the logic above for handling the last two bytes in a 16-byte chunk and the handling of overflow in count
            result.push(current_value);
            result.push(current_value);
            result.push(count);
        } else {
            result.push(current_value);
        }
*/

        let mut count: u8 = 0;
        let mut current_value: Option<u8> = None;
        // let mut maybe_push = |value: u8| {
        //     if let Some(curval) = current_value {
        //         if count > 1 {
        //             result.push(curval);
        //             result.push(curval);
        //             result.push(count);
        //             current_value = Some(value);
        //             count = 1;
        //         } else if count == 1 {
        //             result.push(curval);
        //             current_value = Some(value);
        //             count = 1;
        //         } else {
        //             panic!("current_value is Some(x) but count == 0");
        //         }
        //     }
        // };
        fn maybe_push(
            result: &mut Vec<u8>,
            current_value: &mut Option<u8>,
            count: &mut u8,
            value: u8,
        ) -> bool {
            if let Some(curval) = current_value.as_mut() {
                if *count > 1u8 {
                    result.push(*curval);
                    result.push(*curval);
                    result.push(*count);
                    println!("multi push: {value}x{count}");
                    *curval = value;
                    *count = 1u8;
                } else if *count == 1u8 {
                    result.push(*curval);
                    *curval = value;
                    *count = 1u8;
                    println!("single push: {value}");
                } else {
                    panic!("current_value is Some(x) but count == 0");
                }
                true
            } else {
                false
            }
        }

        for &value in &indexes[..] {
            // determine whether or not we are at the end two bytes of a
            // BYTES_PER_SEND chunk and then simply put two bytes as is, because
            // we cannot fit an escaped RLE sequence thingamajig here
            if (result.len() % BYTES_PER_SEND) >= (BYTES_PER_SEND - 2) {
                let a = maybe_push(&mut result, &mut current_value, &mut count, value);
                println!("maybe_push: {a}");
                if !a {
                    result.push(value);
                    current_value = None;
                    count = 0;
                }
            } else if current_value == None {
                current_value = Some(value);
                count = 1;
            } else if value == current_value.expect("current_value should always be Some(x) here") {
                if let Some(x) = count.checked_add(1) {
                    count = x;
                } else {
                    // We can no longer fit the count in a single byte if we are to go on, we are forced to start anew
                    result.push(value);
                    result.push(value);
                    result.push(count);
                    // No need to set current_value here as they are identical per the value == current_value check above
                    count = 1;
                }
            } else {
                maybe_push(&mut result, &mut current_value, &mut count, value);
            }
        }
        maybe_push(&mut result, &mut current_value, &mut count, 0);

        // DEBUG OUTPUT
        println!("RLE compressed data:");
        for chunk in result.chunks(16) {
            println!("  {chunk:?}");
        }
        println!("RLE Compression ratio: {:.2}% (original length: {}, compressed length: {})",
                 ((result.len() as f64) / (indexes.len() as f64))*100.0, indexes.len(), result.len());

        indexes = result;
    }

    let (cancel_flag, win, progressbar) = create_progressbar_window(appmsg)?;

    let palette = palette.clone();
    let appmsg = appmsg.clone();
    thread::spawn(move || -> () {

        let send_bool = |var: &str, b: bool| -> Result<usize, Box<dyn Error>> {
            let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                addr: format!("{OSC_PREFIX}/{var}"),
                args: vec![OscType::Bool(b)],
            }))?;
            Ok(sock.send_to(&msg_buf, to_addr)?)
        };

        let send_int = |var: &str, i: i32| -> Result<usize, Box<dyn Error>> {
            let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                addr: format!("{OSC_PREFIX}/{var}"),
                args: vec![OscType::Int(i)],
            }))?;
            Ok(sock.send_to(&msg_buf, to_addr)?)
        };

        let mut send_clk = {
            let mut clk: bool = true;
            move || -> Result<usize, Box<dyn Error>> {
                let result = send_bool("CLK", clk);
                clk = !clk;
                result
            }
        };

        let send_cmd = |cmd: &[u8]| -> Result<(), Box<dyn Error>> {
            for n in 0..BYTES_PER_SEND {
                send_int(&format!("V{n:X}"),
                         // cmd.get(n).unwrap_or(&0u8) as i32
                         cmd.get(n).copied().unwrap_or_default() as i32
                )?;
            }
            Ok(())
        };

        let progress_message = |msg: String, progress: f64| -> () {
            println!("{}", msg);
            // Hack to avoid this thread getting held by the app main thread (currently the file choosers cause an issue for one)
            thread::spawn({
                let mut progressbar = progressbar.clone();
                move || {
                    progressbar.set_label(&msg);
                    progressbar.set_value(progress);
                    fltk::app::awake();
                }
            });
        };

        println!("palette.len(): {}, indexes.len(): {}", palette.len(), indexes.len());

        match || -> Result<(), Box<dyn Error>> {
            let duration = Duration::from_secs_f64(sleep_time);

            // Reset CLK (we can use the send_clk helper after here)
            progress_message("Reset CLK".to_string(), 0.0);
            send_bool("CLK", true)?;
            thread::sleep(duration);
            send_bool("CLK", false)?;
            thread::sleep(duration);

            // Reset pixel pos
            progress_message("Reset pixel pos".to_string(), 0.0);
            send_int("V0", 0)?;
            send_bool("Reset", true)?;
            send_clk()?;
            thread::sleep(duration);

            // Set compression mode
            progress_message((if options.rle_compression { "Enable RLE compression" } else { "Disable RLE compression" }).to_string(), 0.0);
            send_cmd(&[SETPIXEL_COMMAND,
                       COMPRESSIONCTRL_PIXEL, 0, // Controls compression. Red channel 0 is off, red channel 255 is on
                       if options.rle_compression { 255 } else { 0 },
                       0, 0, 0])?;
            send_clk()?;
            thread::sleep(duration);

            // Set BPP
            progress_message("Set BPP".to_string(), 0.0);
            send_cmd(&[SETPIXEL_COMMAND, // Set data pixel command (when Reset is active)
                       BITDEPTH_PIXEL, 0, // BITDEPTH_PIXEL at 2,0 controls BPP (red channel)
                       match bitdepth {
                           1 => 192,
                           2 => 128,
                           4 => 64,
                           8 => 0,
                           _ => panic!("This is unreachable"),
                       },
                       0, 0, 0])?;
            send_clk()?;
            thread::sleep(duration);

            // Set palette
            match color {
                Color::Indexed => {
                    progress_message("Reset palette write index".to_string(), 0.0);
                    send_cmd(&[
                        SETPIXEL_COMMAND,
                        PALETTEWRIDX_PIXEL, 0,
                        0,    // red channel: wridx 0
                        0,    // green channel: unused
                        0,    // blue channel: unused
                        0,    // alpha channel: unused
                    ])?;
                    send_clk()?;
                    thread::sleep(duration);

                    // We send 5 colors at a time
                    let palette_chunks = palette.chunks(PALETTE_COLORS_PER_SEND);
                    let palette_numchunks = palette_chunks.len();
                    for (n, chunk) in palette.chunks(5).enumerate() {
                        if cancel_flag.load(Ordering::Relaxed) {
                            println!("{}", "Send OSC thread cancelled");
                            return Ok(());
                        }

                        let mut data: [u8; BYTES_PER_SEND] = [0; BYTES_PER_SEND];
                        data[0] = PALETTEWRITE_COMMAND;
                        debug_assert!(chunk.len()*3 <= (data.len() - 1));
                        for (i, col) in chunk.iter().enumerate() {
                            // Note that what looks like an off-by-one here is actually us making sure to not overwrite
                            // PALETTEWRITE_COMMAND in the first byte
                            data[i*3 + 1] = col.r;
                            data[i*3 + 2] = col.g;
                            data[i*3 + 3] = col.b;
                        }
                        send_cmd(&data)?;
                        send_clk()?;

                        let progress: f64 = ((n as f64)/(palette_numchunks as f64))*100.0;
                        progress_message(format!("Sent palette chunk {n}/{palette_numchunks}"), progress);

                        thread::sleep(duration);
                    }

                    progress_message("Enable indexed colors".to_string(), 0.0);
                    send_cmd(&[
                        SETPIXEL_COMMAND,
                        PALETTECTRL_PIXEL, 0,
                        255,  // red channel: palette active
                        0,    // green channel: palette write mode inactive
                        0,    // blue channel: unused
                        0,    // alpha channel: unused
                    ])?;
                    send_clk()?;
                    thread::sleep(duration);
                },
                Color::Grayscale => {
                    progress_message("Set to grayscale mode".to_string(), 0.0);
                    send_cmd(&[
                        SETPIXEL_COMMAND,
                        PALETTECTRL_PIXEL, 0,
                        0,    // red channel: palette inactive
                        0,    // green channel: palette write mode not active
                        0,    // blue channel: unused/reset palette
                        0,    // alpha unused
                    ])?;
                    send_clk()?;
                    thread::sleep(duration);
                }
            }

            // Reset the reset bit
            progress_message("Clear the reset bit".to_string(), 0.0);
            send_bool("Reset", false)?;
            thread::sleep(duration);

            let now = std::time::Instant::now();

            let chunks = indexes.chunks(BYTES_PER_SEND);
            let countmax: usize = chunks.len();
            let eta = Duration::from_secs_f64((countmax as f64) * sleep_time);
            for (count, index16) in chunks.enumerate() {
                if cancel_flag.load(Ordering::Relaxed) {
                    println!("{}", "Send OSC thread cancelled");
                    return Ok(());
                }

                //dbg!(&index16);
                println!("{index16:?}");
                send_cmd(index16)?;

                send_clk()?;

                let progress = ((count as f64)/(countmax as f64))*100.0;
                let elapsed = now.elapsed();
                let msg = format!("Sent pixel chunk {}/{} {:.1}%\t ETA: {:.2?}/{:.2?}", count+1, countmax, progress, elapsed, eta);
                progress_message(msg, progress);

                thread::sleep(duration);
            }
            if !cancel_flag.load(Ordering::Relaxed) {
                println!("Send OSC thread finished sending all");
            }

            Ok(())
        }() {
            Ok(()) => (),
            Err(err) => error_alert(&appmsg, format!("send_osc background process failed: {err}"))
        };

        if let Err(err) = appmsg.send(AppMessage::DeleteWindow(win)) {
            error_alert(&appmsg, format!("send_osc background process failed while sending delete window command: {err}"));
        };
        fltk::app::awake();
    });


    Ok(())
}
