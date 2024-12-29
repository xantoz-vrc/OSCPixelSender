use fltk::{app, frame::Frame, enums::CallbackTrigger, enums::FrameType, image::*, enums::ColorDepth, prelude::*, window::Window, group::*, button::*, valuator::*, dialog, input::*, menu};
use std::error::Error;
use std::path::PathBuf;
use std::iter::zip;
use rayon::prelude::*;
use std::thread;
use std::panic;
use std::string::String;
use image::{self, imageops};
use std::sync::mpsc;

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
pub enum AppMessage {
    SetTitle(String),
    Alert(String),
}

#[derive(Debug, Clone)]
pub enum BgMessage{
    LoadImage(PathBuf),
    UpdateImage,
    ClearImage,
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
        let range: std::ops::Range<u8> = 0..(palette.len() as u8 - 1);
        for (i, pixel) in zip(range, fb.chunks_exact_mut(4)) {
            let max: f64 = (palette.len()-1) as f64;
            let val: u8 = (i as f64 * (255.0/max)).round() as u8;
            pixel.copy_from_slice(&[val, val, val, 255]);
        }
    }

    Ok(RgbImage::new(&fb, width, height, ColorDepth::Rgba8)?)
}

// TODO: Make these into macros so they can have printouts of the function and line on where they were used or so in their error messages
fn try_send<T: std::any::Any>(sender: &mpsc::Sender<T>, msg: T) -> Result<(), String> {
    sender.send(msg).map_err(|err| format!("Send error {err:?}"))?;
    if std::any::TypeId::of::<AppMessage>() == std::any::TypeId::of::<T>() {
        fltk::app::awake();
    }
    Ok(())
}

fn send_noerr<T: std::any::Any>(sender: &mpsc::Sender<T>, msg: T) -> () {
    match try_send(sender, msg) {
        Ok(()) => (),
        Err(msg) => eprintln!("try_send failed: {msg}"),
    }
}

