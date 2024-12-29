pub mod mq;

use fltk::{app, frame::Frame, enums::*, image::*, prelude::*, window::Window, group::*, button::*, valuator::*, dialog, input::*, menu};
use std::error::Error;
use std::path::PathBuf;
use std::iter::zip;
use rayon::prelude::*;
use std::thread;
use std::panic;
use std::string::String;
use image::{self, imageops};
use std::sync::mpsc;
// use std::sync::OnceLock;

#[allow(unused_macros)]
macro_rules! function {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        name.strip_suffix("::f").unwrap()
    }}
}

#[allow(dead_code)]
fn print_type_of<T>(_: &T) {
    println!("{}", std::any::type_name::<T>());
}

#[derive(Debug, Clone)]
pub enum Color {
    Grayscale,
    Indexed,
}

#[derive(Debug, Clone)]
pub enum PixFmt {
    Bpp1(Color),
    Bpp4(Color),
    Bpp5(Color),
    Bpp8(Color),
}

pub enum AppMessage {
    SetTitle(String),
    Alert(String),
    // TODO: instead of passing a closure, just have this return the window to the sender on a sender-provided channel?
    //       Since I think calling window.show() might need to be from the main thread as well this will probably require another message
    //       to show a window
    CreateWindow(i32, i32, String, Box<dyn FnOnce(&mut Window) -> Result<(), Box<dyn Error>> + Send + Sync>),
    DeleteWindow(Window),
}

#[derive(Debug, Clone)]
pub enum BgMessage{
    LoadImage(PathBuf),
    UpdateImage{
        no_quantize: bool,
        grayscale: bool,
        grayscale_output: bool,
        reorder_palette: bool,
        maxcolors: i32,
        dithering: f32,
        scaling: bool,
        scale: usize,
        multiplier: u8,
    },
    ClearImage,
    SendOSC{
        speed: f64,
        pixfmt: PixFmt,
    },
    Quit,
}

impl BgMessage {
    fn is_update(&self) -> bool {
        match self {
            BgMessage::UpdateImage{..} => true,
            _ => false
        }
    }
}

fn get_file() -> Option<PathBuf> {
    let mut nfc = dialog::NativeFileChooser::new(dialog::FileDialogType::BrowseFile);

    match nfc.try_show() {
        Err(err) => {
            let msg = format!("Failed to show NativeFileChooser: {err:?}");
            eprintln!("{}", msg);
            dialog::alert_default(&msg);
            None
        },
        Ok(a) => match a {
            dialog::NativeFileChooserAction::Success => {
                let name = nfc.filename();
                if name.as_os_str().is_empty() {
                    dialog::alert_default("Please specify a file!");
                    None
                } else {
                    Some(name)
                }
            }
            dialog::NativeFileChooserAction::Cancelled => None,
        }
    }
}

fn scale_image(bytes: &[u8],
               width: usize, height: usize,
               nwidth: usize, nheight: usize) -> Result<(Vec<u8>, usize, usize), Box<dyn Error>> {
    assert!(bytes.len() == width * height * 4); // RGBA format assumed

    let img = image::RgbaImage::from_raw(width as u32, height as u32, bytes.to_vec()).ok_or("bytes not big enough for width and height")?;
    let dimg = image::DynamicImage::from(img);
    let newimg = dimg.resize_to_fill(nwidth as u32, nheight as u32, imageops::FilterType::Lanczos3).into_rgba8();

    let (w, h): (u32, u32) = newimg.dimensions();
    Ok((newimg.into_raw(), w as usize, h as usize))
}

fn sharedimage_to_bytes(image : &SharedImage, grayscale : bool) -> Result<(Vec<u8>, usize, usize), Box<dyn Error>> {
    // let bytes : Vec<u8> = image.to_rgb_image()?.convert(ColorDepth::L8)?.convert(ColorDepth::Rgba8)?.to_rgb_data();

    let mut rgbimage = image.to_rgb_image()?;
    if grayscale {
        rgbimage = rgbimage.convert(ColorDepth::L8)?;
    }

    let bytes : Vec<u8> = rgbimage.convert(ColorDepth::Rgba8)?.to_rgb_data();
    println!("bytes.len(): {}", bytes.len());
    let width : usize = rgbimage.data_w().try_into()?;
    let height : usize = rgbimage.data_h().try_into()?;

    Ok((bytes, width, height))
}

