use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

use core_runtime::should_skip_browser_tests;
use core_runtime::BrowserClient;

const DIFF_THRESHOLD: f64 = 0.06;

#[test]
fn test_som_visual_regression_threshold() -> Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body style='margin:0;background:#f8f7f3;'>
                <div style='width:320px;height:180px;position:relative;background:#f8f7f3;'>
                    <div style='position:absolute;left:16px;top:16px;width:96px;height:48px;background:#1f6f8b;'></div>
                    <div style='position:absolute;left:128px;top:16px;width:176px;height:48px;background:#99c24d;'></div>
                    <button id='cta' style='position:absolute;left:16px;top:96px;width:288px;height:56px;border:0;background:#f18f01;color:#111;'>Go</button>
                </div>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let capture = page.get_visual()?;
    let artifact_dir = artifact_dir()?;
    fs::create_dir_all(&artifact_dir)?;

    let actual_path = artifact_dir.join("som_visual_actual.png");
    fs::write(&actual_path, &capture.image_png)?;

    let baseline_path = baseline_path();
    if should_update_baseline() {
        if let Some(parent) = baseline_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&baseline_path, &capture.image_png)?;
        return Ok(());
    }

    let baseline_png = fs::read(&baseline_path).with_context(|| {
        format!(
            "Missing baseline image: {}. Run with UPDATE_SOM_BASELINE=1 to create it.",
            baseline_path.display()
        )
    })?;

    let (diff_ratio, diff_image) = diff_ratio_and_image(&baseline_png, &capture.image_png)?;

    let diff_path = artifact_dir.join("som_visual_diff.png");
    diff_image.save(&diff_path)?;

    assert!(
        diff_ratio <= DIFF_THRESHOLD,
        "visual diff ratio {diff_ratio:.6} exceeded threshold {DIFF_THRESHOLD:.6}"
    );

    Ok(())
}

fn diff_ratio_and_image(baseline_png: &[u8], actual_png: &[u8]) -> Result<(f64, RgbaImage)> {
    let baseline_decoded = image::load_from_memory(baseline_png)
        .context("Failed to decode baseline PNG")?
        .to_rgba8();
    let actual_decoded = image::load_from_memory(actual_png)
        .context("Failed to decode actual PNG")?
        .to_rgba8();

    let (baseline_w, baseline_h) = baseline_decoded.dimensions();
    let (actual_w, actual_h) = actual_decoded.dimensions();
    let width = baseline_w.min(actual_w);
    let height = baseline_h.min(actual_h);
    if width == 0 || height == 0 {
        anyhow::bail!(
            "invalid image dimensions: baseline=({baseline_w},{baseline_h}), actual=({actual_w},{actual_h})"
        );
    }

    // Different environments can produce slightly different screenshot canvas sizes.
    // Compare the common top-left viewport area to keep regression checks stable.
    let baseline = image::imageops::crop_imm(&baseline_decoded, 0, 0, width, height).to_image();
    let actual = image::imageops::crop_imm(&actual_decoded, 0, 0, width, height).to_image();
    let mut diff = RgbaImage::new(width, height);

    let mut total = 0.0f64;
    for y in 0..height {
        for x in 0..width {
            let base = baseline.get_pixel(x, y).0;
            let curr = actual.get_pixel(x, y).0;

            let dr = base[0].abs_diff(curr[0]);
            let dg = base[1].abs_diff(curr[1]);
            let db = base[2].abs_diff(curr[2]);
            let da = base[3].abs_diff(curr[3]);

            let channel_sum = u16::from(dr) + u16::from(dg) + u16::from(db) + u16::from(da);
            total += f64::from(channel_sum) / (255.0 * 4.0);

            diff.put_pixel(x, y, image::Rgba([dr, dg, db, 255]));
        }
    }

    let pixels = f64::from(width) * f64::from(height);
    Ok((total / pixels, diff))
}

fn should_update_baseline() -> bool {
    std::env::var("UPDATE_SOM_BASELINE")
        .ok()
        .is_some_and(|value| value == "1")
}

fn artifact_dir() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("SOM_ARTIFACT_DIR") {
        return Ok(PathBuf::from(path));
    }

    let mut default_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    default_dir.push("../target/som-artifacts");
    Ok(default_dir)
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("som")
        .join("som_visual_baseline.png")
}

#[test]
fn test_diff_ratio_handles_max_channel_diff_without_overflow() -> Result<()> {
    let baseline = encode_solid_rgba_png([0, 0, 0, 255])?;
    let actual = encode_solid_rgba_png([255, 255, 255, 255])?;

    let (ratio, diff) = diff_ratio_and_image(&baseline, &actual)?;
    assert!((ratio - 0.75).abs() < f64::EPSILON, "ratio should be 0.75");
    assert_eq!(diff.get_pixel(0, 0).0, [255, 255, 255, 255]);

    Ok(())
}

fn encode_solid_rgba_png(pixel: [u8; 4]) -> Result<Vec<u8>> {
    let image = RgbaImage::from_pixel(1, 1, Rgba(pixel));
    let dynamic = DynamicImage::ImageRgba8(image);

    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);
    dynamic.write_to(&mut cursor, ImageFormat::Png)?;
    Ok(buffer)
}
