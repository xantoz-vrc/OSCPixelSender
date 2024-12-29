use fltk::{app, frame::Frame, enums::FrameType, image::*, enums::ColorDepth, prelude::*, window::Window, group::*, button::*, valuator::*, dialog};
use std::error::Error;
use std::path::PathBuf;
use std::iter::zip;
use rayon::prelude::*;
use std::sync::RwLock;
use std::thread;
use std::panic;
use std::string::String;
use image::{self, imageops};
use std::sync::mpsc;
use std::sync::OnceLock;

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
pub enum Message {
    SetTitle(String),
    Alert(String),
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
            let index : u8 = index*(255/(palette.len()-1)) as u8;
            pixel.copy_from_slice(&[index, index, index, 255]);
        }
    }

    Ok(RgbImage::new(&fb, width as i32, height as i32, ColorDepth::Rgba8)?)
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

    let mut col = Flex::default_fill().column();
    col.set_margin(20);
    col.set_spacing(20);
    let mut openbtn = Button::default().with_label("Open");
    let mut clearbtn = Button::default().with_label("Clear");

    let mut no_quantize_toggle = CheckButton::default().with_label("Disable quantization").with_id("no_quantize_toggle");
    let mut grayscale_toggle = CheckButton::default().with_label("Grayscale the image before converting").with_id("grayscale_toggle");
    let mut grayscale_output_toggle = CheckButton::default().with_label("Output the palette indexes without using the palette as grayscale").with_id("grayscale_output_toggle");
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

    row.fixed(&col, 300);
    col.fixed(&openbtn, 50);
    col.fixed(&clearbtn, 50);
    col.fixed(&no_quantize_toggle, 10);
    col.fixed(&grayscale_toggle, 10);
    col.fixed(&grayscale_output_toggle, 10);
    col.fixed(&reorder_palette_toggle, 10);
    col.fixed(&maxcolors_slider, 30);
    col.fixed(&dithering_slider, 30);
    col.fixed(&scaling_toggle, 20);

    static SEND: OnceLock<mpsc::Sender<Message>> = OnceLock::new();
    let chan: (mpsc::Sender<Message>, mpsc::Receiver<Message>) = mpsc::channel::<Message>();
    let recv = chan.1;
    SEND.set(chan.0).unwrap();

    static IMAGEPATH: RwLock<Option<PathBuf>> = RwLock::new(None);

    fn try_send(msg: Message) -> Result<(), String> {
        SEND.get().ok_or("SEND not set")?
            .send(msg).map_err(|err| format!("Send error {err:?}"))?;
        fltk::app::awake();
        Ok(())
    }

    fn send_noerr(msg: Message) -> () {
        match try_send(msg) {
            Ok(()) => (),
            Err(msg) => eprintln!("try_send failed: {msg}"),
        }
    }

    fn clearimage() {
        match || -> Result<(), String> {
            let mut frame: Frame = app::widget_from_id("frame").ok_or("widget_from_id fail")?;

            let mut imagepath_lock = IMAGEPATH.write().map_err(|err| format!("Failed to lock IMAGEPATH for writing: {err}"))?;
            *imagepath_lock = None;
            frame.set_image(None::<SharedImage>);
            frame.set_label("Clear");
            frame.changed();

            try_send(Message::SetTitle("Clear".to_string()))?;

            Ok(())
        }() {
            Ok(()) => (),
            Err(msg) => {
                eprintln!("{}", msg);
                send_noerr(Message::Alert(msg));
            }
        }
    }

    fn loadimage() {
        println!("loadimage called");

        thread::spawn(|| {
            match (
                || -> Result<(), String> {
                    let mut frame: Frame = app::widget_from_id("frame").ok_or("widget_from_id fail")?;
                    let no_quantize_toggle: CheckButton = app::widget_from_id("no_quantize_toggle").ok_or("widget_from_id fail")?;
                    let grayscale_toggle: CheckButton = app::widget_from_id("grayscale_toggle").ok_or("widget_from_id fail")?;
                    let grayscale_output_toggle: CheckButton = app::widget_from_id("grayscale_output_toggle").ok_or("widget_from_id fail")?;
                    let reorder_palette_toggle: CheckButton = app::widget_from_id("reorder_palette_toggle").ok_or("widget_from_id fail")?;
                    let maxcolors_slider: HorValueSlider = app::widget_from_id("maxcolors_slider").ok_or("widget_from_id fail")?;
                    let dithering_slider: HorValueSlider = app::widget_from_id("dithering_slider").ok_or("widget_from_id fail")?;
                    let scaling_toggle: CheckButton = app::widget_from_id("scaling_toggle").ok_or("widget_from_id fail")?;

                    // Clone the path, we do not want to keep holding the
                    // lock. It can lead to deadlock with clearimage otherwise
                    // for one.
                    let path = {
                        let imagepath_readguard = IMAGEPATH.read()
                            .map_err(|err| format!("Error obtaining read lock for image path variable: {err:?}"))?;
                        let Some(ref path) = *imagepath_readguard else {
                            eprintln!("loadimage: No file selected/imagepath not set");
                            return Ok(());
                        };
                        path.clone()
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
                            (bytes, width, height) = scale_image(&bytes, width, height, 128, 128)
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
                            rgbimage.scale(512, 512, true, true); // Display pixelly image larger
                        }
                        frame.set_image(Some(rgbimage));
                    } else {
                        frame.set_image(Some(image));
                    }

                    let pathstr = path.to_string_lossy();
                    frame.set_label(&pathstr);
                    frame.changed();
                    try_send(Message::SetTitle(pathstr.to_string()))?;

                    println!("Finished processing for {path:?}");

                    Ok(())
                }
            )() {
                Ok(()) => (),
                Err(msg) => {
                    eprintln!("{}", msg);
                    send_noerr(Message::Alert(msg));
                    clearimage();
                },
            }
        });
    }

    openbtn.set_callback({
        move |_| {
            println!("Open button pressed");

            let Some(path) = get_file() else {
                eprintln!("No file selected/cancelled");
                return;
            };

            *(IMAGEPATH.write().unwrap()) = Some(path);
            loadimage();
        }
    });

    clearbtn.set_callback({
        move |_| {
            println!("Clear button pressed");
            clearimage();
        }
    });

    no_quantize_toggle.set_callback(|_| loadimage());
    grayscale_toggle.set_callback(|_| loadimage());
    grayscale_output_toggle.set_callback(|_| loadimage());
    reorder_palette_toggle.set_callback(|_| loadimage());
    maxcolors_slider.set_callback(|_| loadimage());
    dithering_slider.set_callback(|_| loadimage());
    scaling_toggle.set_callback(|_| loadimage());

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
            send_noerr(Message::Alert(format!("{panic_info}")));
        }
    }));

    // app.run()?;

    while app.wait() {
        match recv.try_recv() {
            Ok(msg) => match msg {
                Message::Alert(s)    => dialog::alert_default(&s),
                Message::SetTitle(s) => wind.set_label(&s),
            },
            Err(mpsc::TryRecvError::Empty) => (),
            Err(err) => eprintln!("Channel error: {err}"),
        }
    }

    println!("App finished");
    Ok(())
}
