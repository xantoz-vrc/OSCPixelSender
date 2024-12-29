extern crate png;
extern crate quantizr;

use std::error::Error;
use std::path::Path;
use std::fs::File;
use std::io::BufWriter;
use std::num::NonZero;

pub fn save_png(
    path: &Path,
    width: NonZero<u32>, height: NonZero<u32>,
    indexes: &[u8], palette: &[quantizr::Color]
) -> Result<(), Box<dyn Error>> {

    let palette: Vec<u8> = palette.iter().flat_map(|c| [c.r, c.g, c.b]).collect();

    let file = File::create(path.with_extension("png"))?;
    let ref mut bufw = BufWriter::new(file);

    let mut encoder = png::Encoder::new(bufw, width.into(), height.into());
    encoder.set_color(png::ColorType::Indexed); // TODO: Add option to set to grayscale as well
    encoder.set_depth(png::BitDepth::Eight); // TODO: Add options for even lower bit-depths
    encoder.set_palette(&palette);

    let mut writer = encoder.write_header()?;

    writer.write_image_data(&indexes)?;

    Ok(())
}
