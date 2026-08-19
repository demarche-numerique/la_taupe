use std::env::{current_dir, var};

use image::{imageops::fast_blur, DynamicImage, GrayImage, ImageBuffer, Luma, Rgb};
use imageproc::{
    edges::canny,
    geometric_transformations::{warp_into, Interpolation, Projection},
    hough::{detect_lines, draw_polar_lines, LineDetectionOptions, PolarLine},
    map::map_pixels,
    morphology::{grayscale_dilate, Mask},
    stats::min_max,
};
use linfa::prelude::Predict;
use linfa::{traits::Fit, Dataset};
use linfa_clustering::KMeans;
use ndarray::Array2;

const WHITE: Rgb<u8> = Rgb::<u8>([255, 255, 255]);
const GREEN: Rgb<u8> = Rgb::<u8>([0, 255, 0]);
const BLACK: Rgb<u8> = Rgb::<u8>([0, 0, 0]);

pub fn only_rotate(image: &DynamicImage, name: &str) -> DynamicImage {
    crate::timing::measure(crate::timing::preprocess, || only_rotate_inner(image, name))
}

fn only_rotate_inner(image: &DynamicImage, name: &str) -> DynamicImage {
    let whithout_shadow_image = remove_shadows(image, name);
    let angle = angle(&whithout_shadow_image, name);

    rotate(image, -angle)
}

pub fn clean_image(image: &DynamicImage, name: &str) -> DynamicImage {
    crate::timing::measure(crate::timing::preprocess, || clean_image_inner(image, name))
}

fn clean_image_inner(image: &DynamicImage, name: &str) -> DynamicImage {
    let whithout_shadow_image = remove_shadows(image, name);
    let angle = angle(&whithout_shadow_image, name);

    let rotated_image = rotate(image, -angle);
    let image = remove_shadows(&rotated_image, name);

    save_image_in_debug(&image, name, "cleaned");

    image
}

fn remove_shadows(image: &DynamicImage, _name: &str) -> DynamicImage {
    let gray_image = image.clone().into_luma8();

    let dilated_image = grayscale_dilate(&gray_image, &Mask::diamond(7));

    let bg_image = fast_blur(&dilated_image, 21.0);

    let mut shadow_removed = gray_image.clone();
    for (x, y, pixel) in shadow_removed.enumerate_pixels_mut() {
        let bg_pixel = bg_image.get_pixel(x, y);
        let new_pixel = 255 - (pixel.0[0].abs_diff(bg_pixel.0[0]));
        *pixel = image::Luma([new_pixel]);
    }

    let normalized_image = normalize_minmax(&shadow_removed, 0, 255);

    let thresholded_image = apply_custom_threshold(&normalized_image, 220);

    thresholded_image.into()
}

fn apply_custom_threshold(img: &GrayImage, threshold: u8) -> GrayImage {
    ImageBuffer::from_fn(img.width(), img.height(), |x, y| {
        let pixel_value = img.get_pixel(x, y).0[0];

        if pixel_value > threshold {
            Luma([255])
        } else {
            Luma([pixel_value])
        }
    })
}

fn normalize_minmax(img: &GrayImage, alpha: u8, beta: u8) -> GrayImage {
    let min_max = min_max(img)[0];
    let (min_val, max_val) = (min_max.min, min_max.max);

    if min_val == max_val {
        return ImageBuffer::from_pixel(img.width(), img.height(), Luma([min_val]));
    }

    let range_input = max_val - min_val;
    let range_output = beta - alpha;

    ImageBuffer::from_fn(img.width(), img.height(), |x, y| {
        let pixel = img.get_pixel(x, y).0[0];

        let normalized =
            ((pixel - min_val) as f32 / range_input as f32) * range_output as f32 + alpha as f32;

        Luma([normalized.round() as u8])
    })
}

