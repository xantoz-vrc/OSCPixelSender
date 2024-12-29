use crate::AppMessage;
use crate::utility::error_alert;

use fltk::prelude::*;
use std::thread;
use std::error::Error;
use std::sync::mpsc;
use std::string::ToString;
use std::str::FromStr;

// TODO: To cut down on repetition: Either use something like strum. Or make your own macro maybe?

#[derive(Debug, Clone, Default)]
pub enum Color {
    #[default]
    Grayscale,
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

#[derive(Debug, Clone)]
pub enum PixFmt {
    Bpp1(Color),
    Bpp2(Color),
    Bpp4(Color),
    Bpp8(Color),
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
            "Bpp1" => Ok(Self::Bpp1(Default::default())),
            "Bpp2" => Ok(Self::Bpp2(Default::default())),
            "Bpp4" => Ok(Self::Bpp4(Default::default())),
            "Bpp8" => Ok(Self::Bpp8(Default::default())),
            "Bpp1(Grayscale)" => Ok(Self::Bpp1(Color::Grayscale)),
            "Bpp2(Grayscale)" => Ok(Self::Bpp2(Color::Grayscale)),
            "Bpp4(Grayscale)" => Ok(Self::Bpp4(Color::Grayscale)),
            "Bpp8(Grayscale)" => Ok(Self::Bpp8(Color::Grayscale)),
            "Bpp1(Indexed)" => Ok(Self::Bpp1(Color::Indexed)),
            "Bpp2(Indexed)" => Ok(Self::Bpp2(Color::Indexed)),
            "Bpp4(Indexed)" => Ok(Self::Bpp4(Color::Indexed)),
            "Bpp8(Indexed)" => Ok(Self::Bpp8(Color::Indexed)),
            _ => Err(format!("Couldn't parse as {}: {}", std::any::type_name::<Self>(), s)),
        }
    }
}

impl PixFmt {
    pub const VALUES: [PixFmt; 8] = [
        PixFmt::Bpp1(Color::Grayscale),
        PixFmt::Bpp2(Color::Grayscale),
        PixFmt::Bpp4(Color::Grayscale),
        PixFmt::Bpp8(Color::Grayscale),
        PixFmt::Bpp1(Color::Indexed),
        PixFmt::Bpp2(Color::Indexed),
        PixFmt::Bpp4(Color::Indexed),
        PixFmt::Bpp8(Color::Indexed),
    ];

    pub fn into_iter() -> core::array::IntoIter<PixFmt, 8> {
        Self::VALUES.into_iter()
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SendOSCOpts {
    linesync: bool,
}

pub fn send_osc(
    appmsg: &mpsc::Sender<AppMessage>,
    indexes: &Vec::<u8>,
    palette: &Vec::<quantizr::Color>,
    msgs_per_second: f64
) -> Result<(), Box<dyn Error>> {
    extern crate rosc;

    use rosc::encoder;
    use rosc::{OscMessage, OscPacket, OscType};
    use std::net::{SocketAddrV4, UdpSocket};
    use std::str::FromStr;
    use std::time::Duration;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let host_addr = SocketAddrV4::from_str("127.0.0.1:9002")?;
    let to_addr = SocketAddrV4::from_str("127.0.0.1:9000")?;
    let sock = UdpSocket::bind(host_addr)?;

    let sleep_time = 1.0/msgs_per_second;

    const OSC_PREFIX: &'static str = "/avatar/parameters/PixelSendCRT";

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

    let (win, mut progressbar) = rx.recv()?;

    let appmsg = appmsg.clone();
    let indexes = indexes.clone();
    let palette = palette.clone();

    thread::spawn(move || -> () {

        println!("palette.len(): {}, indexes.len(): {}", palette.len(), indexes.len());

        match || -> Result<(), Box<dyn Error>> {
            let duration = Duration::from_secs_f64(sleep_time);

            let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                addr: format!("{OSC_PREFIX}/CLK"),
                args: vec![OscType::Bool(true)],
            }))?;
            sock.send_to(&msg_buf, to_addr)?;

            thread::sleep(duration);

            let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                addr: format!("{OSC_PREFIX}/CLK"),
                args: vec![OscType::Bool(false)],
            }))?;
            sock.send_to(&msg_buf, to_addr)?;

            thread::sleep(duration);

            let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                addr: format!("{OSC_PREFIX}/Reset"),
                args: vec![OscType::Bool(true)],
            }))?;
            sock.send_to(&msg_buf, to_addr)?;

            thread::sleep(duration);

            let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                addr: format!("{OSC_PREFIX}/Reset"),
                args: vec![OscType::Bool(false)],
            }))?;
            sock.send_to(&msg_buf, to_addr)?;

            thread::sleep(duration);

            let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                addr: format!("{OSC_PREFIX}/CLK"),
                args: vec![OscType::Bool(false)],
            }))?;
            sock.send_to(&msg_buf, to_addr)?;

            thread::sleep(duration);

            let now = std::time::Instant::now();

            let mut clk: bool = true;
            let chunks = indexes.chunks_exact(16);
            let mut count: usize = 0;
            let countmax: usize = chunks.len();
            let eta = Duration::from_secs_f64((countmax as f64) * sleep_time);
            for index16 in chunks {
                if cancel_flag.load(Ordering::Relaxed) {
                    println!("{}", "Send OSC thread cancelled");
                    break;
                }

                dbg!(&index16);

                let mut n: u32 = 0;
                for index in index16 {
                    let valuename = format!("V{:X}", n);
                    n += 1;
                    let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                        addr: format!("{OSC_PREFIX}/{valuename}"),
                        args: vec![OscType::Int(*index as i32)],
                    }))?;
                    sock.send_to(&msg_buf, to_addr)?;
                }

                let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                    addr: format!("{OSC_PREFIX}/CLK"),
                    args: vec![OscType::Bool(clk)],
                }))?;
                sock.send_to(&msg_buf, to_addr)?;

                clk = !clk;
                count += 1;

                let progress = ((count as f64)/(countmax as f64))*100.0;
                let elapsed = now.elapsed();
                let msg = format!("Sent pixel chunk {}/{} {:.1}%\t ETA: {:.2?}/{:.2?}", count, countmax, progress, elapsed, eta);
                println!("{}", msg);
                progressbar.set_label(&msg);
                progressbar.set_value(progress);

                fltk::app::awake();

                thread::sleep(duration);
            }
            println!("Send OSC thread finished sending all");

            appmsg.send(AppMessage::DeleteWindow(win))?;
            fltk::app::awake();

            Ok(())
        }() {
            Ok(()) => (),
            Err(err) => error_alert(&appmsg, format!("send_osc background process failed: {err}"))
        };
    });


    Ok(())
}