// Ugly hack to workaround quantizr not being really made for
// grayscale by reordering the pallette, which means that the indexes
// should be able to be used without the palette as a sort-of
// grayscale image
fn reorder_palette_by_brightness(indexes : &Vec<u8>, palette : &quantizr::Palette) -> (Vec<u8>, Vec<quantizr::Color>)
{
    let mut permutation : Vec<usize> = (0..(palette.count as usize)).collect();
    permutation.sort_by_key(|&i| {
        let c = palette.entries[i];
        let (r,g,b) = (c.r as i32, c.g as i32, c.b as i32);
        r + g + b
    });

    let new_palette : Vec<quantizr::Color> =
        permutation.iter()
        .map(|&i| palette.entries[i])
        .collect();

    // Trying out fancy rayon parallel iterators
    // TODO: use a HashMap? or just an array that gets the reverse mapping
    let new_indexes : Vec<u8> = indexes.par_iter().map(
        |ic| permutation.iter().position(|&r| r == *ic as usize).unwrap_or_default() as u8
    ).collect();

    (new_indexes, new_palette)
}

// Make it a paletted image
fn quantize_image(bytes : &Vec<u8>,
                  width : usize, height : usize,
                  max_colors : i32,
                  dithering_level : f32,
                  reorder_palette : bool) -> Result<(Vec<u8>, Vec<quantizr::Color>), Box<dyn Error>> {

    // Need to make sure that input buffer is matching width and
    // height params for an RGBA buffer (4 bytes per pixel)
    assert!(width * height * 4 == bytes.len());

    let qimage = quantizr::Image::new(bytes, width, height)?;
    let mut qopts = quantizr::Options::default();
    qopts.set_max_colors(max_colors)?;

    let mut result = quantizr::QuantizeResult::quantize(&qimage, &qopts);
    result.set_dithering_level(dithering_level)?;

    let mut indexes = vec![0u8; width*height];
    result.remap_image(&qimage, indexes.as_mut_slice())?;
    assert!(width * height == indexes.len());

    let palette = result.get_palette();

    let result: (Vec<u8>, Vec<quantizr::Color>) = if reorder_palette {
        reorder_palette_by_brightness(&indexes, palette)
    } else {
        (indexes, palette.entries[0..(palette.count as usize)].to_vec())
    };

    Ok(result)
}

// Turn the quantized thing back into RGB for display
fn quantized_image_to_rgbimage(indexes : &Vec<u8>,
                               palette : &Vec<quantizr::Color>,
                               width : usize,
                               height : usize,
                               grayscale_output : bool) -> Result<RgbImage, Box<dyn Error>> {
    assert!(width * height == indexes.len());

    let mut fb: Vec<u8> = vec![0u8; indexes.len() * 4];
    if !grayscale_output {
        for (&index, pixel) in zip(indexes, fb.chunks_exact_mut(4)) {
            let c : quantizr::Color = palette[index as usize];
            pixel.copy_from_slice(&[c.r, c.g, c.b, c.a]);
        }
    } else {
        for (&index, pixel) in zip(indexes, fb.chunks_exact_mut(4)) {
            let max: f64 = (palette.len() - 1) as f64;
            let index: u8 = (index as f64*(255.0/max)).round() as u8;
            pixel.copy_from_slice(&[index, index, index, 255]);
        }
    }

    Ok(RgbImage::new(&fb, width as i32, height as i32, ColorDepth::Rgba8)?)
}

