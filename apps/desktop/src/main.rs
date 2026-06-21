mod app;
mod icon;

use app::DesktopApp;
use app_services::AppServices;
use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::net::TcpListener;
use tokio::runtime::Runtime;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() -> eframe::Result<()> {
    const APP_NAME: &str = "freedb";

    // Single-instance lock via TCP port
    let _lock = TcpListener::bind("127.0.0.1:19919").unwrap_or_else(|_| {
        eprintln!("freedb 已在运行中（端口 19919 已被占用）");
        std::process::exit(1);
    });

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let runtime = Runtime::new().expect("failed to create tokio runtime");
    let services = AppServices::new().expect("failed to initialize services");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_icon(icon::app_icon_data(256))
            .with_inner_size([1480.0, 920.0])
            .with_min_inner_size([1200.0, 760.0])
            .with_maximized(true),
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            configure_fonts(&cc.egui_ctx);
            Ok(Box::new(DesktopApp::new(runtime, services)))
        }),
    )
}

fn configure_fonts(ctx: &egui::Context) {
    // 修改焦点交出策略：阻止单击编辑器外部无关区域导致选中消失。
    // 默认是 SurrenderFocusOn::Clicks — 点击任意位置即交出焦点。
    // 改为 SurrenderFocusOn::Never — 只有明确请求才交焦点。
    ctx.memory_mut(|mem| {
        mem.options.input_options.surrender_focus_on = egui::SurrenderFocusOn::Never;
    });

    let mut fonts = FontDefinitions::default();

    for path in [
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("system-cjk".into(), FontData::from_owned(bytes).into());

            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "system-cjk".into());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push("system-cjk".into());
            break;
        }
    }

    ctx.set_fonts(fonts);
}
