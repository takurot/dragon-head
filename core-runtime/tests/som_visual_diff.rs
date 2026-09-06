use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

use core_runtime::BrowserClient;

// Calibrated for the 3-channel (RGB-only) diff computed by `diff_ratio_and_image`
// below — screenshots are always fully opaque (alpha constant at 255, so
// including it in the diff only ever adds zero and dilutes the ratio's
// ceiling from 1.0 to 0.75 for no signal). This is the same absolute RGB
// sensitivity as the previous 4-channel `0.06` threshold (0.06 * 4/3 = 0.08),
// not a loosening (issue #281).
const DIFF_THRESHOLD: f64 = 0.08;

/// `BrowserClient::new()` leaves the window size unset, so the captured
/// screenshot's canvas is whatever viewport Chrome defaults to — unspecified
/// and observed to differ across OS/Chrome builds (e.g. macOS 756x417 vs
/// Linux CI 780x437 for the same fixture). Pinning the window size below
/// (`new_with_window_size`) makes the capture deterministic, so a real
/// dimension mismatch now reliably indicates a capture/layout regression
/// rather than environment noise — only a couple of pixels of slack remain,
/// for encoder-level rounding (issue #281; Codex review on PR #322).
const FIXTURE_WINDOW_SIZE: (u32, u32) = (800, 600);
const MAX_DIMENSION_SLACK_PX: u32 = 2;

#[test]
fn test_som_visual_regression_threshold() -> Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new_with_window_size(FIXTURE_WINDOW_SIZE.0, FIXTURE_WINDOW_SIZE.1)?;
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

    // `navigate` resolves once the load event fires, but doesn't guarantee a
    // final layout/paint pass has settled — capturing immediately after was
    // a source of occasional single-frame diff noise. This fixture has no
    // fonts, images, or transitions, so a short fixed wait is sufficient
    // (issue #281).
    thread::sleep(Duration::from_millis(150));

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
    if baseline_w.abs_diff(actual_w) > MAX_DIMENSION_SLACK_PX
        || baseline_h.abs_diff(actual_h) > MAX_DIMENSION_SLACK_PX
    {
        anyhow::bail!(
            "screenshot dimensions diverged by more than {MAX_DIMENSION_SLACK_PX}px: \
             baseline=({baseline_w},{baseline_h}), actual=({actual_w},{actual_h}) — \
             the window size is pinned via new_with_window_size, so this most likely \
             indicates a real capture/layout regression"
        );
    }
    let width = baseline_w.min(actual_w);
    let height = baseline_h.min(actual_h);
    if width == 0 || height == 0 {
        anyhow::bail!(
            "invalid image dimensions: baseline=({baseline_w},{baseline_h}), actual=({actual_w},{actual_h})"
        );
    }

    // With the window size pinned (see FIXTURE_WINDOW_SIZE above), the only
    // expected size variance is a pixel or two of encoder-level rounding.
    // Compare the common top-left viewport area to keep regression checks
    // stable against that.
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

            // Screenshots are always fully opaque (alpha constant at 255),
            // so including alpha in the diff sum never adds real signal —
            // it only dilutes the ratio's achievable ceiling from 1.0 to
            // 0.75 (issue #281). Compare RGB only.
            let channel_sum = u16::from(dr) + u16::from(dg) + u16::from(db);
            total += f64::from(channel_sum) / (255.0 * 3.0);

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
    assert!(
        (ratio - 1.0).abs() < f64::EPSILON,
        "RGB-only ratio should reach the full 1.0 ceiling, got {ratio}"
    );
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
