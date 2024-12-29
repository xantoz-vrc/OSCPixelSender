pub mod mq;
mod send_osc;
mod save_png;
#[macro_use]
mod utility;

use utility::{print_err, alert, error_alert};

use fltk::{app, frame::Frame, enums::*, prelude::*, window::Window, group::*, button::*, valuator::*, dialog, input::*, menu};
use std::error::Error;
use std::path::PathBuf;
use std::iter::zip;
use rayon::prelude::*;
use std::thread;
use std::panic;
use std::string::String;
use image::{self, imageops};
use std::sync::mpsc;
use std::default::Default;
use std::cmp::min;
use strum::*;
use strum_macros::*;

#[allow(unused_macros)]
macro_rules! function {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        name.strip_suffix("::f").unwrap_or(name)
    }}
}

macro_rules! time_it {
    ($context:literal, $($tt:tt)+) => {
        let timer = std::time::Instant::now();
        $(
            $tt
        )+
        println!("{}: {:?}", $context, timer.elapsed());
    }
}

pub enum AppMessage {
    SetTitle(String),
    Alert(String),
    // TODO: instead of passing a closure, just have this return the window to the sender on a sender-provided channel?
    //       Since I think calling window.show() might need to be from the main thread as well this will probably require another message
    //       to show a window
    // TODO alt: Just have a generic "RunOnMain" message taking a closure.
    CreateWindow(i32, i32, String, Box<dyn FnOnce(&mut Window) -> Result<(), Box<dyn Error>> + Send + Sync>),
    DeleteWindow(Window),
}

