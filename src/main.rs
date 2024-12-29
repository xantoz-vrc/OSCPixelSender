use fltk::{app, frame::Frame, enums::FrameType, image::*, enums::ColorDepth, prelude::*, window::Window, group::*, button::*, dialog};
use std::error::Error;
use std::path::PathBuf;
use std::iter::zip;
use rayon::prelude::*;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::Mutex;

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
    // dbg!(&permutation);
    permutation.sort_by_key(|&i| {
        let c = palette.entries[i];
        let (r,g,b) = (c.r as i32, c.g as i32, c.b as i32);
        r + g + b
    });
    dbg!(&permutation);

    let new_palette : Vec<quantizr::Color> =
        permutation.iter()
        .map(|&i| palette.entries[i])
        .collect();

/*
    let mut new_palette : Vec<quantizr::Color> = vec![quantizr::Color { r: 0, g: 0, b: 0, a: 0}; palette.count as usize];
    for (old_idx, &new_idx) in permutation.iter().enumerate() {
        //new_palette[new_idx] = palette.entries[old_idx];
        new_palette[old_idx] = palette.entries[new_idx];
    }
*/

    // dbg!(palette.entries[0..(palette.count as usize)]);
    // dbg!(new_palette);

    dbg!(palette.entries[0..(palette.count as usize)].iter().map(|c| format!("{:03}, {:03}, {:03}, {:03}", c.r, c.g, c.b, c.a)).collect::<Vec<_>>());
    dbg!(new_palette.iter().map(|c| format!("{:03}, {:03}, {:03}, {:03}", c.r, c.g, c.b, c.a)).collect::<Vec<_>>());

    // Trying out fancy rayon parallel iterators
    // let new_indexes : Vec<u8> = indexes.par_iter().map(
    //     |ic| permutation[*ic as usize] as u8
    // ).collect();
    let new_indexes : Vec<u8> = indexes.par_iter().map(
        |ic| permutation.iter().position(|&r| r == *ic as usize).unwrap_or_default() as u8
    ).collect();

    (new_indexes, new_palette)
}

