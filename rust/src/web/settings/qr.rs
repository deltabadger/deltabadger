//! RQRCodeCore's version-12/H byte segment and legacy mask scoring, using qrcode's Reed-Solomon encoder.
use crate::web::WebError;
use qrcode::{
    bits::Bits,
    canvas::{Canvas, MaskPattern},
    Color, EcLevel, Version,
};
const WIDTH: usize = 65;
fn score(pixels: &[bool]) -> f64 {
    let at = |row: usize, col: usize| pixels[row * WIDTH + col];
    let mut points = 0u32;
    for row in 0..WIDTH {
        for col in 0..WIDTH {
            let mut same = 0;
            for r in row.saturating_sub(1)..=(row + 1).min(WIDTH - 1) {
                for c in col.saturating_sub(1)..=(col + 1).min(WIDTH - 1) {
                    if (r != row || c != col) && at(r, c) == at(row, col) {
                        same += 1;
                    }
                }
            }
            if same > 5 {
                points += 3 + same - 5;
            }
            if row + 1 < WIDTH
                && col + 1 < WIDTH
                && at(row, col) == at(row + 1, col)
                && at(row, col) == at(row, col + 1)
                && at(row, col) == at(row + 1, col + 1)
            {
                points += 3;
            }
            let pattern = [true, false, true, true, true, false, true];
            if col + 6 < WIDTH && (0..7).all(|i| at(row, col + i) == pattern[i]) {
                points += 40;
            }
            if row + 6 < WIDTH && (0..7).all(|i| at(row + i, col) == pattern[i]) {
                points += 40;
            }
        }
    }
    let dark = pixels.iter().filter(|&&p| p).count() as f64;
    f64::from(points) + (100.0 * dark / (WIDTH * WIDTH) as f64 - 50.0).abs() / 5.0 * 10.0
}
fn test_pixels(canvas: &Canvas) -> Vec<bool> {
    let mut pixels: Vec<_> = (0..WIDTH)
        .flat_map(|row| (0..WIDTH).map(move |col| canvas.get(col as i16, row as i16).is_dark()))
        .collect();
    // RQRCode make_impl(test=true): format/version words and the fixed dark module are light while scoring.
    for i in 0..18 {
        pixels[(i / 3) * WIDTH + (i % 3 + WIDTH - 11)] = false;
        pixels[(i % 3 + WIDTH - 11) * WIDTH + i / 3] = false;
    }
    for i in 0..15 {
        let row = if i < 6 {
            i
        } else if i < 8 {
            i + 1
        } else {
            WIDTH - 15 + i
        };
        let col = if i < 8 {
            WIDTH - i - 1
        } else if i == 8 {
            7
        } else {
            15 - i - 1
        };
        pixels[row * WIDTH + 8] = false;
        pixels[8 * WIDTH + col] = false;
    }
    pixels[(WIDTH - 8) * WIDTH + 8] = false;
    pixels
}
pub fn colors(uri: &str) -> Result<Vec<Color>, WebError> {
    let mut bits = Bits::new(Version::Normal(12));
    bits.push_byte_data(uri.as_bytes())
        .and_then(|_| bits.push_terminator(EcLevel::H))
        .map_err(|_| WebError::Config("cannot encode enrollment QR".into()))?;
    let (data, ec) =
        qrcode::ec::construct_codewords(&bits.into_bytes(), Version::Normal(12), EcLevel::H)
            .map_err(|_| WebError::Config("cannot encode enrollment QR".into()))?;
    let mut canvas = Canvas::new(Version::Normal(12), EcLevel::H);
    canvas.draw_all_functional_patterns();
    canvas.draw_data(&data, &ec);
    let patterns = [
        MaskPattern::Checkerboard,
        MaskPattern::HorizontalLines,
        MaskPattern::VerticalLines,
        MaskPattern::DiagonalLines,
        MaskPattern::LargeCheckerboard,
        MaskPattern::Fields,
        MaskPattern::Diamonds,
        MaskPattern::Meadow,
    ];
    let mut best = None;
    for pattern in patterns {
        let mut candidate = canvas.clone();
        candidate.apply_mask(pattern);
        let points = score(&test_pixels(&candidate));
        if best.as_ref().is_none_or(|(old, _)| points < *old) {
            best = Some((points, candidate));
        }
    }
    let (_, best) = best.ok_or_else(|| WebError::Config("enrollment QR has no mask".into()))?;
    Ok(best.into_colors())
}