fn palette_to_rgbimage(palette: &[quantizr::Color], grayscale_output: bool) -> Result<RgbImage, Box<dyn Error>> {
    let mut fb: Vec<u8> = vec![0u8; palette.len() * 4];
    let width: i32 = 1;
    let height: i32 = palette.len().try_into()?;

    if !grayscale_output {
        for (&col, pixel) in zip(palette, fb.chunks_exact_mut(4)) {
            pixel.copy_from_slice(&[col.r, col.g, col.b, 255]);
        }
    } else {
        let range: std::ops::Range<u8> = 0..((palette.len()-1) as u8);
        for (i, pixel) in zip(range, fb.chunks_exact_mut(4)) {
            let max: f64 = (palette.len()-1) as f64;
            let val: u8 = (i as f64 * (255.0/max)).round() as u8;
            pixel.copy_from_slice(&[val, val, val, 255]);
        }
    }

    Ok(RgbImage::new(&fb, width, height, ColorDepth::Rgba8)?)
}

fn print_err<T, E: Error>(result: Result<T, E>) -> () {
    match result {
        Ok(_t) => (),
        Err(err) => eprintln!("{}", err),
    }
}

fn send_osc(appmsg: &mpsc::Sender<AppMessage>, indexes: &Vec::<u8>, palette: &Vec::<quantizr::Color>, msgs_per_second: f64) -> Result<(), Box<dyn Error>> {
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
    let (tx, rx) = mpsc::channel::<(Window, fltk::misc::Progress)>();

    // New windows need to be created on the main thread, so we message the main thread
    appmsg.send({
        let cancel_flag = Arc::clone(&cancel_flag);
        AppMessage::CreateWindow(
            400, 200, "Sending OSC".to_string(),
            Box::new(move |win| -> Result<(), Box<dyn Error>> {
                let col = Flex::default_fill().column();

                let mut progressbar = fltk::misc::Progress::default_fill();
                progressbar.set_minimum(0.0);
                progressbar.set_maximum(100.0);
                progressbar.set_value(0.0);

                win.set_callback({
                    let cancel_flag = Arc::clone(&cancel_flag);
                    move |_win| {
                        if app::event() == Event::Close {
                            println!("Send OSC window got Event::close");
                            cancel_flag.store(true, Ordering::Relaxed);
                        }
                    }
                });

                let mut cancel_btn = Button::default().with_label("Cancel");
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

            let mut msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                addr: format!("{OSC_PREFIX}/Reset"),
                args: vec![OscType::Bool(true)],
            }))?;
            sock.send_to(&msg_buf, to_addr)?;

            thread::sleep(duration);

            msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                addr: format!("{OSC_PREFIX}/Reset"),
                args: vec![OscType::Bool(false)],
            }))?;
            sock.send_to(&msg_buf, to_addr)?;

            msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                addr: format!("{OSC_PREFIX}/CLK"),
                args: vec![OscType::Bool(false)],
            }))?;
            sock.send_to(&msg_buf, to_addr)?;

            thread::sleep(duration);

            let now = std::time::Instant::now();

            let mut clk: bool = false;
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
            Err(err) => {
                let msg = format!("send_osc background process failed: {err}");
                eprintln!("{}", msg);
                print_err(appmsg.send(AppMessage::Alert(msg)));
                fltk::app::awake();
            },
        };
    });


    Ok(())
}

fn enable_send_osc_button(active: bool) -> Result<(), String> {
    let mut send_osc_btn: Button = app::widget_from_id("send_osc_btn").ok_or("widget_from_id fail")?;
    if active {
        send_osc_btn.activate();
    } else {
        send_osc_btn.deactivate();
    }
    fltk::app::awake();
    Ok(())
}

