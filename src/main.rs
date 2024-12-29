use fltk::{app, frame::Frame, enums::FrameType, image::SharedImage, prelude::*, window::Window, group::*, button::Button, dialog};
use std::error::Error;
use std::rc::Rc;
use std::cell::RefCell;
use std::path::PathBuf;

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

fn main() -> Result<(), Box<dyn Error>> {
    let app = app::App::default().with_scheme(app::Scheme::Gleam);
    // let app = app::App::default().with_scheme(app::Scheme::Oxy);
    let mut wind = Window::default().with_size(800, 600);

    let mut row = Flex::default_fill().row();
    // row.set_margin(20);
    row.set_spacing(20);
    let frame = Rc::new(RefCell::new(Frame::default_fill()));
    frame.borrow_mut().set_frame(FrameType::DownBox);
    // let mut borrow = frame.borrow_mut();
    // borrow.set_frame(FrameType::DownBox);

    let mut col = Flex::default_fill().column();
    col.set_margin(20);
    let mut openbtn = Button::default().with_label("Open");
    let mut clearbtn = Button::default().with_label("Clear");

    row.fixed(&col, 200);
    col.fixed(&openbtn, 50);
    col.fixed(&clearbtn, 50);

    {
        let fr1 = Rc::clone(&frame);
        openbtn.set_callback(move |_| {
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
                    let mut fr = fr1.borrow_mut();

                    println!("(before scale) w,h: {},{}", image.width(), image.height());
                    image.scale(256, 256, true, true);
                    println!("(after scale) w,h: {},{}", image.width(), image.height());

                    fr.set_image(Some(image));
                    fr.set_label(&path.to_string_lossy());
                    fr.changed();

                    // fr.set_image_scaled(Some(image));
                    // fr.set_label(path.to_string_lossy());
                    // fr.changed();
                },
            };

        });
    }

    {
        let fr2 : Rc::<RefCell::<Frame>> = Rc::clone(&frame);
        clearbtn.set_callback(move |_| {
            println!("Clear button pressed");

            let mut fr = fr2.borrow_mut();
            fr.set_image(None::<SharedImage>);
            fr.set_label("Clear");
            fr.changed();
        });
    }

    col.end();
    row.end();
    wind.end();

    wind.make_resizable(true);
    wind.show();

    app.run()?;

    println!("App finished");
    Ok(())
}
