//! Image → terminal cells. Each cell shows two vertical pixels using "▄":
//! background = top pixel, foreground = bottom pixel.

use image::RgbImage;
use image::imageops::FilterType;

use crate::config::theme::Rgb;

/// Resample to `w` × `2h` pixels and pair rows into `h` rows of `w` cells.
pub fn to_cells(img: &RgbImage, w: u16, h: u16) -> Vec<Vec<(Rgb, Rgb)>> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let small = image::imageops::resize(img, w as u32, h as u32 * 2, FilterType::Triangle);
    (0..h as u32)
        .map(|row| {
            (0..w as u32)
                .map(|x| {
                    let t = small.get_pixel(x, row * 2).0;
                    let b = small.get_pixel(x, row * 2 + 1).0;
                    (Rgb(t[0], t[1], t[2]), Rgb(b[0], b[1], b[2]))
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_by_two_image_maps_to_one_row_of_two_cells() {
        let mut img = RgbImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        img.put_pixel(1, 0, image::Rgb([0, 255, 0]));
        img.put_pixel(0, 1, image::Rgb([0, 0, 255]));
        img.put_pixel(1, 1, image::Rgb([255, 255, 255]));
        let cells = to_cells(&img, 2, 1);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0][0], (Rgb(255, 0, 0), Rgb(0, 0, 255)));
        assert_eq!(cells[0][1], (Rgb(0, 255, 0), Rgb(255, 255, 255)));
        assert!(to_cells(&img, 0, 3).is_empty());
    }
}