#[derive(Debug, Clone)]
pub enum BgMessage{
    LoadImage(PathBuf),
    SaveImage(PathBuf),
    UpdateImage{
        no_quantize: bool,
        grayscale: bool,
        grayscale_output: bool,
        reorder_palette: bool,
        maxcolors: i32,
        dithering: f32,
        scaling: bool,
        scale: u32,
        multiplier: u8,
        resize_type: ResizeType,
        scaler_type: ScalerType,
    },
    ClearImage,
    SendOSC(send_osc::SendOSCOpts),
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

fn get_file(dialogtype: dialog::FileDialogType) -> Option<PathBuf> {
    let mut nfc = dialog::NativeFileChooser::new(dialogtype);

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

#[derive(Debug, Clone, Default, PartialEq, EnumIter, EnumString, IntoStaticStr)]
pub enum ScalerType {
    #[default]
    XZBilinear,
    ImageCrateNearest,
    ImageCrateTriangle,
    ImageCrateCatmullRom,
    ImageCrateGaussian,
    ImageCrateLanczos3,
}

// Seems like maybe we could replace EnumString & IntoStaticStr with just the Display trait from strum?
// Using .to_string() to stringify and String::from("foo") to parse
// We could use VariantNames to get an array of strings instead of EnumIter
#[derive(Debug, Clone, Default, PartialEq, EnumIter, EnumString, IntoStaticStr)]
pub enum ResizeType {
    #[default]
    ToFill,
    ToFit,
}

// Home-cooked bilinear scaling
fn scale_image_bilinear(src: &[u8],
                        width: u32, height: u32,
                        nwidth: u32, nheight: u32,
                        resize: ResizeType
) -> Result<(Vec<u8>, u32, u32), Box<dyn Error>> {
    type F = f64;

    let width = width as usize;
    let height = height as usize;
    let nwidth = nwidth as usize;
    let nheight = nheight as usize;
    println!("{}: width={width}, height={height}, nwidth={nwidth}, nheight={nheight}", function!());

    assert!(src.len() == width * height * 4); // RGBA format assumed

    let (nwidth, nheight): (usize, usize) = match resize {
        ResizeType::ToFill => (nwidth, nheight), // TODO: In this case we need to change width and height as well unless we squish...
        ResizeType::ToFit => {
            if width > height {
                // Wider than tall
                let aspect_ratio: F = (width as F)/(height as F);
                (nwidth, ((nheight as F)/aspect_ratio).round() as usize)
            } else {
                // Taller than wide (or square)
                let aspect_ratio: F = (height as F)/(width as F);
                (((nwidth as F)/aspect_ratio).round() as usize, nheight)
            }
        },
    };

    println!("{}: width={width}, height={height}, nwidth={nwidth}, nheight={nheight}", function!());

    let x_scale: F = (width as F)/(nwidth as F);
    let y_scale: F = (height as F)/(nheight as F);

    let mut buffer: Vec<u8> = vec![0u8; nwidth * nheight * 4];
    // Parallelized using rayon
    buffer.par_chunks_exact_mut(4).enumerate().for_each(|(i, pixel)| {
        type Px = [u8; 4];
        type FPx = [F; 4];

        let (idst_x, idst_y) = (i % nwidth, i / nwidth);
        let (dst_x, dst_y) = (idst_x as F, idst_y as F);
        let (src_x, src_y) = (dst_x*x_scale, dst_y*y_scale);

        let src_ul = (src_x.floor(), src_y.floor());
        let src_ur = (src_x.ceil(),  src_y.floor());
        let src_dl = (src_x.floor(), src_y.ceil());
        let src_dr = (src_x.ceil(),  src_y.ceil());
        let isrc_ul = ((src_ul.0 as usize)%width, (src_ul.1 as usize)%height); // Wrap out of bounds
        let isrc_ur = ((src_ur.0 as usize)%width, (src_ur.1 as usize)%height);
        let isrc_dl = ((src_dl.0 as usize)%width, (src_dl.1 as usize)%height);
        let isrc_dr = ((src_dr.0 as usize)%width, (src_dr.1 as usize)%height);

        let idx_src_ul = (isrc_ul.0 + width*isrc_ul.1)*4;
        let idx_src_ur = (isrc_ur.0 + width*isrc_ur.1)*4;
        let idx_src_dl = (isrc_dl.0 + width*isrc_dl.1)*4;
        let idx_src_dr = (isrc_dr.0 + width*isrc_dr.1)*4;

        // Get the right byte slices out
        let iul: Px = src[idx_src_ul..idx_src_ul+4].try_into().expect("ul: Slices should be 4 long by definition");
        let iur: Px = src[idx_src_ur..idx_src_ur+4].try_into().expect("ur: Slices should be 4 long by definition");
        let idl: Px = src[idx_src_dl..idx_src_dl+4].try_into().expect("dl: Slices should be 4 long by definition");
        let idr: Px = src[idx_src_dr..idx_src_dr+4].try_into().expect("dr: Slices should be 4 long by definition");
        let ul: FPx = iul.map(|x| x as F);
        let ur: FPx = iur.map(|x| x as F);
        let dl: FPx = idl.map(|x| x as F);
        let dr: FPx = idr.map(|x| x as F);

        // interpolate along x
        let diff_x: F = src_ur.0 - src_x;
        debug_assert!(diff_x >= 0.0 && diff_x <= 1.0, "diff_x={diff_x} not between 0.0 and 1.0");
        // FIXME: Would be really cool to zip(ul, ur).map(|(a,b)| a*diff_x + b*(1.0 - diff_x)) here, but that won't work without heap allocation I think...
        //        Unless somehow const generics
        let interp_u: FPx = [
            ul[0]*diff_x + ur[0]*(1.0 - diff_x),
            ul[1]*diff_x + ur[1]*(1.0 - diff_x),
            ul[2]*diff_x + ur[2]*(1.0 - diff_x),
            ul[3]*diff_x + ur[3]*(1.0 - diff_x),
        ];
        let interp_d: FPx = [
            dl[0]*diff_x + dr[0]*(1.0 - diff_x),
            dl[1]*diff_x + dr[1]*(1.0 - diff_x),
            dl[2]*diff_x + dr[2]*(1.0 - diff_x),
            dl[3]*diff_x + dr[3]*(1.0 - diff_x),
        ];

        // interpolate along y
        let diff_y: F = src_dr.1 - src_y;
        debug_assert!(diff_y >= 0.0 && diff_y <= 1.0, "diff_y={diff_y} not between 0.0 and 1.0");

        let result: FPx = [
            interp_u[0]*diff_y + interp_d[0]*(1.0 - diff_y),
            interp_u[1]*diff_y + interp_d[1]*(1.0 - diff_y),
            interp_u[2]*diff_y + interp_d[2]*(1.0 - diff_y),
            interp_u[3]*diff_y + interp_d[3]*(1.0 - diff_y),
        ];

        let result: Px = result.map(|x| x as u8);
        pixel.copy_from_slice(&result);
    });

    Ok((buffer, nwidth.try_into()?, nheight.try_into()?))
}

// Image scaling using scaling from the image crate
fn scale_image_imagecrate(
    bytes: Vec<u8>,
    width: u32, height: u32,
    nwidth: u32, nheight: u32,
    resize: ResizeType,
    filter_type: imageops::FilterType,
) -> Result<(Vec<u8>, u32, u32), Box<dyn Error>> {
    assert!(bytes.len() == (width * height * 4) as usize); // RGBA format assumed

    let img = image::RgbaImage::from_raw(width as u32, height as u32, bytes).ok_or("bytes not big enough for width and height")?;
    let dimg = image::DynamicImage::from(img);
    let newimg = match resize {
        ResizeType::ToFill => dimg.resize_to_fill(nwidth, nheight, filter_type),
        ResizeType::ToFit  =>         dimg.resize(nwidth, nheight, filter_type),
    }.into_rgba8();

    let (w, h): (u32, u32) = newimg.dimensions();
    Ok((newimg.into_raw(), w, h))
}

fn scale_image(
    bytes: Vec<u8>,
    width: u32, height: u32,
    nwidth: u32, nheight: u32,
    resize: ResizeType,
    scaler_type: ScalerType,
) -> Result<(Vec<u8>, u32, u32), Box<dyn Error>> {
    match scaler_type {
        ScalerType::XZBilinear           => scale_image_bilinear(&bytes, width, height, nwidth, nheight, resize),
        ScalerType::ImageCrateNearest    => scale_image_imagecrate(bytes, width, height, nwidth, nheight, resize, imageops::FilterType::Nearest),
        ScalerType::ImageCrateTriangle   => scale_image_imagecrate(bytes, width, height, nwidth, nheight, resize, imageops::FilterType::Triangle),
        ScalerType::ImageCrateCatmullRom => scale_image_imagecrate(bytes, width, height, nwidth, nheight, resize, imageops::FilterType::CatmullRom),
        ScalerType::ImageCrateGaussian   => scale_image_imagecrate(bytes, width, height, nwidth, nheight, resize, imageops::FilterType::Gaussian),
        ScalerType::ImageCrateLanczos3   => scale_image_imagecrate(bytes, width, height, nwidth, nheight, resize, imageops::FilterType::Lanczos3),
    }
}

fn rgbaimage_to_bytes(image: &image::RgbaImage, grayscale: bool) -> (Vec<u8>, u32, u32) {
    use image::Pixel;

    let mut newimg = image.clone();
    let (w, h) = image.dimensions();

    if grayscale {
        for pixel in newimg.pixels_mut() {
            let gray = pixel.to_luma_alpha();
            let val = gray.0[0];
            let alpha = gray.0[1];
            *pixel = image::Rgba([val, val, val, alpha]);
        }
    }

    (newimg.into_raw(), w, h)
}

#[allow(dead_code)]
fn sharedimage_to_bytes(image : &fltk::image::SharedImage, grayscale : bool) -> Result<(Vec<u8>, u32, u32), Box<dyn Error>> {
    // let bytes : Vec<u8> = image.to_rgb_image()?.convert(ColorDepth::L8)?.convert(ColorDepth::Rgba8)?.to_rgb_data();

    let mut rgbimage = image.to_rgb_image()?;
    if grayscale {
        rgbimage = rgbimage.convert(ColorDepth::L8)?;
    }

    let bytes: Vec<u8> = rgbimage.convert(ColorDepth::Rgba8)?.to_rgb_data();
    println!("bytes.len(): {}", bytes.len());
    let width: u32 = rgbimage.data_w().try_into()?;
    let height: u32 = rgbimage.data_h().try_into()?;

    Ok((bytes, width, height))
}

// Ugly hack to workaround quantizr not being really made for
// grayscale by reordering the pallette, which means that the indexes
// should be able to be used without the palette as a sort-of
// grayscale image
fn reorder_palette_by_brightness(indexes : &[u8], palette : &quantizr::Palette) -> (Vec<u8>, Vec<quantizr::Color>)
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
fn quantize_image(bytes : &[u8],
                  width : u32, height : u32,
                  max_colors : i32,
                  dithering_level : f32,
                  reorder_palette : bool) -> Result<(Vec<u8>, Vec<quantizr::Color>), Box<dyn Error>> {

    // Need to make sure that input buffer is matching width and
    // height params for an RGBA buffer (4 bytes per pixel)
    assert!((width * height * 4) as usize == bytes.len());

    let qimage = quantizr::Image::new(bytes, width as usize, height as usize)?;
    let mut qopts = quantizr::Options::default();
    qopts.set_max_colors(max_colors)?;

    let mut result = quantizr::QuantizeResult::quantize(&qimage, &qopts);
    result.set_dithering_level(dithering_level)?;

    let mut indexes = vec![0u8; (width*height) as usize];
    result.remap_image(&qimage, indexes.as_mut_slice())?;
    assert!((width * height) as usize == indexes.len());

    let palette = result.get_palette();

    let result: (Vec<u8>, Vec<quantizr::Color>) = if reorder_palette {
        time_it!(
            "reorder_palette_by_brightness",
            let result = reorder_palette_by_brightness(&indexes, palette);
        );
        result
    } else {
        (indexes, palette.entries[0..(palette.count as usize)].to_vec())
    };

    Ok(result)
}

// Pads the image after already being quantized (assumes 1 byte per pixel)
// We do it on our own and in this manner because we wish to do it after we have quantized the image using quantizr
fn pad_image(bytes: Vec<u8>,
             pad_value: u8,
             width: u32, height: u32,
             nwidth: u32, nheight: u32
) -> (Vec<u8>, u32, u32) {
    let width: usize = width as usize;
    let height: usize = height as usize;
    let nwidth: usize = nwidth as usize;
    let nheight: usize = nheight as usize;

    println!("{}: bytes.len()={} width={width}, height={height}, nwidth={nwidth}, nheight={nheight}", function!(), bytes.len());

    assert!(width * height == bytes.len(), "width={width} * height={height} != bytes.len()={}", bytes.len()); // 8 bpp indexed image input
    assert!(nwidth >= width);
    assert!(nheight >= height);

    let mut output: Vec<u8> = bytes;

    // First pad width if applicable
    if nwidth > width {
        let diff = nwidth - width;
        let lpadding = diff / 2;
        let rpadding = diff.div_ceil(2);
        debug_assert!(lpadding + rpadding == diff);

        let size_after_padding = output.len() + (output.len()/width)*diff;
        let mut result: Vec<u8> = Vec::with_capacity(size_after_padding);

        for chunk in output.chunks_exact(width) {
            result.extend(std::iter::repeat(pad_value).take(lpadding));
            result.extend(chunk);
            result.extend(std::iter::repeat(pad_value).take(rpadding));
        }
        debug_assert!(result.len() == size_after_padding, "result.len()={}, size_after_padding={}", result.len(), size_after_padding);

        output = result;
    }

    // Then pad height if applicable
    if nheight > height {
        let diff = nheight - height;
        let tpadding = diff / 2;
        let bpadding = diff.div_ceil(2);
        debug_assert!(tpadding + bpadding == diff);

        let size_after_padding = output.len() + nwidth*diff;
        let mut result: Vec<u8> = Vec::with_capacity(size_after_padding);
        result.extend(std::iter::repeat(pad_value).take(tpadding*nwidth));
        result.extend(output);
        result.extend(std::iter::repeat(pad_value).take(bpadding*nwidth));
        debug_assert!(result.len() == size_after_padding, "result.len()={}, size_after_padding={}", result.len(), size_after_padding);

        output = result;
    }

    (output, nwidth as u32, nheight as u32)
}

fn rgbaimage_to_fltk_rgbimage(image: &image::RgbaImage) -> Result<fltk::image::RgbImage, Box<dyn Error>> {
    let (w, h) = image.dimensions();
    Ok(fltk::image::RgbImage::new(image.as_raw(), w.try_into()?, h.try_into()?, ColorDepth::Rgba8)?)
}

// Turn the quantized thing back into RGB for display
fn quantized_image_to_fltk_rgbimage(
    indexes: &[u8],
    palette: &[quantizr::Color],
    width: u32,
    height: u32,
    grayscale_output: bool
) -> Result<fltk::image::RgbImage, Box<dyn Error>> {
    assert!((width * height) as usize == indexes.len());

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

    Ok(fltk::image::RgbImage::new(&fb, width as i32, height as i32, ColorDepth::Rgba8)?)
}

fn palette_to_fltk_rgbimage(palette: &[quantizr::Color], grayscale_output: bool) -> Result<fltk::image::RgbImage, Box<dyn Error>> {
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

    Ok(fltk::image::RgbImage::new(&fb, width, height, ColorDepth::Rgba8)?)
}

fn enable_save_and_send_osc_button(active: bool) -> Result<(), String> {
    let mut savebtn: Button = app::widget_from_id("savebtn").ok_or("widget_from_id fail")?;
    let mut send_osc_btn: Button = app::widget_from_id("send_osc_btn").ok_or("widget_from_id fail")?;
    if active {
        savebtn.activate();
        send_osc_btn.activate();
    } else {
        savebtn.deactivate();
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
        #[allow(dead_code)]
        struct ProcessedImage {
            indexes: Vec<u8>,
            palette: Vec<quantizr::Color>,
            width: u32,
            height: u32,
            maxcolors: i32,
            grayscale_output: bool,
        }

        let mut rgbaimage: Option<image::RgbaImage> = None;
        let mut processed_image: Option<ProcessedImage> = None;

        loop {
            let recvres = receiver.recv();
            let Ok(msg) = recvres else {
                let s = format!("Error receiving from mq::MessageQueueReceiver: {}", recvres.unwrap_err());
                error_alert(&appmsg, s);
                continue;
            };

            match msg {
                BgMessage::Quit => {
                    break;
                },
                BgMessage::LoadImage(path) => {
                    match || -> Result<(), String> {
                        let image = image::ImageReader::open(&path)
                            .map_err(|err| format!("Couldn't open image {path:?}: {err}"))?
                            .with_guessed_format()
                            .map_err(|err| format!("Error when guessing format: {err}"))?
                            .decode()
                            .map_err(|err| format!("Failed to decode image {path:?}: {err}"))?;

                        rgbaimage = Some(image.to_rgba8());
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
                            error_alert(&appmsg, format!("LoadImage fail:\n{errmsg}"));
                            print_err(sender.send(BgMessage::ClearImage));
                        }
                    };
                },
                BgMessage::SaveImage(path) => {
                    match || -> Result<(), String> {
                        let path = path.with_extension("png");

                        let img = processed_image.as_ref()
                            .ok_or("No indexes or palette data")?;

                        let w = img.width.try_into().map_err(|err| format!("Trying to save zero width image: {err}"))?;
                        let h = img.height.try_into().map_err(|err| format!("Trying to save zero height image: {err}"))?;

                        save_png::save_png(
                            &path, w, h, &img.indexes, &img.palette,
                            match img.grayscale_output {
                                true  => save_png::ColorType::Grayscale,
                                false => save_png::ColorType::Indexed,
                            },
                        ).map_err(|err| format!("Couldn't save image to {path:?}: {err}"))?;

                        alert(&appmsg, format!("Saved image as {path:?}"));
                        Ok(())
                    }() {
                        Ok(()) => (),
                        Err(errmsg) => error_alert(&appmsg, format!("SaveImage error:\n{errmsg}")),
                    };
                },
                BgMessage::ClearImage => {
                    match || -> Result<(), String> {
                        let mut frame: Frame = app::widget_from_id("frame").ok_or("widget_from_id fail")?;
                        let mut palette_frame: Frame = app::widget_from_id("palette_frame").ok_or("widget_from_id fail")?;

                        processed_image = None;

                        rgbaimage = None;

                        frame.set_image(None::<fltk::image::RgbImage>);
                        frame.set_label("Clear");
                        frame.changed();

                        palette_frame.set_image(None::<fltk::image::RgbImage>);
                        palette_frame.changed();

                        enable_save_and_send_osc_button(false)?;

                        appmsg.send(AppMessage::SetTitle("Clear".to_string()))
                            .map_err(|err| format!("Send error: {err}"))?;
                        fltk::app::awake();

                        Ok(())
                    }() {
                        Ok(()) => (),
                        Err(errmsg) => error_alert(&appmsg, format!("ClearImage fail:\n{errmsg}")),
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
                    resize_type,
                    scaler_type,
                } => {
                    match || -> Result<(), String> {
                        enable_save_and_send_osc_button(false)?;

                        let Some(ref image) = rgbaimage else {
                            eprintln!("No image loaded");
                            return Ok(());
                        };

                        let now = std::time::Instant::now();

                        if !no_quantize {
                            let mut bytes: Vec<u8>;
                            let mut width: u32;
                            let mut height: u32;

                            time_it!(
                                "rgbaimage_to_bytes",
                                (bytes, width, height) = rgbaimage_to_bytes(&image, grayscale);
                            );

                            if scaling {
                                time_it!(
                                    "scale_image",
                                    (bytes, width, height) = scale_image(bytes, width, height, scale, scale, resize_type, scaler_type)
                                        .map_err(|err| format!("scale_image failed: {err:?}"))?;
                                );
                            }

                            time_it!(
                                "quantize_image",
                                let (mut indexes, palette) = quantize_image(
                                    &bytes, width, height,
                                    maxcolors,
                                    dithering,
                                    reorder_palette,
                                ).map_err(|err| format!("Quantization failed: {err:?}"))?;
                            );

                            if scaling {
                                // Pad if needed (needed when ResizeType::ToFit was used)

                                // While it would at first glance seem to make sense to handle padding directly in
                                // scale_image that would essentially force black into the palette of all images, and
                                // since the padding color isn't that important it's best to just do it after
                                // quantization. For now just picking whatever color 0 is, but we could eventually try
                                // to implement some fuzzy logic for picking the padding color.

                                time_it!(
                                    "pad_image",
                                    (indexes, width, height) = pad_image(indexes, 0u8, width, height, scale, scale);
                                );
                            }

                            time_it!(
                                "quantized_image_to_fltk_rgbimage",
                                let mut rgbimage = quantized_image_to_fltk_rgbimage(
                                    &indexes, &palette,
                                    width, height,
                                    grayscale_output,
                                ).map_err(|err| format!("Conversion to rgbimage failed: {err:?}"))?;
                            );

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

                                let palette_rgbimage = palette_to_fltk_rgbimage(&palette, grayscale_output)
                                    .map_err(|err| format!("Couldn't generate palette RgbImage: {err:?}"))?;
                                palette_frame.set_image_scaled(Some(palette_rgbimage));
                                palette_frame.changed();
                                palette_frame.redraw();
                            }

                            processed_image = Some(ProcessedImage{
                                indexes: indexes,
                                palette: palette,
                                width: width,
                                height: height,
                                maxcolors: maxcolors,
                                grayscale_output: grayscale_output,
                            });
                            enable_save_and_send_osc_button(true)?;
                        } else {
                            let mut frame: Frame = app::widget_from_id("frame").ok_or("widget_from_id fail")?;
                            frame.set_image(Some(
                                rgbaimage_to_fltk_rgbimage(image)
                                    .map_err(|err| format!("Failed to convert from image::RgbaImage to fltk::image::RgbImage: {err}"))?
                            ));
                            frame.changed();
                            frame.redraw();

                            // TODO: there should be a fallback here maybe
                            processed_image = None;
                            enable_save_and_send_osc_button(false)?;
                        }

                        fltk::app::awake();

                        println!("Finished updating image (took {:.2?})", now.elapsed());

                        Ok(())
                    }() {
                        Ok(()) => (),
                        Err(errmsg) => {
                            error_alert(&appmsg, format!("UpdateImage fail:\n{errmsg}"));
                            print_err(sender.send(BgMessage::ClearImage));
                        },
                    };
                },
                BgMessage::SendOSC(options) => {
                    println!("SendOSC({options:?})");
                    match || -> Result<(), String> {
                        let img = processed_image.as_ref()
                            .ok_or("Indexes and palette not generated yet")?;
                        send_osc::send_osc(&appmsg, &img.indexes, &img.palette, img.width, img.height, options)
                            .map_err(|err| format!("send_osc failed: {err}"))?;
                        Ok(())
                    }() {
                        Ok(()) => (),
                        Err(errmsg) => error_alert(&appmsg, format!("SendOSC fail:\n{errmsg}")),
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
        let resize_type_choice: menu::Choice = app::widget_from_id("resize_type_choice").ok_or("widget_from_id fail")?;
        let scaler_type_choice: menu::Choice = app::widget_from_id("scaler_type_choice").ok_or("widget_from_id fail")?;
        let multiplier_choice: menu::Choice = app::widget_from_id("multiplier_choice").ok_or("widget_from_id fail")?;

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
                    let choice: String = multiplier_choice.choice()
                        .ok_or("No multiplier choice selected")?;
                    let choice = choice.strip_suffix("x")
                        .ok_or_else(|| format!("No x suffix in multiplier choice: {choice:?}"))?;
                    let multiplier = choice.parse()
                        .map_err(|err| format!("Couldn't parse multiplier {choice:?}: {err}"))?;
                    Ok(multiplier)
                }() {
                    Ok(res) => res,
                    Err(msg) => {
                        error_alert(&appmsg, msg);
                        1
                    },
                }
            },
            resize_type: {
                match || -> Result<ResizeType, String> {
                    let choice = resize_type_choice.choice()
                        .ok_or("No resize type selected")?;
                    let parsed = choice.parse()
                        .map_err(|err| format!("Couldn't parse resize type {choice:?}: {err}"))?;
                    Ok(parsed)
                }() {
                    Ok(res) => res,
                    Err(msg) => {
                        error_alert(&appmsg, msg);
                        Default::default()
                    },
                }
            },
            scaler_type: {
                match || -> Result<ScalerType, String> {
                    let choice = scaler_type_choice.choice()
                        .ok_or("No resize type selected")?;
                    let parsed = choice.parse()
                        .map_err(|err| format!("Couldn't parse resize type {choice:?}: {err}"))?;
                    Ok(parsed)
                }() {
                    Ok(res) => res,
                    Err(msg) => {
                        error_alert(&appmsg, msg);
                        Default::default()
                    },
                }
            }
        };

        bg.send_or_replace_if(BgMessage::is_update, msg)
            .map_err(|err| format!("Send error: {err}"))?;

        Ok(())
    }() {
        Ok(()) => (),
        Err(errmsg) => error_alert(&appmsg, format!("{}:\n{}", function!(), errmsg)),
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let app = app::App::default().with_scheme(app::Scheme::Gleam);
    let screen_size = fltk::app::screen_size();
    println!("Screen size; {}x{}", screen_size.0, screen_size.1);
    let screen_size_int: (i32, i32) = (screen_size.0 as i32, screen_size.1 as i32);
    let mut wind = Window::default().with_size(
        min(1600, screen_size_int.0 - 64),
        min(1000, screen_size_int.1 - 64)
    );

    let small_screen = screen_size_int.1 < 1000;

    let mut row = Flex::default_fill().row();
    // row.set_margin(20);
    row.set_spacing(20);
    let mut frame = Frame::default_fill().with_id("frame");
    frame.set_frame(FrameType::DownBox);

    let palette_frame = Frame::default_fill().with_id("palette_frame");
    // palette_frame.set_frame(FrameType::DownBox);
    row.fixed(&palette_frame, 50);

    let scroll = fltk::group::Scroll::default_fill();
    row.fixed(&scroll, 300);

    let mut col = Flex::default_fill().column();
    row.fixed(&col, 280);
    col.set_margin(20);
    col.set_spacing(if small_screen { 15 } else { 20 });
    let mut openbtn = Button::default().with_label("Open");
    let mut savebtn = Button::default().with_label("Save").with_id("savebtn");
    savebtn.deactivate();
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
    let mut scale_input = IntInput::default().with_size(0, 40).with_label("Scale (NxN)").with_id("scale_input").with_align(Align::Inside);
    // scale_input.set_trigger(CallbackTrigger::Changed);
    scale_input.set_trigger(CallbackTrigger::EnterKey);
    scale_input.set_value(SCALE_DEFAULT);
    scale_input.set_maximum_size(4);
    let mut resize_type_choice = menu::Choice::default()
        .with_label("Scaling fit:")
        .with_id("resize_type_choice");
    resize_type_choice.add_choice(&ResizeType::iter().map(|e| e.into()).collect::<Vec<&'static str>>().join("|"));
    resize_type_choice.set_value(0);
    let mut scaler_type_choice = menu::Choice::default()
        .with_label("Scaler algorithm:")
        .with_id("scaler_type_choice");
    scaler_type_choice.add_choice(&ScalerType::iter().map(|e| e.into()).collect::<Vec<&'static str>>().join("|"));
    scaler_type_choice.set_value(0);

    let mut multiplier_choice = menu::Choice::default()
        .with_label("Display scale multiplier:")
        .with_id("multiplier_choice");
    multiplier_choice.add_choice("1x|2x|3x|4x|5x|6x|7x|8x");
    multiplier_choice.set_value(4);

    let mut divider = Frame::default_fill();
    divider.set_color(Color::Black);
    divider.set_frame(FrameType::FlatBox);

    const OSC_SPEED_DEFAULT: f64 = 5.0;
    let mut send_osc_btn = Button::default().with_label("Send OSC").with_id("send_osc_btn");
    send_osc_btn.deactivate();
    let mut osc_speed_slider = HorValueSlider::default().with_label("OSC updates/second").with_id("osc_speed_slider");
    osc_speed_slider.set_range(0.5, 20.0);
    osc_speed_slider.set_step(0.5, 1);
    osc_speed_slider.set_value(OSC_SPEED_DEFAULT);
    let osc_rle_compression_toggle = CheckButton::default().with_label("Use RLE compression").with_id("osc_rle_compression_toggle");
    osc_rle_compression_toggle.set_checked(true);
    let mut osc_pixfmt_choice = menu::Choice::default()
        .with_label("OSC Pixel format");
    // let pixfmt_choices = send_osc::PixFmt::into_iter().fold("".to_string(), |acc, s| format!("{acc}|{}", s.to_string()));
    // let pixfmt_choices = send_osc::PixFmt::into_iter().map(|p| p.to_string()).reduce(|acc, s| format!("{acc}|{s}")).unwrap();
    // let pixfmt_choices = send_osc::PixFmt::into_iter().map(|p| p.to_string()).join("|");
    let pixfmt_choices = send_osc::PixFmt::VALUES.map(|p| p.to_string()).join("|");
    osc_pixfmt_choice.add_choice(&pixfmt_choices);
    osc_pixfmt_choice.set_callback(|c| {
        println!("osc_pixfmt_choice: {:?}", c.choice())
    });
    osc_pixfmt_choice.set_value(0);

    let button_size = if small_screen { 30 } else { 50 };
    let toggle_size = if small_screen { 20 } else { 30 };
    let slider_size = if small_screen { 25 } else { 30 };
    let choice_size = if small_screen { 25 } else { 30 };
    let input_size = if small_screen { 20 } else { 30 };
    col.fixed(&openbtn, button_size);
    col.fixed(&savebtn, button_size);
    col.fixed(&clearbtn, button_size);
    col.fixed(&no_quantize_toggle, toggle_size);
    col.fixed(&grayscale_toggle, toggle_size);
    col.fixed(&grayscale_output_toggle, toggle_size);
    col.fixed(&reorder_palette_toggle, toggle_size);
    col.fixed(&maxcolors_slider, slider_size);
    col.fixed(&dithering_slider, slider_size);
    col.fixed(&scaling_toggle, toggle_size);
    col.fixed(&scale_input, input_size);
    col.fixed(&resize_type_choice, choice_size);
    col.fixed(&scaler_type_choice, choice_size);
    col.fixed(&multiplier_choice, choice_size);
    col.fixed(&divider, 5);
    col.fixed(&send_osc_btn, button_size);
    col.fixed(&osc_speed_slider, slider_size);
    col.fixed(&osc_rle_compression_toggle, toggle_size);
    col.fixed(&osc_pixfmt_choice, choice_size);

    let (appmsg, appmsg_recv) = mpsc::channel::<AppMessage>();
    let (joinhandle, bg) = start_background_process(&appmsg);

    openbtn.set_callback({
        let bg = bg.clone();
        let appmsg = appmsg.clone();
        move |_| {
            let Some(path) = get_file(dialog::FileDialogType::BrowseFile) else {
                eprintln!("No file selected/cancelled");
                return;
            };

            match || -> Result<(), Box<dyn Error>> {
                bg.send_or_replace_if(BgMessage::is_update, BgMessage::LoadImage(path))?;
                Ok(())
            }() {
                Ok(()) => (),
                Err(err) => error_alert(&appmsg, format!("Open button failed: {err}")),
            }
        }
    });

    savebtn.set_callback({
        let bg = bg.clone();
        let appmsg = appmsg.clone();
        move |_| {
            let Some(path) = get_file(dialog::FileDialogType::BrowseSaveFile) else {
                eprintln!("No file selected/cancelled");
                return;
            };

            match || -> Result<(), Box<dyn Error>> {
                bg.send(BgMessage::SaveImage(path))?;
                Ok(())
            }() {
                Ok(()) => (),
                Err(err) => error_alert(&appmsg, format!("Save button failed: {err}")),
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
                error_alert(&appmsg, format!("{}", sendresult.unwrap_err()));
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
    resize_type_choice.set_callback({ let bg = bg.clone(); let appmsg = appmsg.clone(); move |_| { send_updateimage(&appmsg, &bg); } });
    scaler_type_choice.set_callback({ let bg = bg.clone(); let appmsg = appmsg.clone(); move |_| { send_updateimage(&appmsg, &bg); } });
    multiplier_choice.set_callback({ let bg = bg.clone(); let appmsg = appmsg.clone(); move |_| { send_updateimage(&appmsg, &bg); } });

    send_osc_btn.set_callback({
        let bg = bg.clone();
        let appmsg = appmsg.clone();
        move |_| {
            match || -> Result<(), String> {
                bg.send(
                    BgMessage::SendOSC(send_osc::SendOSCOpts{
                        pixfmt: osc_pixfmt_choice.choice()
                            .ok_or("No PixFmt selected")?
                            .parse()?,
                        msgs_per_second: osc_speed_slider.value(),
                        rle_compression: osc_rle_compression_toggle.value(),
                        ..Default::default()
                    })
                ).map_err(|err| format!("bg.send error: {err}"))?;
                Ok(())
            }() {
                Ok(()) => (),
                Err(err) => error_alert(&appmsg, format!("Send OSC button error:\n{err}")),
            }
        }
    });

    scroll.end();
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
                    if let Err(err) = res {
                        let msg = format!("CreateWindow error: {err}");
                        eprintln!("{}", msg);
                        dialog::alert_default(&msg);
                        // Something failed, delete the window
                        Window::delete(wind);
                    } else {
                        wind.end();
                        wind.show();
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