fn start_background_process(appmsg_sender: &mpsc::Sender<AppMessage>) -> (thread::JoinHandle<()>, mq::MessageQueueSender<BgMessage>) {
    let (sender, receiver) = mq::mq::<BgMessage>();

    let appmsg = appmsg_sender.clone();
    let sender_return = sender.clone();

    let joinhandle: thread::JoinHandle<()> = thread::spawn(move || -> () {
        let mut sharedimage: Option<SharedImage> = None;

        let mut indexes_and_palette: Option<(Vec<u8>, Vec<quantizr::Color>)> = None;

        loop {
            let recvres = receiver.recv();
            let Ok(msg) = recvres else {
                let s = format!("Error receiving from mq::MessageQueueReceiver: {}", recvres.unwrap_err());
                eprintln!("{}", s);
                print_err(appmsg.send(AppMessage::Alert(s)));
                fltk::app::awake();
                continue;
            };

            match msg {
                BgMessage::Quit => {
                    break;
                },
                BgMessage::LoadImage(path) => {
                    match || -> Result<(), String> {
                        // TODO: Switch to using the image crate to load and also to grayscale. Also evaluate it at dithering?
                        //       We should only convert to FLTK image format at the very end
                        let image = SharedImage::load(&path)
                            .map_err(|err| format!("Image load for image {path:?} failed: {err:}"))?;

                        sharedimage = Some(image);
                        println!("Loaded image {path:?}");

                        let pathstr = path.to_string_lossy();
                        {
                            let mut frame: Frame = app::widget_from_id("frame").ok_or("widget_from_id fail")?;
                            frame.set_label(&pathstr);
                            frame.changed();
                            frame.redraw();
                        }

                        appmsg.send(AppMessage::SetTitle(pathstr.to_string())).
                            map_err(|err| format!("Send error: {err}"))?;
                        fltk::app::awake();

                        send_updateimage(&appmsg, &sender);

                        println!("Finished LoadImage for {path:?}");
                        Ok(())
                    }() {
                        Ok(()) => (),
                        Err(errmsg) => {
                            let msg = format!("LoadImage fail:\n{errmsg}");
                            eprintln!("{}", msg);
                            print_err(appmsg.send(AppMessage::Alert(msg)));
                            print_err(sender.send(BgMessage::ClearImage));
                            fltk::app::awake();
                        }
                    }
                },
                BgMessage::ClearImage => {
                    match || -> Result<(), String> {
                        let mut frame: Frame = app::widget_from_id("frame").ok_or("widget_from_id fail")?;
                        let mut palette_frame: Frame = app::widget_from_id("palette_frame").ok_or("widget_from_id fail")?;

                        indexes_and_palette = None::<(Vec<u8>, Vec<quantizr::Color>)>;

                        sharedimage = None::<SharedImage>;

                        frame.set_image(None::<SharedImage>);
                        frame.set_label("Clear");
                        frame.changed();

                        palette_frame.set_image(None::<RgbImage>);
                        palette_frame.changed();

                        enable_send_osc_button(false)?;

                        appmsg.send(AppMessage::SetTitle("Clear".to_string()))
                            .map_err(|err| format!("Send error: {err}"))?;
                        fltk::app::awake();

                        Ok(())
                    }() {
                        Ok(()) => (),
                        Err(errmsg) => {
                            let msg = format!("ClearImage fail:\n{errmsg}");
                            eprintln!("{}", msg);
                            print_err(appmsg.send(AppMessage::Alert(msg)));
                            fltk::app::awake();
                        }
                    };
                },
                BgMessage::UpdateImage{
                    no_quantize,
                    grayscale,
                    grayscale_output,
                    reorder_palette,
                    maxcolors,
                    dithering,
                    scaling,
                    scale,
                    multiplier,
                } => {
                    match || -> Result<(), String> {
                        enable_send_osc_button(false)?;

                        let Some(ref image) = sharedimage else {
                            eprintln!("No image loaded");
                            return Ok(());
                        };

                        let now = std::time::Instant::now();

                        if !no_quantize {
                            let mut bytes: Vec<u8>;
                            let mut width: usize;
                            let mut height: usize;

                            (bytes, width, height) = sharedimage_to_bytes(&image, grayscale)
                                .map_err(|err| format!("sharedimage_to_bytes failed: {err:?}"))?;

                            if scaling {
                                (bytes, width, height) = scale_image(&bytes, width, height, scale, scale)
                                    .map_err(|err| format!("scale_image failed: {err:?}"))?;
                            }

                            let (indexes, palette) = quantize_image(
                                &bytes, width, height,
                                maxcolors,
                                dithering,
                                reorder_palette,
                            ).map_err(|err| format!("Quantization failed: {err:?}"))?;

                            let mut rgbimage = quantized_image_to_rgbimage(
                                &indexes, &palette,
                                width, height,
                                grayscale_output,
                            ).map_err(|err| format!("Conversion to rgbimage failed: {err:?}"))?;

                            if scaling {
                                rgbimage.scale((width as i32) * (multiplier as i32),
                                               (height as i32) * (multiplier as i32),
                                               true, true); // Display pixelly image larger
                            }

                            {
                                let mut frame: Frame = app::widget_from_id("frame").ok_or("widget_from_id fail")?;
                                let mut palette_frame: Frame = app::widget_from_id("palette_frame").ok_or("widget_from_id fail")?;

                                frame.set_image(Some(rgbimage));
                                frame.changed();
                                frame.redraw();

                                let palette_rgbimage = palette_to_rgbimage(&palette, grayscale_output)
                                    .map_err(|err| format!("Couldn't generate palette RgbImage: {err:?}"))?;
                                palette_frame.set_image_scaled(Some(palette_rgbimage));
                                palette_frame.changed();
                                palette_frame.redraw();
                            }

                            indexes_and_palette = Some((indexes, palette));
                            enable_send_osc_button(true)?;
                        } else {
                            let mut frame: Frame = app::widget_from_id("frame").ok_or("widget_from_id fail")?;
                            frame.set_image(Some(image.clone()));
                            frame.changed();
                            frame.redraw();

                            // TODO: there should be a fallback here maybe
                            indexes_and_palette = None;
                            enable_send_osc_button(false)?;
                        }

                        fltk::app::awake();

                        println!("Finished updating image (took {:.2?})", now.elapsed());

                        Ok(())
                    }() {
                        Ok(()) => (),
                        Err(errmsg) => {
                            let msg = format!("UpdateImage fail:\n{errmsg}");
                            eprintln!("{}", msg);
                            print_err(appmsg.send(AppMessage::Alert(msg)));
                            print_err(sender.send(BgMessage::ClearImage));
                            fltk::app::awake();
                        },
                    };
                },
                BgMessage::SendOSC{
                    speed,
                    pixfmt,
                } => {
                    println!("SendOSC{{speed: {speed:?}, pixfmt: {pixfmt:?}}}");
                    match || -> Result<(), String> {
                        let (indexes, palette) = indexes_and_palette.as_ref()
                            .ok_or("Indexes and palette not generated yet")?;
                        send_osc(&appmsg, indexes, palette, speed)
                            .map_err(|err| format!("send_osc failed: {err}"))?;
                        Ok(())
                    }() {
                        Ok(()) => (),
                        Err(errmsg) => {
                            let msg = format!("SendOSC fail:\n{errmsg}");
                            eprintln!("{}", msg);
                            print_err(appmsg.send(AppMessage::Alert(msg)));
                            fltk::app::awake();
                        },
                    };
                },
            };
        }

        println!("BG Process Finished");
    });

    (joinhandle, sender_return)
}

