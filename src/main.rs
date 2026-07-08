mod api;
mod capture;
mod config;
mod notify;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

use config::Config;

const CAPTURE_ID: &str = "capture";
const REFRESH_ID: &str = "refresh";
const QUIT_ID: &str = "quit";
const MODEL_PREFIX: &str = "model:";
const IDLE_TITLE: &str = "";
const BUSY_TITLE: &str = "OCRing";

/// Everything that can happen off the main thread and needs to be handled
/// back on the tao event loop.
enum UserEvent {
    Menu(MenuEvent),
    HotKey(GlobalHotKeyEvent),
    ModelsFetched(Result<Vec<String>, String>),
    OcrStarted,
    OcrFinished { model: String, result: Result<(String, f64), String> },
}

struct AppState {
    tray_icon: TrayIcon,
    config: Config,
    api_key: String,
    user_id: String,
    current_model: String,
    models: Vec<String>,
    models_are_fallback: bool,
}

impl AppState {
    fn rebuild_menu(&self) {
        let menu = Menu::new();

        let capture_item = MenuItem::with_id(CAPTURE_ID, "キャプチャしてOCR (⌥⌘8)", true, None);
        let _ = menu.append(&capture_item);
        let _ = menu.append(&PredefinedMenuItem::separator());

        let model_submenu = tray_icon::menu::Submenu::new("モデル", true);
        for model in &self.models {
            let id = format!("{MODEL_PREFIX}{model}");
            let checked = model == &self.current_model;
            let item = CheckMenuItem::with_id(id, model, true, checked, None);
            let _ = model_submenu.append(&item);
        }
        if self.models_are_fallback {
            let notice = MenuItem::new("(一覧取得失敗・既定リスト)", false, None);
            let _ = model_submenu.append(&notice);
        }
        let _ = menu.append(&model_submenu);

        let refresh_item = MenuItem::with_id(REFRESH_ID, "モデル一覧を再取得", true, None);
        let _ = menu.append(&refresh_item);

        let _ = menu.append(&PredefinedMenuItem::separator());
        let quit_item = MenuItem::with_id(QUIT_ID, "終了", true, None);
        let _ = menu.append(&quit_item);

        self.tray_icon.set_menu(Some(Box::new(menu)));
    }

    fn set_busy_title(&self, busy: bool) {
        let title = if busy { BUSY_TITLE } else { IDLE_TITLE };
        self.tray_icon.set_title(Some(title));
    }
}