fn angle(image: &DynamicImage, name: &str) -> f32 {
    let gray_image = image.clone().into_luma8();
    let edges = canny(&gray_image, 50.0, 100.0);

    let lines = [400, 200, 100].iter().find_map(|&threshold| {
        let options = LineDetectionOptions {
            vote_threshold: threshold,
            suppression_radius: 0,
        };
        let lines: Vec<PolarLine> = detect_lines(&edges, options);

        if !lines.is_empty() {
            Some(lines)
        } else {
            None
        }
    });

    if lines.is_none() {
        log::warn!("No lines detected after retrying, returning 0.0");
        return 0.0;
    }

    let lines = lines.unwrap();

    if var("DEBUG_LINE_DETECTION").is_ok() {
        draw_lines(&edges.into(), &lines, name);
    }

    let angles: Vec<f64> = lines
        .iter()
        .map(|line| line.angle_in_degrees as f64)
        .collect();

    if angles.is_empty() {
        log::trace!("No angles detected, returning 0.0");
        return 0.0;
    }

    let angle = main_cluster_mean_angle(angles);
    log::trace!("angle detected: {}", angle);

    (angle - 90.0).to_radians() as f32
}

/// Projection appliquée par [`rotate`] : rotation autour du centre, sur un canevas
/// agrandi pour ne rien perdre. Exposée pour transporter une position connue — une ancre
/// — dans l'image tournée sans la redétecter.
pub fn rotation_projection(width: u32, height: u32, theta: f32) -> Projection {
    let (width, height) = (width as f32, height as f32);
    let (new_width, new_height) = (
        (width * theta.cos().abs() + height * theta.sin().abs()),
        (height * theta.cos().abs() + width * theta.sin().abs()),
    );

    let (cx, cy) = (width / 2f32, height / 2f32);
    let (new_cx, new_cy) = ((new_width / 2f32), (new_height / 2f32));

    Projection::translate(new_cx, new_cy)
        * Projection::rotate(theta)
        * Projection::translate(-cx, -cy)
}

/// Rectangle englobant d'un rectangle transporté par [`rotation_projection`].
pub fn rotate_rect(
    (x, y, w, h): (u32, u32, u32, u32),
    width: u32,
    height: u32,
    theta: f32,
) -> (u32, u32, u32, u32) {
    let projection = rotation_projection(width, height, theta);
    let corners = [
        (x as f32, y as f32),
        ((x + w) as f32, y as f32),
        ((x + w) as f32, (y + h) as f32),
        (x as f32, (y + h) as f32),
    ];

    let mapped: Vec<(f32, f32)> = corners
        .iter()
        .map(|&(px, py)| projection * (px, py))
        .collect();
    let min_x = mapped.iter().map(|p| p.0).fold(f32::MAX, f32::min).max(0.0);
    let min_y = mapped.iter().map(|p| p.1).fold(f32::MAX, f32::min).max(0.0);
    let max_x = mapped.iter().map(|p| p.0).fold(f32::MIN, f32::max);
    let max_y = mapped.iter().map(|p| p.1).fold(f32::MIN, f32::max);

    (
        min_x as u32,
        min_y as u32,
        (max_x - min_x).max(1.0) as u32,
        (max_y - min_y).max(1.0) as u32,
    )
}

pub fn rotate(image: &DynamicImage, theta: f32) -> DynamicImage {
    let image = image.to_rgb8();

    let (width, height) = (image.width() as f32, image.height() as f32);
    let (new_width, new_height) = (
        (width * theta.cos().abs() + height * theta.sin().abs()),
        (height * theta.cos().abs() + width * theta.sin().abs()),
    );

    let mut new_image = ImageBuffer::from_pixel(new_width as u32, new_height as u32, WHITE);
    let projection = rotation_projection(image.width(), image.height(), theta);

    warp_into(
        &image,
        &projection,
        Interpolation::Bicubic,
        WHITE,
        &mut new_image,
    );

    new_image.into()
}

