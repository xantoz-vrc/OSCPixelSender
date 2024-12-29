use fltk::{app, frame::Frame, enums::FrameType, image::*, enums::ColorDepth, prelude::*, window::Window, group::*, button::Button, dialog};
use std::error::Error;
use std::path::PathBuf;
use std::iter::zip;

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

fn sharedimage_to_bytes(image : &SharedImage) -> Result<(Vec<u8>, usize, usize), Box<dyn Error>>
{
    let bytes : Vec<u8> = image.to_rgb_image()?.convert(ColorDepth::Rgba8)?.to_rgb_data();
    println!("bytes.len(): {}", bytes.len());
    let width : usize = image.data_w().try_into()?;
    let height : usize = image.data_h().try_into()?;

    Ok((bytes, width, height))
}

// Make it palletted image and then we reconvert it back to RgbImage
// for display (I haven't found a way to display paletted images directly in FLTK)
//
// TODO: Split this up into two functions, one which returns the
// indexes+palette and another which turns indexes + palette into an
// RGBImage for display
fn quantize_image(bytes : &Vec<u8>, width : usize, height : usize) -> Result<RgbImage, Box<dyn Error>> {

    let qimage = quantizr::Image::new(bytes, width, height)?;
    let mut qopts = quantizr::Options::default();
    qopts.set_max_colors(16)?;

    let mut result = quantizr::QuantizeResult::quantize(&qimage, &qopts);
    result.set_dithering_level(1.0)?;

    let mut indexes = vec![0u8; width*height];
    result.remap_image(&qimage, indexes.as_mut_slice())?;

    let palette = result.get_palette();

    // -------------------- cut here --------------------

    // Turn the quantized thing back into RGB for display
    let mut fb: Vec<u8> = vec![0u8; width * height * 4];
    for (index, pixel) in zip(indexes, fb.chunks_exact_mut(4)) {
        let c : quantizr::Color = palette.entries[index as usize];
        pixel.copy_from_slice(&[c.r, c.g, c.b, c.a]);
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
    let mut frame = Frame::default_fill();
    frame.set_frame(FrameType::DownBox);

    let mut col = Flex::default_fill().column();
    col.set_margin(20);
    let mut openbtn = Button::default().with_label("Open");
    let mut clearbtn = Button::default().with_label("Clear");

    row.fixed(&col, 200);
    col.fixed(&openbtn, 50);
    col.fixed(&clearbtn, 50);

    openbtn.set_callback({
        let mut fr = frame.clone();
        move |_| {
            println!("Open button pressed");

            // let path = "F:/tw20230603-1.jpg";

            let maybe_path = get_file();
            if !maybe_path.is_some() {
                eprintln!("No file selected");
                return;
            }

            let path = &maybe_path.unwrap();

            match SharedImage::load(path) {
                Err(err) => {
                    let msg = format!("Image load for image {path:?} failed: {err:?}");
                    eprintln!("{}", msg);
                    dialog::alert_default(&msg);
                },
                Ok(mut image) => {
                    println!("Loaded image {path:?}");

                    println!("(before scale) w,h: {},{}", image.width(), image.height());
                    image.scale(256, 256, true, true);
                    println!("(after scale) w,h: {},{}", image.width(), image.height());

                    let bresult = sharedimage_to_bytes(&image);
                    let Ok((bytes, width, height)) = bresult else {
                        let msg = format!("sharedimage_to_bytes failed: {bresult:?}");
                        eprintln!("{}", msg);
                        dialog::alert_default(&msg);
                        return;
                    };

                    let qresult = quantize_image(&bytes, width, height);
                    let Ok(rgbimage) = qresult else {
                        let msg = format!("Quantization failed: {qresult:?}");
                        eprintln!("{}", msg);
                        dialog::alert_default(&msg);
                        return;
                    };

                    fr.set_image(Some(rgbimage));
                    fr.set_label(&path.to_string_lossy());
                    fr.changed();

                    // fr.set_image(Some(image));
                    // fr.set_label(&path.to_string_lossy());
                    // fr.changed();

                    // fr.set_image_scaled(Some(image));
                    // fr.set_label(path.to_string_lossy());
                    // fr.changed();
                },
            };
        }
    });

    clearbtn.set_callback({
        let mut fr = frame.clone();
        move |_| {
            println!("Clear button pressed");

            fr.set_image(None::<SharedImage>);
            fr.set_label("Clear");
            fr.changed();
        }
    });

    col.end();
    row.end();
    wind.end();

    wind.make_resizable(true);
    wind.show();

    app.run()?;

    println!("App finished");
    Ok(())
}