/// A minimal solid-color RGBA icon, generated at runtime (no asset files).
/// テンプレートアイコン(黒+透過)。template 指定によりメニューバーの
/// ライト/ダーク外観に合わせて macOS 側が自動で色を反転する。
/// モチーフ: スキャン枠(四隅のブラケット) + テキスト行
fn make_icon() -> Icon {
    const SIZE: u32 = 22;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let mut set = |x: u32, y: u32| {
        if x < SIZE && y < SIZE {
            let i = ((y * SIZE + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
        }
    };

    const T: u32 = 2; // 線の太さ
    const ARM: u32 = 6; // ブラケットの腕の長さ
    const IN: u32 = 2; // 外周からのインセット
    let max = SIZE - 1 - IN;
    // 四隅のブラケット
    for a in 0..ARM {
        for t in 0..T {
            // 上左・上右
            set(IN + a, IN + t);
            set(max - a, IN + t);
            set(IN + t, IN + a);
            set(max - t, IN + a);
            // 下左・下右
            set(IN + a, max - t);
            set(max - a, max - t);
            set(IN + t, max - a);
            set(max - t, max - a);
        }
    }
    // テキスト行(長・短)
    for x in 7..15 {
        set(x, 9);
        set(x, 10);
    }
    for x in 7..12 {
        set(x, 13);
        set(x, 14);
    }

    Icon::from_rgba(rgba, SIZE, SIZE).expect("failed to build tray icon")
}

fn fail_startup(message: &str) -> ! {
    eprintln!("snap-ocr: {message}");
    notify::notify("snap-ocr", message);
    std::process::exit(1);
}

fn main() {
    // .env は実行ディレクトリ→バイナリ隣接の順で探す(cargo run と .app 起動の両対応)
    if dotenvy::dotenv().is_err() {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let _ = dotenvy::from_path(dir.join(".env"));
            }
        }
    }

    let mut config = Config::load().unwrap_or_else(|e| {
        eprintln!("snap-ocr: failed to load config: {e}");
        Config::default()
    });

    let api_key = match config::resolve_api_key() {
        Some(key) => key,
        None => {
            eprintln!("{}", config::SETUP_HELP);
            fail_startup("APIキーが見つかりません。ターミナルの案内を確認してください。");
        }
    };

    let user_id = match config::resolve_user_id(&mut config) {
        Some(id) => id,
        None => {
            eprintln!("{}", config::SETUP_HELP);
            fail_startup("userId が見つかりません。ターミナルの案内を確認してください。");
        }
    };

    let current_model = config
        .model
        .clone()
        .unwrap_or_else(|| config::DEFAULT_MODEL.to_string());

    let (models, models_are_fallback) = match api::fetch_models(&api_key, &user_id) {
        Ok(models) if !models.is_empty() => (models, false),
        _ => (
            config::FALLBACK_MODELS.iter().map(|s| s.to_string()).collect(),
            true,
        ),
    };
    // If the persisted/default model isn't in the fetched list, fall back to
    // the first available one so the check mark always lands somewhere real.
    let current_model = if models.contains(&current_model) {
        current_model
    } else {
        models.first().cloned().unwrap_or(current_model)
    };

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);

    let hotkeys_manager = GlobalHotKeyManager::new().expect("failed to init global hotkey manager");
    // ⌃⌘8 は他プロセスに先取りされて届かない環境だったため ⌥⌘8 を採用(2026-07-08 実測)
    let candidates: Vec<(&str, HotKey)> = vec![
        ("⌥⌘8", HotKey::new(Some(Modifiers::SUPER | Modifiers::ALT), Code::Digit8)),
    ];
    let mut hotkey_ids: Vec<(u32, &str)> = Vec::new();
    for (label, hk) in &candidates {
        match hotkeys_manager.register(*hk) {
            Ok(()) => {
                eprintln!("snap-ocr: global hotkey {label} registered (id={})", hk.id());
                hotkey_ids.push((hk.id(), label));
            }
            Err(e) => eprintln!("snap-ocr: failed to register {label}: {e}"),
        }
    }

    let proxy = event_loop.create_proxy();
    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = menu_proxy.send_event(UserEvent::Menu(event));
    }));

    // ControlFlow::Wait 中はループが寝るので、ホットキーも proxy 経由で起こす
    let hotkey_proxy = proxy.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        eprintln!("snap-ocr: hotkey event received (id={}, state={:?})", event.id, event.state);
        let _ = hotkey_proxy.send_event(UserEvent::HotKey(event));
    }));

    let busy = Arc::new(AtomicBool::new(false));

    let mut state: Option<AppState> = None;
    let mut initial_models = Some((models, models_are_fallback));
    let mut initial_model = Some(current_model);
    let mut initial_config = Some(config);
    let mut initial_api_key = Some(api_key);
    let mut initial_user_id = Some(user_id);

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        if let Event::NewEvents(tao::event::StartCause::Init) = event {
            // Tray icons must be created only once the event loop is running.
            let (models, models_are_fallback) = initial_models.take().unwrap();
            let tray_icon = TrayIconBuilder::new()
                .with_icon(make_icon())
                .with_icon_as_template(true)
                .with_tooltip("snap-ocr")
                .with_title(IDLE_TITLE)
                .build()
                .expect("failed to build tray icon");

            let app_state = AppState {
                tray_icon,
                config: initial_config.take().unwrap(),
                api_key: initial_api_key.take().unwrap(),
                user_id: initial_user_id.take().unwrap(),
                current_model: initial_model.take().unwrap(),
                models,
                models_are_fallback,
            };
            app_state.rebuild_menu();
            state = Some(app_state);
            return;
        }

        match event {
            Event::UserEvent(UserEvent::HotKey(hk_event)) => {
                if hk_event.state == HotKeyState::Pressed {
                    if let Some((_, label)) = hotkey_ids.iter().find(|(id, _)| *id == hk_event.id) {
                        eprintln!("snap-ocr: hotkey {label} pressed -> capture");
                        trigger_capture(&busy, &proxy, &state);
                    }
                }
            }
            Event::UserEvent(UserEvent::Menu(menu_event)) => {
                handle_menu_event(menu_event.id(), &mut state, &busy, &proxy, control_flow);
            }
            Event::UserEvent(UserEvent::ModelsFetched(result)) => {
                if let Some(app_state) = state.as_mut() {
                    match result {
                        Ok(models) if !models.is_empty() => {
                            if !models.contains(&app_state.current_model) {
                                app_state.current_model =
                                    models.first().cloned().unwrap_or_default();
                                let _ = app_state.config.set_model(&app_state.current_model);
                            }
                            app_state.models = models;
                            app_state.models_are_fallback = false;
                        }
                        _ => {
                            app_state.models = config::FALLBACK_MODELS
                                .iter()
                                .map(|s| s.to_string())
                                .collect();
                            app_state.models_are_fallback = true;
                            if !app_state.models.contains(&app_state.current_model) {
                                app_state.current_model = app_state.models[0].clone();
                                let _ = app_state.config.set_model(&app_state.current_model);
                            }
                        }
                    }
                    app_state.rebuild_menu();
                }
            }
            Event::UserEvent(UserEvent::OcrStarted) => {
                if let Some(app_state) = state.as_ref() {
                    app_state.set_busy_title(true);
                }
            }
            Event::UserEvent(UserEvent::OcrFinished { model, result }) => {
                if let Some(app_state) = state.as_ref() {
                    app_state.set_busy_title(false);
                }
                busy.store(false, Ordering::SeqCst);
                match result {
                    Ok((_text, elapsed)) => {
                        notify::notify(
                            "snap-ocr",
                            &format!("クリップボードにコピーしました ({elapsed:.1}s / {model})"),
                        );
                    }
                    Err(err) => {
                        notify::notify("snap-ocr", &format!("OCR失敗: {err}"));
                    }
                }
            }
            _ => {}
        }
    });
}