fn main_cluster_mean_angle(angles: Vec<f64>) -> f64 {
    let n = angles.len();
    let array: Array2<f64> = Array2::from_shape_vec((n, 1), angles.to_vec()).unwrap();
    let dataset = Dataset::from(array.clone());

    let model = KMeans::params(2)
        .max_n_iterations(100)
        .fit(&dataset)
        .expect("Failed to fit KMeans model");

    let labels = model.predict(array.clone());

    let mut cluster_0 = Vec::new();
    let mut cluster_1 = Vec::new();

    for (i, &label) in labels.targets.iter().enumerate() {
        if label == 0 {
            cluster_0.push(angles[i]);
        } else {
            cluster_1.push(angles[i]);
        }
    }

    let values = if cluster_0.len() >= cluster_1.len() {
        cluster_0
    } else {
        cluster_1
    };

    values.iter().sum::<f64>() / values.len() as f64
}

pub fn resize(img: &DynamicImage, initial_height: u32, target_height: u32) -> DynamicImage {
    let resize_factor = target_height as f32 / initial_height as f32;

    log::trace!(
        "Resizing image from {}x{} to {}x{} with factor {}",
        img.width(),
        img.height(),
        (img.width() as f32 * resize_factor) as u32,
        (img.height() as f32 * resize_factor) as u32,
        resize_factor
    );

    img.resize(
        (img.width() as f32 * resize_factor) as u32,
        (img.height() as f32 * resize_factor) as u32,
        image::imageops::FilterType::Lanczos3,
    )
}

fn draw_lines(edges: &DynamicImage, lines: &[PolarLine], name: &str) {
    let color_edges = map_pixels(edges, |_x, _y, p| if p[0] > 0 { WHITE } else { BLACK });
    let lines_image = draw_polar_lines(&color_edges, lines, GREEN);
    save_image_in_debug(&lines_image.into(), name, "lines");
}

pub fn save_image_in_debug(image: &DynamicImage, name: &str, suffix: &str) {
    if var("DEBUG_IMAGE").is_err() {
        return;
    }

    let debug_dir = current_dir()
        .expect("Failed to get current directory")
        .join("la_taupe_debug");

    std::fs::create_dir_all(&debug_dir).unwrap_or_else(|e| {
        panic!(
            "Failed to create debug directory: {}, {}",
            debug_dir.display(),
            e
        )
    });

    // name without the extension .jpeg or .png
    let name = name.rsplit_once('.').map_or(name, |(name, _)| name);

    image
        .save(debug_dir.join(format!("{}_{}.png", name, suffix)))
        .unwrap_or_else(|e| panic!("Failed to save image: {}, {}", name, e));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rect_rotated_by_zero_is_itself() {
        assert_eq!(
            rotate_rect((10, 20, 30, 40), 200, 100, 0.0),
            (10, 20, 30, 40)
        );
    }

    /// Un quart de tour échange largeur et hauteur du rectangle, et le canevas s'élargit
    /// pour tout contenir : le rectangle reste dans les bornes.
    #[test]
    fn a_quarter_turn_swaps_the_sides() {
        let (_, _, w, h) = rotate_rect((10, 20, 30, 40), 200, 100, std::f32::consts::FRAC_PI_2);

        assert!((w as i32 - 40).abs() <= 1, "w={}", w);
        assert!((h as i32 - 30).abs() <= 1, "h={}", h);
    }

    /// Le rectangle transporté doit correspondre à l'image transportée : on vérifie que
    /// le centre du rectangle tourné est le centre de l'original passé par la même
    /// projection.
    #[test]
    fn the_transported_rect_follows_the_image() {
        let theta = 0.3;
        let (x, y, w, h) = rotate_rect((50, 60, 20, 10), 300, 200, theta);
        let center = ((x + w / 2) as f32, (y + h / 2) as f32);

        let expected = rotation_projection(300, 200, theta) * (60.0, 65.0);

        assert!(
            (center.0 - expected.0).abs() < 2.0,
            "{:?} vs {:?}",
            center,
            expected
        );
        assert!(
            (center.1 - expected.1).abs() < 2.0,
            "{:?} vs {:?}",
            center,
            expected
        );
    }
}
