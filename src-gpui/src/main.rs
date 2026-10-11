// Release builds open no console window next to the application on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod branch_tracking;
mod chrome;
mod code_view;
mod command_palette;
mod diff;
mod file_browser;
mod format;
mod graph;
mod history;
mod history_view;
mod image_preview;
mod inspector;
mod integrations;
#[cfg(feature = "app-benchmark")]
mod perf;
mod preferences;
mod recent_repositories;
mod repository_context;
mod repository_open;
mod settings;
mod sidebar;
mod syntax;
mod tabs;
mod theme;
mod view_cache;
mod virtual_scroll;

use gpui_kit::{prelude::*, *};
use mangrove_core::Workspace;
#[cfg(feature = "app-benchmark")]
use std::time::Instant;
use std::{path::PathBuf, sync::Arc};

/// Bundle identifier, cache directory and Linux desktop entry name.
const APP_IDENTIFIER: &str = "dev.mangrove.gpui";

/// Launch configuration keeps normal CLI, cache and preferences independent
/// from the optional application benchmark.
struct Startup {
    path: Option<String>,
    cache: PathBuf,
    preferences: preferences::Preferences,
    #[cfg(feature = "app-benchmark")]
    started: Instant,
    #[cfg(feature = "app-benchmark")]
    benchmark: Option<perf::Configuration>,
}

impl Startup {
    fn load() -> Result<Self, String> {
        #[cfg(feature = "app-benchmark")]
        let started = Instant::now();
        #[cfg(feature = "app-benchmark")]
        let benchmark = perf::Configuration::from_env()?;
        #[cfg(feature = "app-benchmark")]
        let preferences = if benchmark.is_some() {
            preferences::Preferences {
                auto_fetch_interval: 0,
                ..Default::default()
            }
        } else {
            preferences::Preferences::load()
        };
        #[cfg(not(feature = "app-benchmark"))]
        let preferences = preferences::Preferences::load();
        let startup = Self {
            path: std::env::args().nth(1),
            cache: dirs::cache_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join(APP_IDENTIFIER),
            preferences,
            #[cfg(feature = "app-benchmark")]
            started,
            #[cfg(feature = "app-benchmark")]
            benchmark,
        };
        #[cfg(feature = "app-benchmark")]
        let startup = startup.for_benchmark();
        Ok(startup)
    }

    #[cfg(feature = "app-benchmark")]
    fn for_benchmark(mut self) -> Self {
        if let Some(configuration) = self.benchmark.as_ref() {
            self.path = Some(configuration.repository.clone());
            self.cache = configuration.cache.clone();
        }
        self
    }
}

fn main() {
    let startup = match Startup::load() {
        Ok(startup) => startup,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let engine = match Workspace::new(startup.cache) {
        Ok(engine) => Arc::new(engine),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let preferences = startup.preferences;
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            preferences.apply_palette(cx);
            app::bind_keys(cx);
            cx.on_action(|_: &app::Quit, cx| cx.quit());
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1440.), px(888.)), cx);
            if let Err(error) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(980.), px(608.))),
                    // Matches the Linux desktop entry, which provides the launcher icon.
                    app_id: Some(APP_IDENTIFIER.into()),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Mangrove".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    // Set the content size directly, avoiding initial macOS frame rounding.
                    window.resize(size(px(1440.), px(888.)));
                    cx.new(|cx| {
                        #[cfg(feature = "app-benchmark")]
                        {
                            app::Mangrove::new_with_benchmark(
                                engine,
                                preferences,
                                startup.path,
                                startup.started,
                                startup.benchmark,
                                window,
                                cx,
                            )
                        }
                        #[cfg(not(feature = "app-benchmark"))]
                        {
                            app::Mangrove::new(engine, preferences, startup.path, window, cx)
                        }
                    })
                },
            ) {
                eprintln!("Impossible d’ouvrir la fenêtre : {error}");
                cx.quit();
            }
            cx.activate(true);
        });
}