fn trigger_capture(
    busy: &Arc<AtomicBool>,
    proxy: &tao::event_loop::EventLoopProxy<UserEvent>,
    state: &Option<AppState>,
) {
    eprintln!("snap-ocr: trigger_capture called");
    if busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        // Already running an OCR job: ignore re-entrant hotkey presses.
        return;
    }
    let Some(app_state) = state.as_ref() else {
        busy.store(false, Ordering::SeqCst);
        return;
    };

    let api_key = app_state.api_key.clone();
    let user_id = app_state.user_id.clone();
    let model = app_state.current_model.clone();
    let proxy = proxy.clone();
    let busy = busy.clone();

    std::thread::spawn(move || {
        let path = match capture::capture_screenshot() {
            Ok(Some(path)) => path,
            Ok(None) => {
                // Cancelled by the user (Esc): stay quiet.
                busy.store(false, Ordering::SeqCst);
                return;
            }
            Err(e) => {
                busy.store(false, Ordering::SeqCst);
                let _ = proxy.send_event(UserEvent::OcrFinished {
                    model,
                    result: Err(format!("スクリーンショット取得に失敗: {e}")),
                });
                return;
            }
        };

        let _ = proxy.send_event(UserEvent::OcrStarted);

        let started = Instant::now();
        let outcome = api::ocr_image(&api_key, &user_id, &model, &path);
        let elapsed = started.elapsed().as_secs_f64();

        capture::cleanup(&path);

        let result = match outcome {
            Ok(text) => {
                if let Err(e) = set_clipboard(&text) {
                    Err(format!("クリップボードへのコピーに失敗: {e}"))
                } else {
                    Ok((text, elapsed))
                }
            }
            Err(e) => Err(e.to_string()),
        };

        let _ = proxy.send_event(UserEvent::OcrFinished { model, result });
    });
}

fn set_clipboard(text: &str) -> anyhow::Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}

fn handle_menu_event(
    id: &MenuId,
    state: &mut Option<AppState>,
    busy: &Arc<AtomicBool>,
    proxy: &tao::event_loop::EventLoopProxy<UserEvent>,
    control_flow: &mut ControlFlow,
) {
    let id_str = id.as_ref();

    if id_str == QUIT_ID {
        *control_flow = ControlFlow::Exit;
        return;
    }

    if id_str == CAPTURE_ID {
        trigger_capture(busy, proxy, state);
        return;
    }

    if id_str == REFRESH_ID {
        let Some(app_state) = state.as_ref() else { return };
        let api_key = app_state.api_key.clone();
        let user_id = app_state.user_id.clone();
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let result = api::fetch_models(&api_key, &user_id).map_err(|e| e.to_string());
            let _ = proxy.send_event(UserEvent::ModelsFetched(result));
        });
        return;
    }

    if let Some(model) = id_str.strip_prefix(MODEL_PREFIX) {
        let Some(app_state) = state.as_mut() else { return };
        if app_state.current_model != model {
            app_state.current_model = model.to_string();
            let _ = app_state.config.set_model(model);
            app_state.rebuild_menu();
        }
    }
}