fn start_background_process(appmsg_sender: &mpsc::Sender<AppMessage>) -> mpsc::Sender<BgMessage> {
    let appmsg = appmsg_sender.clone();
    let (sender, receiver) = mpsc::channel::<BgMessage>();
    let sender_return = sender.clone();

    thread::spawn(move || {
        let mut imagepath: Option<PathBuf> = None;

        for msg in receiver {
            match msg {
                BgMessage::LoadImage(path) => {
                    imagepath = Some(path);
                },
                BgMessage::ClearImage => {
                    match || -> Result<(), String> {
                        let mut frame: Frame = app::widget_from_id("frame").ok_or("widget_from_id fail")?;
                        let mut palette_frame: Frame = app::widget_from_id("palette_frame").ok_or("widget_from_id fail")?;

                        imagepath = None;

                        frame.set_image(None::<SharedImage>);
                        frame.set_label("Clear");
                        frame.changed();

                        palette_frame.set_image(None::<RgbImage>);
                        palette_frame.changed();

                        fltk::app::redraw();

                        try_send(&appmsg, AppMessage::SetTitle("Clear".to_string()))?;

                        Ok(())
                    }() {
                        Ok(()) => (),
                        Err(errmsg) => {
                            let msg = format!("ClearImage fail:\n{errmsg}");
                            eprintln!("{}", msg);
                            send_noerr(&appmsg, AppMessage::Alert(msg));
                        }
                    }
                },
                BgMessage::UpdateImage => {
                    match || -> Result<(), String> {
                        let mut frame: Frame = app::widget_from_id("frame").ok_or("widget_from_id fail")?;
                        let mut palette_frame: Frame = app::widget_from_id("palette_frame").ok_or("widget_from_id fail")?;
                        let no_quantize_toggle: CheckButton = app::widget_from_id("no_quantize_toggle").ok_or("widget_from_id fail")?;
                        let grayscale_toggle: CheckButton = app::widget_from_id("grayscale_toggle").ok_or("widget_from_id fail")?;
                        let grayscale_output_toggle: CheckButton = app::widget_from_id("grayscale_output_toggle").ok_or("widget_from_id fail")?;
                        let reorder_palette_toggle: CheckButton = app::widget_from_id("reorder_palette_toggle").ok_or("widget_from_id fail")?;
                        let maxcolors_slider: HorValueSlider = app::widget_from_id("maxcolors_slider").ok_or("widget_from_id fail")?;
                        let dithering_slider: HorValueSlider = app::widget_from_id("dithering_slider").ok_or("widget_from_id fail")?;
                        let scaling_toggle: CheckButton = app::widget_from_id("scaling_toggle").ok_or("widget_from_id fail")?;
                        let scale_input: IntInput = app::widget_from_id("scale_input").ok_or("widget_from_id fail")?;
                        let multiplier_menubutton: menu::MenuButton = app::widget_from_id("multiplier_menubutton").ok_or("widget_from_id fail")?;

                        let Some(ref path) = imagepath else {
                            eprintln!("loadimage: No file selected/imagepath not set");
                            return Ok(());
                        };

                        // TODO: Switch to using the image crate to load and also to grayscale. Also evaluate it at dithering?
                        //       We should only convert to FLTK image format at the very end
                        let image = SharedImage::load(&path).map_err(|err| format!("Image load for image {path:?} failed: {err:?}"))?;
                        println!("Loaded image {path:?}");

                        if !no_quantize_toggle.is_checked() {
                            let mut bytes: Vec<u8>;
                            let mut width: usize;
                            let mut height: usize;

                            (bytes, width, height) = sharedimage_to_bytes(&image, grayscale_toggle.is_checked())
                                .map_err(|err| format!("sharedimage_to_bytes failed: {err:?}"))?;

                            if scaling_toggle.is_checked() {
                                let value = scale_input.value();
                                let scale: usize = value.parse()
                                    .map_err(|err| format!("Couldn't parse scale {value:?}: {err:?}"))?;
                                (bytes, width, height) = scale_image(&bytes, width, height, scale, scale)
                                    .map_err(|err| format!("scale_image failed: {err:?}"))?;
                            }

                            let (indexes, palette) = quantize_image(
                                &bytes, width, height,
                                maxcolors_slider.value() as i32,
                                dithering_slider.value() as f32,
                                reorder_palette_toggle.is_checked(),
                            ).map_err(|err| format!("Quantization failed: {err:?}"))?;

                            let mut rgbimage = quantized_image_to_rgbimage(
                                &indexes, &palette,
                                width, height,
                                grayscale_output_toggle.is_checked(),
                            ).map_err(|err| format!("Conversion to rgbimage failed: {err:?}"))?;

                            if scaling_toggle.is_checked() {
                                let multiplier: usize =
                                    match || -> Result<_, String> {
                                        let choice: String = multiplier_menubutton.choice()
                                            .ok_or("No choice selected in multiplier menubutton")?;
                                        let choice = choice.strip_suffix("x")
                                            .ok_or_else(|| format!("No x suffix in multiplier menubutton choice: {choice:?}"))?;
                                        let multiplier = choice.parse()
                                            .map_err(|err| format!("Couldn't parse multiplier {choice:?}: {err:?}"))?;
                                        Ok(multiplier)
                                    }() {
                                        Ok(res) => res,
                                        Err(msg) => {
                                            eprintln!("{}", msg);
                                            1
                                        },
                                    };
                                rgbimage.scale((width*multiplier) as i32, (height*multiplier) as i32, true, true); // Display pixelly image larger
                            }
                            frame.set_image(Some(rgbimage));

                            let palette_rgbimage = palette_to_rgbimage(&palette, grayscale_output_toggle.is_checked())
                                .map_err(|err| format!("Couldn't generate palette RgbImage: {err:?}"))?;
                            palette_frame.set_image_scaled(Some(palette_rgbimage));
                            palette_frame.changed();
                            palette_frame.redraw();
                        } else {
                            frame.set_image(Some(image));
                        }

                        let pathstr = path.to_string_lossy();
                        frame.set_label(&pathstr);
                        frame.changed();
                        frame.redraw();
                        fltk::app::awake();
                        try_send(&appmsg, AppMessage::SetTitle(pathstr.to_string()))?;

                        println!("Finished processing for {path:?}");

                        Ok(())
                    }() {
                        Ok(()) => (),
                        Err(errmsg) => {
                            let msg = format!("UpdateImage fail:\n{errmsg}");
                            eprintln!("{}", msg);
                            send_noerr(&appmsg, AppMessage::Alert(msg));
                            send_noerr(&sender, BgMessage::ClearImage);
                        },
                    }
                },
            }
        }
    });

    sender_return
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

    let (appmsg_send, appmsg_recv) = mpsc::channel::<AppMessage>();
    let bg_send = start_background_process(&appmsg_send);

    openbtn.set_callback({
        let bg_send = bg_send.clone();
        let appmsg_send = appmsg_send.clone();
        move |_| {
            println!("Open button pressed");

            let Some(path) = get_file() else {
                eprintln!("No file selected/cancelled");
                return;
            };

            match || -> Result<(), String> {
                try_send(&bg_send, BgMessage::LoadImage(path))?;
                try_send(&bg_send, BgMessage::UpdateImage)?;
                Ok(())
            }() {
                Ok(()) => (),
                Err(msg) => {
                    eprintln!("{}", msg);
                    send_noerr(&appmsg_send, AppMessage::Alert(msg));
                }
            }
        }
    });

    clearbtn.set_callback({
        let bg_send = bg_send.clone();
        let appmsg_send = appmsg_send.clone();
        move |_| {
            println!("Clear button pressed");

            let sendresult = bg_send.send(BgMessage::ClearImage);
            if sendresult.is_err() {
                let msg = format!("{:?}", sendresult);
                eprintln!("{}", msg);
                send_noerr(&appmsg_send, AppMessage::Alert(msg));
            }
        }
    });

    no_quantize_toggle.set_callback(     { let bg_send = bg_send.clone(); move |_| send_noerr(&bg_send, BgMessage::UpdateImage) });
    grayscale_toggle.set_callback(       { let bg_send = bg_send.clone(); move |_| send_noerr(&bg_send, BgMessage::UpdateImage) });
    grayscale_output_toggle.set_callback({ let bg_send = bg_send.clone(); move |_| send_noerr(&bg_send, BgMessage::UpdateImage) });
    reorder_palette_toggle.set_callback( { let bg_send = bg_send.clone(); move |_| send_noerr(&bg_send, BgMessage::UpdateImage) });
    maxcolors_slider.set_callback(       { let bg_send = bg_send.clone(); move |_| send_noerr(&bg_send, BgMessage::UpdateImage) });
    dithering_slider.set_callback(       { let bg_send = bg_send.clone(); move |_| send_noerr(&bg_send, BgMessage::UpdateImage) });
    scaling_toggle.set_callback(         { let bg_send = bg_send.clone(); move |_| send_noerr(&bg_send, BgMessage::UpdateImage) });
    scale_input.set_callback({
        let bg_send = bg_send.clone();
        move |i| {
            let value = i.value();
            println!("scale_input: i.value() = {:?}, i.active={:?}", i.value(), i.active());
            if value.len() > 0 {
                send_noerr(&bg_send, BgMessage::UpdateImage)
            } else {
                i.set_value(SCALE_DEFAULT);
            }
        }
    });
    multiplier_menubutton.set_callback({
        let bg_send = bg_send.clone();
        move |m| {
            println!("multiplier_menubutton: m.choice() = {:?}", m.choice());
            m.set_label(&format!("Display scale multiplier: {}", m.choice().unwrap_or("NOT SET".to_string())));
            send_noerr(&bg_send, BgMessage::UpdateImage);
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
            send_noerr(&appmsg_send, AppMessage::Alert(format!("{panic_info}")));
        }
    }));

    // app.run()?;

    while app.wait() {
        match appmsg_recv.try_recv() {
            Ok(msg) => match msg {
                AppMessage::Alert(s)    => dialog::alert_default(&s),
                AppMessage::SetTitle(s) => wind.set_label(&s),
            },
            Err(mpsc::TryRecvError::Empty) => (),
            Err(err) => eprintln!("Channel error: {err}"),
        }
    }

    println!("App finished");
    Ok(())
}