// Make it paletted image and then we reconvert it back to RgbImage
// for display (I haven't found a way to display paletted images directly in FLTK)
//
// TODO: Split this up into two functions, one which returns the
// indexes+palette and another which turns indexes + palette into an
// RGBImage for display
fn quantize_image(bytes : &Vec<u8>, width : usize, height : usize, max_colors : i32, grayscale_output : bool, reorder_palette : bool) -> Result<RgbImage, Box<dyn Error>> {

    let qimage = quantizr::Image::new(bytes, width, height)?;
    let mut qopts = quantizr::Options::default();
    qopts.set_max_colors(max_colors)?;

    let mut result = quantizr::QuantizeResult::quantize(&qimage, &qopts);
    result.set_dithering_level(1.0)?;

    let mut indexes = vec![0u8; width*height];
    result.remap_image(&qimage, indexes.as_mut_slice())?;

    let palette = result.get_palette();

    let a : Vec<_>;
    let b : Vec<_>;
    let (new_indexes, new_palette) : (&[u8], &[quantizr::Color]) = if reorder_palette {
        (a, b) = reorder_palette_by_brightness(&indexes, palette);
        (a.as_slice(), b.as_slice())
    } else {
        (indexes.as_slice(), &palette.entries[0..(palette.count as usize)])
    };

    // -------------------- cut here --------------------


    // Turn the quantized thing back into RGB for display
    let mut fb: Vec<u8> = vec![0u8; width * height * 4];
    if !grayscale_output {
        for (&index, pixel) in zip(new_indexes, fb.chunks_exact_mut(4)) {
            let c : quantizr::Color = new_palette[index as usize];
            pixel.copy_from_slice(&[c.r, c.g, c.b, c.a]);
        }
    } else {
        for (&index, pixel) in zip(new_indexes, fb.chunks_exact_mut(4)) {
            let index : u8 = index*palette.count as u8;
            pixel.copy_from_slice(&[index, index, index, index]);
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
    let mut frame = Frame::default_fill();
    frame.set_frame(FrameType::DownBox);

    let mut col = Flex::default_fill().column();
    col.set_margin(20);
    let mut openbtn = Button::default().with_label("Open");
    let mut clearbtn = Button::default().with_label("Clear");

    let mut grayscale_toggle = CheckButton::default().with_label("Grayscale the image before converting");
    let mut grayscale_output_toggle = CheckButton::default().with_label("Output the palette indexes without using the palette as grayscale");
    let mut reorder_palette_toggle = CheckButton::default().with_label("Sort palette");
    reorder_palette_toggle.set_checked(true);

    row.fixed(&col, 200);
    col.fixed(&openbtn, 50);
    col.fixed(&clearbtn, 50);
    col.fixed(&grayscale_toggle, 30);
    col.fixed(&grayscale_output_toggle, 30);

    let imagepath_arc : Arc<RwLock<Option<PathBuf>>> = Arc::new(RwLock::new(None));

    let clearimage_arc = Arc::new(Mutex::new({
        let mut fr = frame.clone();
        let mut wn = wind.clone();
        let imagepath = Arc::clone(&imagepath_arc);
        move || {
            *(imagepath.write().unwrap()) = None;
            fr.set_image(None::<SharedImage>);
            fr.set_label("Clear");
            fr.changed();
            wn.set_label("Clear");
        }
    }));

    let loadimage_arc = Arc::new(Mutex::new({
        let mut fr = frame.clone();
        let mut wn = wind.clone();
        let gr_toggle = grayscale_toggle.clone();
        let gr_output_toggle = grayscale_output_toggle.clone();
        let reorder_palette_toggle = reorder_palette_toggle.clone();
        let imagepath = Arc::clone(&imagepath_arc);
        let clearimage = Arc::clone(&clearimage_arc);
        move || {
            println!("loadimage called");

            // Clone the path, we do not want to keep holding the
            // lock. It can lead to deadlock with clearimage otherwise
            // for one.
            let path = {
                let imagepath_readguard = imagepath.read().unwrap();
                let Some(path) = &*imagepath_readguard else {
                    eprintln!("loadimage: No file selected/imagepath not set");
                    return;
                };
                path.clone()
            };

            let loadresult = SharedImage::load(&path);
            let Ok(image) = loadresult else {
                let msg = format!("Image load for image {path:?} failed: {loadresult:?}");
                eprintln!("{}", msg);
                dialog::alert_default(&msg);
                clearimage.lock().unwrap()();
                return;
            };

            println!("Loaded image {path:?}");

            println!("(before scale) w,h: {},{}", image.width(), image.height());
            //image.scale(256, 256, true, true);
            println!("(after scale) w,h: {},{}", image.width(), image.height());

            let bresult = sharedimage_to_bytes(&image, gr_toggle.is_checked());
            let Ok((bytes, width, height)) = bresult else {
                let msg = format!("sharedimage_to_bytes failed: {bresult:?}");
                eprintln!("{}", msg);
                dialog::alert_default(&msg);
                return;
            };

            let qresult = quantize_image(&bytes, width, height, 16, gr_output_toggle.is_checked(), reorder_palette_toggle.is_checked());
            let Ok(rgbimage) = qresult else {
                let msg = format!("Quantization failed: {qresult:?}");
                eprintln!("{}", msg);
                dialog::alert_default(&msg);
                return;
            };

            fr.set_image(Some(rgbimage));
            fr.set_label(&path.to_string_lossy());
            fr.changed();

            wn.set_label(&path.to_string_lossy());
        }
    }));

    openbtn.set_callback({
        let imagepath = Arc::clone(&imagepath_arc);
        let loadimage = Arc::clone(&loadimage_arc);
        move |_| {
            println!("Open button pressed");

            let Some(path) = get_file() else {
                eprintln!("No file selected/cancelled");
                return;
            };

            *(imagepath.write().unwrap()) = Some(path);
            loadimage.lock().unwrap()();
        }
    });

    clearbtn.set_callback({
        let clearimage = Arc::clone(&clearimage_arc);

        move |_| {
            println!("Clear button pressed");
            clearimage.lock().unwrap()();
        }
    });

    let loadimage_callback = {
        let loadimage = Arc::clone(&loadimage_arc);
        move |_btn : &mut CheckButton| {
            loadimage.lock().unwrap()();
        }
    };

    grayscale_toggle.set_callback(loadimage_callback.clone());
    grayscale_output_toggle.set_callback(loadimage_callback.clone());
    reorder_palette_toggle.set_callback(loadimage_callback.clone());

    col.end();
    row.end();
    wind.end();

    wind.make_resizable(true);
    wind.show();

    app.run()?;

    println!("App finished");
    Ok(())
}
