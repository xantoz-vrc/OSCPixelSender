extern crate png;
extern crate quantizr;

use std::error::Error;
use std::path::Path;
use std::fs::File;
use std::io::BufWriter;
use std::num::NonZero;

#[derive(Debug, Clone, PartialEq)]
pub enum ColorType {
    Grayscale,
    Indexed,
}

pub fn save_png(
    path: &Path,
    width: NonZero<u32>, height: NonZero<u32>,
    indexes: &[u8], palette: &[quantizr::Color],
    colortype: ColorType,
) -> Result<(), Box<dyn Error>> {

    let png_palette: Vec<u8>;
    let png_data: Vec<u8>;

    let file = File::create(path).
        map_err(|err| format!("Couldn't create file: {err}"))?;
    let ref mut bufw = BufWriter::new(file);

    let bitdepth = {
        match palette.len() {
            ..=2     => png::BitDepth::One,
            ..=4     => png::BitDepth::Two,
            ..=16    => png::BitDepth::Four,
            ..=256   => png::BitDepth::Eight,
            // ..=65536 => png::BitDepth::Sixteen,
            ..=65536 => return Err("16bpp currently not supported".into()),
            // _ => return Err(Box::new(PngError::TooLargePalette)),
            _ => return Err("Too large palette".into()),
        }
    };

    // We need to do the conversion per line, because it might happen
    // that the width doesn't divide evenly when we are using 4bpp,
    // 2bpp or 1bpp modes. In that case each line must be padded out
    // some pixels.
    let data: &[u8] = match bitdepth {
        png::BitDepth::One => {
            png_data = indexes
                .chunks_exact(u32::try_into(width.into())?)
                .flat_map(|line|
                          line.chunks(8)
                          .map(|p|
                               p.get(0).map_or(0, |v| (v & 0b1) << 7) |
                               p.get(1).map_or(0, |v| (v & 0b1) << 6) |
                               p.get(2).map_or(0, |v| (v & 0b1) << 5) |
                               p.get(3).map_or(0, |v| (v & 0b1) << 4) |
                               p.get(4).map_or(0, |v| (v & 0b1) << 3) |
                               p.get(5).map_or(0, |v| (v & 0b1) << 2) |
                               p.get(6).map_or(0, |v| (v & 0b1) << 1) |
                               p.get(7).map_or(0, |v| (v & 0b1) << 0))
                ).collect();
            &png_data
        },
        png::BitDepth::Two => {
            png_data = indexes
                .chunks_exact(u32::try_into(width.into())?)
                .flat_map(|line|
                          line.chunks(4)
                          .map(|p|
                               p.get(0).map_or(0, |v| (v & 0b11) << 6) |
                               p.get(1).map_or(0, |v| (v & 0b11) << 4) |
                               p.get(2).map_or(0, |v| (v & 0b11) << 2) |
                               p.get(3).map_or(0, |v| (v & 0b11) << 0))
                ).collect();
            &png_data
        },
        png::BitDepth::Four => {
            png_data = indexes
                .chunks_exact(u32::try_into(width.into())?)
                .flat_map(|line|
                          line.chunks(2)
                          .map(|p|
                               p.get(0).map_or(0, |v| (v & 0b1111) << 4) |
                               p.get(1).map_or(0, |v| (v & 0b1111) << 0))
                ).collect();
            &png_data
        },
        png::BitDepth::Eight => indexes,
        png::BitDepth::Sixteen => return Err("Unsupported bitdepth".into()),
    };

    let mut encoder = png::Encoder::new(bufw, width.into(), height.into());
    if colortype == ColorType::Indexed {
        png_palette = palette.iter().flat_map(|c| [c.r, c.g, c.b]).collect();
        encoder.set_palette(&png_palette);
    }
    let typ = match colortype {
        ColorType::Grayscale => png::ColorType::Grayscale,
        ColorType::Indexed => png::ColorType::Indexed,
    };
    encoder.set_color(typ);
    encoder.set_depth(bitdepth);
    encoder.set_compression(png::Compression::Best);
    encoder.set_adaptive_filter(png::AdaptiveFilterType::Adaptive);

    println!("Saving PNG of color {typ:?} with bit depth {bitdepth:?}");

    let mut writer = encoder.write_header()
        .map_err(|err| format!("Failed when writing header: {err}"))?;

    writer.write_image_data(data)
        .map_err(|err| format!("Failed when writing image data: {err}"))?;

    Ok(())
}
