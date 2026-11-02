#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod diff;
mod git;
mod graph;
mod platform;
mod ui;

fn main() -> eframe::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1172.0, 636.0])
            .with_min_inner_size([960.0, 600.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "GitCrab",
        options,
        Box::new(move |cc| Ok(Box::new(ui::GitCrab::new(cc, path)))),
    )
}

fn app_icon() -> eframe::egui::IconData {
    let mut rgba = vec![0; 32 * 32 * 4];
    for y in 0_i32..32 {
        for x in 0_i32..32 {
            let shell = (x - 16).pow(2) + (y - 18).pow(2) < 70;
            let claw =
                ((x - 5).pow(2) + (y - 9).pow(2) < 17) || ((x - 27).pow(2) + (y - 9).pow(2) < 17);
            let legs = (8..=24).contains(&y)
                && ((x + y == 24) || (x - y == 8) || (x + y == 29) || (x - y == 3));
            let eye = (y == 14 || y == 15) && (x == 12 || x == 20);
            let c = if eye {
                [28, 30, 35, 255]
            } else if shell || claw || legs {
                [21, 160, 191, 255]
            } else {
                [0, 0, 0, 0]
            };
            let i = ((y * 32 + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&c);
        }
    }
    eframe::egui::IconData {
        rgba,
        width: 32,
        height: 32,
    }
}
