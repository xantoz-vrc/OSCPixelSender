use fltk::{app, frame::Frame, enums::FrameType, image::*, enums::ColorDepth, prelude::*, window::Window, group::*, button::*, valuator::*, dialog};
use std::error::Error;
use std::path::PathBuf;
use std::iter::zip;
use rayon::prelude::*;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::Mutex;
use std::thread;
use std::panic;
use std::string::String;
use image::{self, imageops};

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
    let mut wind = Window::default().with_size(800, 600);

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

    let mut no_quantize_toggle = CheckButton::default().with_label("Disable quantization");
    let mut grayscale_toggle = CheckButton::default().with_label("Grayscale the image before converting");
    let mut grayscale_output_toggle = CheckButton::default().with_label("Output the palette indexes without using the palette as grayscale");
    let mut reorder_palette_toggle = CheckButton::default().with_label("Sort palette");
    reorder_palette_toggle.set_checked(true);

    let mut maxcolors_slider = HorValueSlider::default().with_label("Max Colors");
    maxcolors_slider.set_range(2.0, 256.0);
    maxcolors_slider.set_step(1.0, 1);
    maxcolors_slider.set_value(16.0);

    let mut dithering_slider = HorValueSlider::default().with_label("Dithering Level");
    dithering_slider.set_range(0.0, 1.0);
    dithering_slider.set_value(1.0);

    row.fixed(&col, 200);
    col.fixed(&openbtn, 50);
    col.fixed(&clearbtn, 50);
    col.fixed(&no_quantize_toggle, 10);
    col.fixed(&grayscale_toggle, 10);
    col.fixed(&grayscale_output_toggle, 10);
    col.fixed(&reorder_palette_toggle, 10);
    col.fixed(&maxcolors_slider, 30);
    col.fixed(&dithering_slider, 30);

    let (send, recv) = app::channel::<Message>();

    static IMAGEPATH: RwLock<Option<PathBuf>> = RwLock::new(None);

    let clearimage = Arc::new(Mutex::new({
        let send = send.clone();
        move || {
            let mut frame: Frame = app::widget_from_id("frame").unwrap();

            *(IMAGEPATH.write().unwrap()) = None;
            frame.set_image(None::<SharedImage>);
            frame.set_label("Clear");
            frame.changed();
            send.send(Message::SetTitle("Clear".to_string()));
        }
    }));

    let loadimage = Arc::new(Mutex::new({
        let no_quantize_toggle = no_quantize_toggle.clone();
        let grayscale_toggle = grayscale_toggle.clone();
        let grayscale_output_toggle = grayscale_output_toggle.clone();
        let reorder_palette_toggle = reorder_palette_toggle.clone();
        let maxcolors_slider = maxcolors_slider.clone();
        let dithering_slider = dithering_slider.clone();
        let clearimage = Arc::clone(&clearimage);
        let send = send.clone();

        move || {
            println!("loadimage called");

            thread::spawn({
                let no_quantize_toggle = no_quantize_toggle.clone();
                let grayscale_toggle = grayscale_toggle.clone();
                let grayscale_output_toggle = grayscale_output_toggle.clone();
                let reorder_palette_toggle = reorder_palette_toggle.clone();
                let maxcolors_slider = maxcolors_slider.clone();
                let dithering_slider = dithering_slider.clone();
                let clearimage = Arc::clone(&clearimage);
                let send = send.clone();

                move || {
                    let mut frame: Frame = app::widget_from_id("frame").unwrap();

                    // Clone the path, we do not want to keep holding the
                    // lock. It can lead to deadlock with clearimage otherwise
                    // for one.
                    let path = {
                        let imagepath_readguard = IMAGEPATH.read().unwrap();
                        let Some(ref path) = *imagepath_readguard else {
                            eprintln!("loadimage: No file selected/imagepath not set");
                            return;
                        };
                        path.clone()
                    };

                    // TODO: Switch to using the image crate to load and also to grayscale. Also evaluate it at dithering?
                    //       We should only convert to FLTK image format at the very end
                    let loadresult = SharedImage::load(&path);
                    let Ok(image) = loadresult else {
                        let msg = format!("Image load for image {path:?} failed: {loadresult:?}");
                        eprintln!("{}", msg);
                        send.send(Message::Alert(msg));
                        clearimage.lock().unwrap()();
                        return;
                    };

                    println!("Loaded image {path:?}");

                    if !no_quantize_toggle.is_checked() {
                        let bresult = sharedimage_to_bytes(&image, grayscale_toggle.is_checked());
                        let Ok((bytes, width, height)) = bresult else {
                            let msg = format!("sharedimage_to_bytes failed: {bresult:?}");
                            eprintln!("{}", msg);
                            send.send(Message::Alert(msg));
                            clearimage.lock().unwrap()();
                            return;
                        };

                        let scale_result = scale_image(&bytes, width, height, 128, 128);
                        let Ok((scaled_image, nwidth, nheight)) = scale_result else {
                            let msg = format!("scale_image failed: {scale_result:?}");
                            eprintln!("{}", msg);
                            send.send(Message::Alert(msg));
                            clearimage.lock().unwrap()();
                            return;
                        };

                        let qresult = quantize_image(
                            &scaled_image,
                            nwidth, nheight,
                            maxcolors_slider.value() as i32,
                            dithering_slider.value() as f32,
                            reorder_palette_toggle.is_checked(),
                        );
                        let Ok((indexes, palette)) = qresult else {
                            let msg = format!("Quantization failed: {:?}", qresult.err());
                            eprintln!("{}", msg);
                            send.send(Message::Alert(msg));
                            clearimage.lock().unwrap()();
                            return;
                        };

                        let mut rgbresult = quantized_image_to_rgbimage(
                            &indexes, &palette,
                            nwidth, nheight,
                            grayscale_output_toggle.is_checked(),
                        );
                        let Ok(mut rgbimage) = rgbresult else {
                            let msg = format!("Quantization failed: {rgbresult:?}");
                            eprintln!("{}", msg);
                            send.send(Message::Alert(msg));
                            clearimage.lock().unwrap()();
                            return;
                        };

                        rgbimage.scale(1024, 1024, true, true); // Display larger
                        frame.set_image(Some(rgbimage));
                    } else {
                        frame.set_image(Some(image));
                    }

                    let pathstr = path.to_string_lossy();
                    frame.set_label(&pathstr);
                    frame.changed();
                    send.send(Message::SetTitle(pathstr.to_string()));
                }
            });
        }
    }));

    openbtn.set_callback({
        let loadimage = Arc::clone(&loadimage);
        move |_| {
            println!("Open button pressed");

            let Some(path) = get_file() else {
                eprintln!("No file selected/cancelled");
                return;
            };

            *(IMAGEPATH.write().unwrap()) = Some(path);
            loadimage.lock().unwrap()();
        }
    });

    clearbtn.set_callback({
        let clearimage = Arc::clone(&clearimage);
        move |_| {
            println!("Clear button pressed");
            clearimage.lock().unwrap()();
        }
    });

    let loadimage_callback = {
        let loadimage = Arc::clone(&loadimage);
        move |_btn : &mut CheckButton| {
            loadimage.lock().unwrap()();
        }
    };

    no_quantize_toggle.set_callback(loadimage_callback.clone());
    grayscale_toggle.set_callback(loadimage_callback.clone());
    grayscale_output_toggle.set_callback(loadimage_callback.clone());
    reorder_palette_toggle.set_callback(loadimage_callback.clone());

    maxcolors_slider.set_callback({ let loadimage = Arc::clone(&loadimage); move |_| { loadimage.lock().unwrap()(); } });
    dithering_slider.set_callback({ let loadimage = Arc::clone(&loadimage); move |_| { loadimage.lock().unwrap()(); } });

    col.end();
    row.end();
    wind.end();

    wind.make_resizable(true);
    wind.show();

    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new({
        let send = send.clone();
        move |panic_info| {
            // invoke the default handler, but then display an alert message
            orig_hook(panic_info);
            send.send(Message::Alert(format!("{panic_info}")));
        }
    }));

    // app.run()?;

    while app.wait() {
        if let Some(msg) = recv.recv() {
            match msg {
                Message::Alert(s)    => dialog::alert_default(&s),
                Message::SetTitle(s) => wind.set_label(&s),
            }
        }
    }

    println!("App finished");
    Ok(())
}