fn send_updateimage(appmsg: &mpsc::Sender<AppMessage>, bg: &mq::MessageQueueSender::<BgMessage>) -> () {
    match || -> Result<(), String> {
        let no_quantize_toggle: CheckButton = app::widget_from_id("no_quantize_toggle").ok_or("widget_from_id fail")?;
        let grayscale_toggle: CheckButton = app::widget_from_id("grayscale_toggle").ok_or("widget_from_id fail")?;
        let grayscale_output_toggle: CheckButton = app::widget_from_id("grayscale_output_toggle").ok_or("widget_from_id fail")?;
        let reorder_palette_toggle: CheckButton = app::widget_from_id("reorder_palette_toggle").ok_or("widget_from_id fail")?;
        let maxcolors_slider: HorValueSlider = app::widget_from_id("maxcolors_slider").ok_or("widget_from_id fail")?;
        let dithering_slider: HorValueSlider = app::widget_from_id("dithering_slider").ok_or("widget_from_id fail")?;
        let scaling_toggle: CheckButton = app::widget_from_id("scaling_toggle").ok_or("widget_from_id fail")?;
        let scale_input: IntInput = app::widget_from_id("scale_input").ok_or("widget_from_id fail")?;
        let multiplier_menubutton: menu::MenuButton = app::widget_from_id("multiplier_menubutton").ok_or("widget_from_id fail")?;

        let msg = BgMessage::UpdateImage{
            no_quantize: no_quantize_toggle.is_checked(),
            grayscale: grayscale_toggle.is_checked(),
            grayscale_output: grayscale_output_toggle.is_checked(),
            reorder_palette: reorder_palette_toggle.is_checked(),
            scaling: scaling_toggle.is_checked(),
            maxcolors: maxcolors_slider.value() as i32,
            dithering: dithering_slider.value() as f32,
            scale: {
                let value = scale_input.value();
                value.parse()
                    .map_err(|err| format!("Couldn't parse scale {value:?}: {err}"))?
            },
            multiplier: {
                match || -> Result<_, String> {
                    let choice: String = multiplier_menubutton.choice()
                        .ok_or("No choice selected in multiplier menubutton")?;
                    let choice = choice.strip_suffix("x")
                        .ok_or_else(|| format!("No x suffix in multiplier menubutton choice: {choice:?}"))?;
                    let multiplier = choice.parse()
                        .map_err(|err| format!("Couldn't parse multiplier {choice:?}: {err}"))?;
                    Ok(multiplier)
                }() {
                    Ok(res) => res,
                    Err(msg) => {
                        eprintln!("{}", msg);
                        1
                    },
                }
            },
        };

        bg.send_or_replace_if(BgMessage::is_update, msg)
            .map_err(|err| format!("Send error: {err}"))?;

        Ok(())
    }() {
        Ok(()) => (),
        Err(errmsg) => {
            let msg = format!("{}:\n{}", function!(), errmsg);
            eprintln!("{}", msg);
            print_err(appmsg.send(AppMessage::Alert(msg)));
            fltk::app::awake();
        },
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let app = app::App::default().with_scheme(app::Scheme::Gleam);
    // let app = app::App::default().with_scheme(app::Scheme::Oxy);
    let mut wind = Window::default().with_size(1600, 900);

    let mut row = Flex::default_fill().row();
    // row.set_margin(20);
    row.set_spacing(20);
    let mut frame = Frame::default_fill().with_id("frame");
    frame.set_frame(FrameType::DownBox);

    let palette_frame = Frame::default_fill().with_id("palette_frame");
    // palette_frame.set_frame(FrameType::DownBox);

    let mut col = Flex::default_fill().column();
    col.set_margin(20);
    col.set_spacing(20);
    let mut openbtn = Button::default().with_label("Open");
    let mut clearbtn = Button::default().with_label("Clear");

    let mut no_quantize_toggle = CheckButton::default().with_label("Disable quantization").with_id("no_quantize_toggle");
    let mut grayscale_toggle = CheckButton::default().with_label("Grayscale the image\nbefore converting").with_id("grayscale_toggle");
    let mut grayscale_output_toggle = CheckButton::default().with_label("Output the palette\nindexes as grayscale").with_id("grayscale_output_toggle");
    let mut reorder_palette_toggle = CheckButton::default().with_label("Sort palette").with_id("reorder_palette_toggle");
    reorder_palette_toggle.set_checked(true);

    let mut maxcolors_slider = HorValueSlider::default().with_label("Max Colors").with_id("maxcolors_slider");
    maxcolors_slider.set_range(2.0, 256.0);
    maxcolors_slider.set_step(1.0, 1);
    maxcolors_slider.set_value(16.0);

    let mut dithering_slider = HorValueSlider::default().with_label("Dithering Level").with_id("dithering_slider");
    dithering_slider.set_range(0.0, 1.0);
    dithering_slider.set_value(1.0);

    let mut scaling_toggle = CheckButton::default().with_label("Enable scaling").with_id("scaling_toggle");
    scaling_toggle.set_checked(true);
    const SCALE_DEFAULT: &'static str = "128";
    let mut scale_input = IntInput::default().with_size(0, 40).with_label("Scale (NxN)").with_id("scale_input");
    // scale_input.set_trigger(CallbackTrigger::Changed);
    scale_input.set_trigger(CallbackTrigger::EnterKey);
    scale_input.set_value(SCALE_DEFAULT);
    scale_input.set_maximum_size(1024);

    // TODO: Is this even the right widget for this? Feels wierd to have to update the label.
    let mut multiplier_menubutton = menu::MenuButton::default()
        .with_label("Display scale multiplier: 5x")
        .with_id("multiplier_menubutton");
    multiplier_menubutton.add_choice("1x\t|2x\t|3x\t|4x\t|5x\t|6x\t|7x\t|8x");
    multiplier_menubutton.set_value(4);

    // TODO: a separator to separate the OSC stuff

    const OSC_SPEED_DEFAULT: f64 = 2.0;
    let mut send_osc_btn = Button::default().with_label("Send OSC").with_id("send_osc_btn");
    send_osc_btn.deactivate();
    let mut osc_speed_slider = HorValueSlider::default().with_label("OSC updates/second").with_id("osc_speed_slider");
    osc_speed_slider.set_range(0.5, 20.0);
    osc_speed_slider.set_step(0.5, 1);
    osc_speed_slider.set_value(OSC_SPEED_DEFAULT);

    row.fixed(&palette_frame, 50);
    row.fixed(&col, 300);
    col.fixed(&openbtn, 50);
    col.fixed(&clearbtn, 50);
    col.fixed(&no_quantize_toggle, 30);
    col.fixed(&grayscale_toggle, 30);
    col.fixed(&grayscale_output_toggle, 20);
    col.fixed(&reorder_palette_toggle, 20);
    col.fixed(&maxcolors_slider, 30);
    col.fixed(&dithering_slider, 30);
    col.fixed(&scaling_toggle, 30);
    col.fixed(&scale_input, 30);
    col.fixed(&multiplier_menubutton, 30);
    col.fixed(&send_osc_btn, 50);
    col.fixed(&osc_speed_slider, 30);

    let (appmsg, appmsg_recv) = mpsc::channel::<AppMessage>();
    let (joinhandle, bg) = start_background_process(&appmsg);

    openbtn.set_callback({
        let bg = bg.clone();
        let appmsg = appmsg.clone();
        move |_| {
            println!("Open button pressed");

            let Some(path) = get_file() else {
                eprintln!("No file selected/cancelled");
                return;
            };

            match || -> Result<(), Box<dyn Error>> {
                bg.send_or_replace_if(BgMessage::is_update, BgMessage::LoadImage(path))?;
                Ok(())
            }() {
                Ok(()) => (),
                Err(err) => {
                    let msg = format!("Openbtn fail: {}", err);
                    eprintln!("{}", msg);
                    print_err(appmsg.send(AppMessage::Alert(msg)));
                }
            }
        }
    });

    clearbtn.set_callback({
        let bg = bg.clone();
        let appmsg = appmsg.clone();
        move |_| {
            println!("Clear button pressed");

            let sendresult = bg.send_or_replace_if(BgMessage::is_update, BgMessage::ClearImage);
            if sendresult.is_err() {
                let msg = format!("{}", sendresult.unwrap_err());
                eprintln!("{}", msg);
                print_err(appmsg.send(AppMessage::Alert(msg)));
                fltk::app::awake();
            }
        }
    });

    no_quantize_toggle.set_callback(     { let a = appmsg.clone(); let b = bg.clone(); move |_| { send_updateimage(&a, &b); } });
    grayscale_toggle.set_callback(       { let a = appmsg.clone(); let b = bg.clone(); move |_| { send_updateimage(&a, &b); } });
    grayscale_output_toggle.set_callback({ let a = appmsg.clone(); let b = bg.clone(); move |_| { send_updateimage(&a, &b); } });
    reorder_palette_toggle.set_callback( { let a = appmsg.clone(); let b = bg.clone(); move |_| { send_updateimage(&a, &b); } });
    maxcolors_slider.set_callback(       { let a = appmsg.clone(); let b = bg.clone(); move |_| { send_updateimage(&a, &b); } });
    dithering_slider.set_callback(       { let a = appmsg.clone(); let b = bg.clone(); move |_| { send_updateimage(&a, &b); } });
    scaling_toggle.set_callback(         { let a = appmsg.clone(); let b = bg.clone(); move |_| { send_updateimage(&a, &b); } });
    scale_input.set_callback({
        let bg = bg.clone();
        let appmsg = appmsg.clone();
        move |i| {
            let value = i.value();
            println!("scale_input: i.value() = {:?}, i.active={:?}", i.value(), i.active());
            if value.len() > 0 {
                send_updateimage(&appmsg, &bg);
            } else {
                i.set_value(SCALE_DEFAULT);
            }
        }
    });
    multiplier_menubutton.set_callback({
        let bg = bg.clone();
        let appmsg = appmsg.clone();
        move |m| {
            println!("multiplier_menubutton: m.choice() = {:?}", m.choice());
            m.set_label(&format!("Display scale multiplier: {}", m.choice().unwrap_or("NOT SET".to_string())));
            send_updateimage(&appmsg, &bg);
        }
    });

    send_osc_btn.set_callback({
        let bg = bg.clone();
        let appmsg = appmsg.clone();
        move |_| {
            let speed = osc_speed_slider.value();
            let result = bg.send(
                BgMessage::SendOSC{
                    speed: speed,
                    pixfmt: PixFmt::Bpp8(Color::Grayscale),
                }
            );
            if result.is_err() {
                let msg = format!("Send OSC button error: {:?}", result);
                eprintln!("{}", msg);
                print_err(appmsg.send(AppMessage::Alert(msg)));
                fltk::app::awake();
            }
        }
    });

    col.end();
    row.end();
    wind.end();

    wind.make_resizable(true);
    wind.show();

    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new({
        move |panic_info| {
            // invoke the default handler, but then display an alert message
            orig_hook(panic_info);
            print_err(appmsg.send(AppMessage::Alert(format!("{panic_info}"))));
            fltk::app::awake();
        }
    }));

    // app.run()?;

    while app.wait() {
        match appmsg_recv.try_recv() {
            Ok(msg) => match msg {
                AppMessage::Alert(s)    => dialog::alert_default(&s),
                AppMessage::SetTitle(s) => wind.set_label(&s),
                AppMessage::CreateWindow(width, height, title, f) => {
                    println!("Creating window {title}({width},{height})");
                    let mut wind = Window::default().with_size(width, height);
                    wind.set_label(&title);
                    let res = f(&mut wind);
                    wind.end();
                    wind.show();
                    if let Err(err) = res {
                        let msg = format!("CreateWindow error: {err}");
                        eprintln!("{}", msg);
                        dialog::alert_default(&msg);
                        // Something failed, delete the window
                        Window::delete(wind);
                    }
                },
                AppMessage::DeleteWindow(mut window) => {
                    window.hide();
                    Window::delete(window);
                },
            },
            Err(mpsc::TryRecvError::Empty) => (),
            Err(err) => eprintln!("Channel error: {err}"),
        }
    }

    println!("App finished");

    bg.send_or_replace(BgMessage::Quit)?;
    joinhandle.join().map_err(|err| format!("Joining failed: {err:?}"))?;
    println!("BG Thread joined");

    Ok(())
}
