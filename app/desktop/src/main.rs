#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use anyhow::{Context as _, Result, ensure};
use forever_core::*;
use gpui::{div, prelude::*, px, rgb, size, *};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Default, Clone, Serialize, Deserialize)]
struct Preferences {
    language: String,
    game: Option<PathBuf>,
    recipe: Option<PathBuf>,
    nickname: String,
    #[serde(default)]
    update_channel: Option<UpdateChannel>,
    #[serde(default)]
    use_local_release: bool,
    #[serde(default)]
    resume_update: Option<ResumeUpdate>,
}
#[derive(Clone, Serialize, Deserialize)]
struct ResumeUpdate {
    expected_app_version: String,
    game: PathBuf,
    plan: Plan,
    replace: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateChannel {
    repository: String,
    public_key: String,
}
fn official_channel() -> UpdateChannel {
    UpdateChannel {
        repository: "rzxx/forever-smp-releases".into(),
        public_key: "c2b7643dbcf1a00f0d4c63ab022b32ecdc3417aadf67a2ec5394526c463dc3e4".into(),
    }
}
impl Preferences {
    fn channel(&self) -> UpdateChannel {
        self.update_channel.clone().unwrap_or_else(official_channel)
    }
}
#[derive(Clone, Copy)]
enum Selection {
    Folder,
    Release,
    Channel,
}
#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Language,
    Source,
    Folder,
    Review,
    Options,
    Busy,
    Access,
    Ready,
    CheckResult,
    Error,
}
struct Wizard {
    prefs: Preferences,
    step: Step,
    release: Option<Release>,
    plan: Option<Plan>,
    choices: BTreeMap<String, bool>,
    recommended: bool,
    replace: bool,
    error: String,
    progress: Arc<Mutex<String>>,
    focus: FocusHandle,
    request_copied: bool,
    feedback: Option<String>,
    advanced: bool,
    app_update: Option<AppOffer>,
    folder_return: Option<Step>,
}
fn fitted_window_bounds(display: Bounds<Pixels>) -> Bounds<Pixels> {
    let width = (f32::from(display.size.width) - 48.).clamp(1., 660.);
    let height = (f32::from(display.size.height) - 96.).clamp(1., 680.);
    Bounds::centered_at(display.center(), size(px(width), px(height)))
}
#[cfg(not(test))]
fn preference_path() -> PathBuf {
    std::env::var_os("FOREVER_SMP_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("ForeverSMP")
        })
        .join("preferences.json")
}
#[cfg(test)]
fn preference_path() -> PathBuf {
    static PROFILE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    PROFILE
        .get_or_init(|| tempfile::tempdir().unwrap())
        .path()
        .join("preferences.json")
}
fn bundled_recipe() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("release.json")))
        .filter(|p| p.is_file())
}
fn plan_for_resume(game: &std::path::Path, approved: &Plan) -> Result<Plan> {
    let current = plan(
        game,
        &approved.release,
        approved.choices.clone(),
        approved.recommended,
    )?;
    let committed = current.previous.as_ref().is_some_and(|state| {
        state.version == approved.release.version
            && state.choices == approved.choices
            && state.recommended == approved.recommended
    }) && current.install.is_empty()
        && current.remove.is_empty()
        && current.presets.is_empty()
        && current.conflicts.is_empty();
    Ok(if committed { current } else { approved.clone() })
}
impl Wizard {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut prefs: Preferences = fs::read(preference_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if let Some(bundled) = bundled_recipe() {
            let version = |path: &PathBuf| {
                fs::read(path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Release>(&b).ok())
                    .and_then(|r| semver::Version::parse(&r.version).ok())
            };
            if prefs.recipe.as_ref().and_then(version) <= version(&bundled) {
                prefs.recipe = Some(bundled);
            }
        }
        let step = if prefs.language.is_empty() {
            Step::Language
        } else {
            Step::Source
        };
        Self {
            prefs,
            step,
            release: None,
            plan: None,
            choices: BTreeMap::new(),
            recommended: true,
            replace: false,
            error: String::new(),
            progress: Arc::default(),
            focus: cx.focus_handle(),
            request_copied: false,
            feedback: None,
            advanced: false,
            app_update: None,
            folder_return: None,
        }
    }
    fn t<'a>(&self, en: &'a str, ru: &'a str) -> &'a str {
        if self.prefs.language == "ru" { ru } else { en }
    }
    fn save(&mut self) -> Result<()> {
        let path = preference_path();
        fs::create_dir_all(path.parent().unwrap())?;
        save_json_atomic(&path, &self.prefs)?;
        Ok(())
    }
    fn fail(&mut self, e: impl std::fmt::Display, cx: &mut Context<Self>) {
        self.feedback = None;
        self.error = e.to_string();
        self.step = Step::Error;
        cx.notify();
    }
    fn open_folder(&mut self, cx: &mut Context<Self>) {
        self.folder_return = Some(self.step);
        self.step = Step::Folder;
        cx.notify();
    }
    fn select_game_folder(&mut self, path: PathBuf) -> Result<()> {
        let previous = self.prefs.game.replace(path);
        if let Err(error) = self.make_plan() {
            self.prefs.game = previous;
            return Err(error);
        }
        self.save()?;
        self.feedback = None;
        self.folder_return = None;
        self.choose_review_step();
        Ok(())
    }
    fn selected_folder(&self) -> Stateful<Div> {
        div()
            .id("selected-folder")
            .debug_selector(|| "selected-folder".into())
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_md()
            .bg(rgb(0x1c2a22))
            .text_sm()
            .child(self.t("Game folder", "Папка игры"))
            .child(
                self.prefs
                    .game
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| self.t("Not selected yet", "Пока не выбрана").into()),
            )
    }
    fn pick(&mut self, selection: Selection, cx: &mut Context<Self>) {
        let folder = matches!(selection, Selection::Folder);
        let job = cx.prompt_for_paths(PathPromptOptions {
            files: !folder,
            directories: folder,
            multiple: false,
            prompt: Some(
                self.t(
                    if folder {
                        "Choose game folder"
                    } else {
                        "Choose JSON file"
                    },
                    if folder {
                        "Выбрать папку игры"
                    } else {
                        "Выбрать файл JSON"
                    },
                )
                .into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            let result = job
                .await
                .context("The file dialog could not return a selection")
                .and_then(|r| r);
            let paths = match result {
                Ok(Some(paths)) => paths,
                Ok(None) => return,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| this.fail(format!("{error:#}"), cx));
                    return;
                }
            };
            if let Some(path) = paths.into_iter().next() {
                let _ = this.update(cx, |this, cx| {
                    let result = (|| -> Result<()> {
                        if folder {
                            this.select_game_folder(path)?;
                        } else if matches!(selection, Selection::Channel) {
                            let channel: UpdateChannel = serde_json::from_slice(&fs::read(path)?)?;
                            github_feed(&channel.repository)?;
                            ensure!(
                                channel.public_key.len() == 64
                                    && channel.public_key.bytes().all(|b| b.is_ascii_hexdigit()),
                                "Invalid public update key"
                            );
                            this.prefs.update_channel = Some(channel);
                            this.prefs.use_local_release = false;
                            this.save()?;
                            this.load_release(cx, true);
                        } else {
                            let release: Release = serde_json::from_slice(&fs::read(&path)?)?;
                            release.validate()?;
                            this.prefs.recipe = Some(path);
                            this.prefs.use_local_release = true;
                            this.app_update = None;
                            this.release = Some(release);
                            this.save()?;
                            this.after_source()?;
                        }
                        Ok(())
                    })();
                    if let Err(e) = result {
                        this.fail(format!("{e:#}"), cx);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn load_release(&mut self, cx: &mut Context<Self>, explicit_check: bool) {
        self.feedback = None;
        self.app_update = None;
        let channel = self.prefs.channel();
        let local = self.prefs.use_local_release;
        let path = self.prefs.recipe.clone();
        let job = cx.background_executor().spawn(async move {
            if !local {
                std::thread::scope(|scope| {
                    let pack =
                        scope.spawn(|| fetch_release(&channel.repository, &channel.public_key));
                    let app = fetch_app_release(INSTALLER_REPOSITORY, INSTALLER_PUBLIC_KEY)?;
                    let pack = pack
                        .join()
                        .map_err(|_| anyhow::anyhow!("Pack update check failed"))??;
                    let app = if app.release.is_newer_than(env!("CARGO_PKG_VERSION"))? {
                        Some(app)
                    } else {
                        None
                    };
                    Ok::<_, anyhow::Error>((pack, app))
                })
            } else {
                let path = path.context("Choose a release.json file first")?;
                let r: Release = serde_json::from_slice(&fs::read(path)?)?;
                r.validate()?;
                Ok((r, None))
            }
        });
        self.step = Step::Busy;
        *self.progress.lock().unwrap() = self.t("Checking release…", "Проверяем выпуск…").into();
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((r, app)) => {
                        this.app_update = app;
                        if let Err(e) = this.finish_check(r, explicit_check) {
                            this.fail(format!("{e:#}"), cx);
                        }
                    }
                    Err(e) => this.fail(format!("{e:#}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn finish_check(&mut self, release: Release, explicit_check: bool) -> Result<()> {
        self.release = Some(release);
        self.after_source()?;
        if explicit_check && matches!(self.step, Step::Ready | Step::Access) {
            self.step = Step::CheckResult;
        }
        Ok(())
    }
    fn after_source(&mut self) -> Result<()> {
        if self.prefs.game.as_ref().is_some_and(|p| p.is_dir()) {
            self.make_plan()?;
            self.choose_review_step();
            if matches!(self.step, Step::Ready | Step::Access) {
                let version = &self.release.as_ref().unwrap().version;
                self.feedback = Some(if self.prefs.use_local_release {
                    format!(
                        "{} {version}. {}",
                        self.t("Local release installed:", "Локальный выпуск установлен:"),
                        self.t("Manual release mode", "Ручной выбор выпуска")
                    )
                } else {
                    format!(
                        "{} — Forever SMP {version}",
                        self.t("You're up to date", "Установлена последняя версия")
                    )
                });
            }
        } else {
            self.step = Step::Folder;
        }
        Ok(())
    }
    fn choose_review_step(&mut self) {
        let p = self.plan.as_ref().unwrap();
        let current = p
            .previous
            .as_ref()
            .is_some_and(|s| s.version == p.release.version && s.choices == p.choices)
            && p.install.is_empty()
            && p.remove.is_empty()
            && p.presets.is_empty()
            && p.conflicts.is_empty()
            && self.app_update.is_none();
        self.step = if current {
            if self.prefs.nickname.is_empty() {
                Step::Access
            } else {
                Step::Ready
            }
        } else {
            Step::Review
        };
    }
    fn review_title(&self, p: &Plan) -> String {
        if self.app_update.is_some() {
            return self.t("Update available", "Доступно обновление").into();
        }
        match &p.previous {
            Some(old) if old.version == p.release.version => self
                .t("Change optional mods", "Изменить необязательные моды")
                .into(),
            Some(old) => format!(
                "{}: {} → {}",
                self.t("Update available", "Доступно обновление"),
                old.version,
                p.release.version
            ),
            None => format!("Forever SMP {}", p.release.version),
        }
    }
    fn review_action(&self, p: &Plan) -> &'static str {
        if self.app_update.is_some() {
            return self.t("Update", "Обновить");
        }
        match &p.previous {
            Some(old) if old.version != p.release.version => {
                self.t("Install update", "Установить обновление")
            }
            Some(_) => self.t("Apply changes", "Применить изменения"),
            None => self.t("Install pack", "Установить сборку"),
        }
    }
    fn feature_row(
        &self,
        feature: &Feature,
        selector: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let enabled = *self.choices.get(&feature.id).unwrap_or(&false);
        let id = feature.id.clone();
        let label = format!(
            "{} {}",
            if enabled { "☑" } else { "☐" },
            if self.prefs.language == "ru" {
                &feature.ru
            } else {
                &feature.en
            }
        );
        div()
            .id(ElementId::Name(selector.clone().into()))
            .debug_selector(move || selector.clone())
            .w_full()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(if enabled { 0x54866c } else { 0x35463e }))
            .bg(rgb(if enabled { 0x243c2f } else { 0x24312c }))
            .hover(|style| style.bg(rgb(0x35463e)))
            .cursor_pointer()
            .child(div().flex_1().child(label))
            .child(div().flex_shrink_0().text_sm().child(self.t(
                if enabled { "On" } else { "Off" },
                if enabled { "Вкл." } else { "Выкл." },
            )))
            .on_click(cx.listener(move |s, _, _, cx| {
                let value = s.choices.entry(id.clone()).or_default();
                *value = !*value;
                s.recommended = false;
                if s.step == Step::Review {
                    match plan(
                        s.prefs.game.as_ref().unwrap(),
                        s.release.as_ref().unwrap(),
                        s.choices.clone(),
                        s.recommended,
                    ) {
                        Ok(p) => s.plan = Some(p),
                        Err(error) => s.fail(format!("{error:#}"), cx),
                    }
                }
                cx.notify();
            }))
    }
    fn advanced_controls(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("advanced-controls")
            .flex()
            .flex_col()
            .gap_2()
            .child(
                self.button(
                    "advanced",
                    self.t(
                        if self.advanced {
                            "Advanced ▴"
                        } else {
                            "Advanced ▾"
                        },
                        if self.advanced {
                            "Дополнительно ▴"
                        } else {
                            "Дополнительно ▾"
                        },
                    )
                    .into(),
                    self.advanced,
                    cx,
                    |s, _, cx| {
                        s.advanced = !s.advanced;
                        cx.notify();
                    },
                ),
            )
            .when(self.advanced, |body| {
                body.child(
                    self.button(
                        "release",
                        self.t(
                            "Use a local release file",
                            "Использовать локальный файл выпуска",
                        )
                        .into(),
                        false,
                        cx,
                        |s, _, cx| s.pick(Selection::Release, cx),
                    ),
                )
                .child(
                    self.button(
                        "channel",
                        self.t(
                            "Choose another update source",
                            "Выбрать другой источник обновлений",
                        )
                        .into(),
                        false,
                        cx,
                        |s, _, cx| s.pick(Selection::Channel, cx),
                    ),
                )
                .child(
                    self.button(
                        "official",
                        self.t("Use official updates", "Использовать официальный источник")
                            .into(),
                        false,
                        cx,
                        |s, _, cx| {
                            s.prefs.update_channel = None;
                            s.prefs.use_local_release = false;
                            match s.save() {
                                Ok(()) => s.load_release(cx, true),
                                Err(e) => s.fail(format!("{e:#}"), cx),
                            }
                        },
                    ),
                )
                .child(
                    self.button(
                        "prism",
                        self.t("Prism / MRPack instructions", "Инструкция Prism / MRPack")
                            .into(),
                        false,
                        cx,
                        |_, _, cx| {
                            cx.open_url(
                                "https://prismlauncher.org/wiki/getting-started/download-modpacks/",
                            )
                        },
                    ),
                )
            })
    }
    fn make_plan(&mut self) -> Result<()> {
        let release = self.release.as_ref().context("No release selected")?;
        let game = self.prefs.game.as_ref().context("Choose a game folder")?;
        if game.join(".forever-smp/pending").exists() {
            recover(game)?;
        }
        let previous = load_state(game, release)?;
        let _ = cleanup_game_backups(game);
        self.choices = release.choices(previous.as_ref());
        self.recommended = previous.as_ref().is_none_or(|s| s.recommended);
        self.plan = Some(plan(game, release, self.choices.clone(), self.recommended)?);
        self.replace = false;
        Ok(())
    }
    fn adopt_folder(&mut self, cx: &mut Context<Self>) {
        let Some(game) = self.prefs.game.clone() else {
            return;
        };
        let job = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                self.t(
                    "Choose the previous release.json",
                    "Выбрать предыдущий release.json",
                )
                .into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            let selection = job
                .await
                .context("The file dialog could not return a selection")
                .and_then(|r| r);
            let result = (|| -> Result<bool> {
                let Some(path) = selection?.and_then(|paths| paths.into_iter().next()) else {
                    return Ok(false);
                };
                let known: Release = serde_json::from_slice(&fs::read(path)?)?;
                adopt(&game, &known)?;
                Ok(true)
            })();
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(true) => {
                        if let Err(e) = this.make_plan() {
                            this.fail(format!("{e:#}"), cx);
                        }
                    }
                    Ok(false) => {}
                    Err(e) => this.fail(format!("{e:#}"), cx),
                };
                cx.notify();
            });
        })
        .detach();
    }
    fn update(&mut self, cx: &mut Context<Self>) {
        let Some(game) = self.prefs.game.clone() else {
            return;
        };
        let Some(plan) = self.plan.clone() else {
            return;
        };
        let replace = self.replace;
        if let Some(offer) = self.app_update.clone() {
            self.update_and_restart(game, plan, replace, offer, cx);
            return;
        }
        let progress = self.progress.clone();
        let resuming = self.prefs.resume_update.is_some();
        let job = cx.background_executor().spawn(async move {
            recover(&game)?;
            let current = if resuming {
                plan_for_resume(&game, &plan)?
            } else {
                plan
            };
            apply(&game, &current, replace, None, |p| {
                *progress.lock().unwrap() = p.into()
            })
        });
        self.step = Step::Busy;
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.prefs.resume_update = None;
                        if let Err(error) = this.save() {
                            this.fail(format!("{error:#}"), cx);
                            return;
                        }
                        this.feedback = Some(format!(
                            "{} — Forever SMP {}",
                            this.t("Changes applied", "Изменения применены"),
                            this.release.as_ref().unwrap().version
                        ));
                        this.step = if this.prefs.nickname.is_empty() {
                            Step::Access
                        } else {
                            Step::Ready
                        };
                    }
                    Err(e) => this.fail(format!("{e:#}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
        self.poll_progress(cx);
        cx.notify();
    }
    fn update_and_restart(
        &mut self,
        game: PathBuf,
        plan: Plan,
        replace: bool,
        offer: AppOffer,
        cx: &mut Context<Self>,
    ) {
        let progress = self.progress.clone();
        let russian = self.prefs.language == "ru";
        let expected_app_version = offer.release.version.clone();
        let job = cx.background_executor().spawn(async move {
            prepare_app_update(
                &std::env::current_exe()?,
                env!("CARGO_PKG_VERSION"),
                &offer,
                INSTALLER_PUBLIC_KEY,
                None,
                |stage| {
                    *progress.lock().unwrap() = match (stage, russian) {
                        ("download-app", false) => "Downloading update…",
                        ("download-app", true) => "Скачиваем обновление…",
                        (_, false) => "Preparing restart…",
                        (_, true) => "Подготавливаем перезапуск…",
                    }
                    .into();
                },
            )
        });
        self.step = Step::Busy;
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(path) => {
                    this.prefs.resume_update = Some(ResumeUpdate {
                        expected_app_version,
                        game,
                        plan,
                        replace,
                    });
                    let result = this.save().and_then(|_| launch_app_update(&path));
                    match result {
                        Ok(()) => cx.quit(),
                        Err(error) => {
                            this.prefs.resume_update = None;
                            let _ = this.save();
                            this.fail(format!("{error:#}"), cx);
                        }
                    }
                }
                Err(error) => this.fail(format!("{error:#}"), cx),
            });
        })
        .detach();
        self.poll_progress(cx);
        cx.notify();
    }
    fn resume_approved_update(&mut self, cx: &mut Context<Self>) {
        self.resume_approved_update_at(env!("CARGO_PKG_VERSION"), cx);
    }
    fn resume_approved_update_at(&mut self, running_version: &str, cx: &mut Context<Self>) {
        let Some(resume) = self.prefs.resume_update.clone() else {
            return;
        };
        let ready = semver::Version::parse(running_version).and_then(|version| {
            Ok(version >= semver::Version::parse(&resume.expected_app_version)?)
        });
        if !matches!(ready, Ok(true)) {
            self.prefs.resume_update = None;
            let _ = self.save();
            self.fail(
                self.t(
                    "The app update did not finish. Your pack has not changed.",
                    "Обновление приложения не завершилось. Сборка не изменена.",
                )
                .to_owned(),
                cx,
            );
            return;
        }
        self.prefs.game = Some(resume.game);
        self.release = Some(resume.plan.release.clone());
        self.choices = resume.plan.choices.clone();
        self.recommended = resume.plan.recommended;
        self.plan = Some(resume.plan);
        self.replace = resume.replace;
        self.app_update = None;
        *self.progress.lock().unwrap() =
            self.t("Finishing update…", "Завершаем обновление…").into();
        self.update(cx);
    }
    fn poll_progress(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(std::time::Duration::from_millis(200)).await;
                match this.update(cx, |this, cx| {
                    cx.notify();
                    this.step == Step::Busy
                }) {
                    Ok(true) => {}
                    _ => break,
                }
            }
        })
        .detach();
    }
    fn button(
        &self,
        id: &'static str,
        label: String,
        primary: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .px_5()
            .py_3()
            .rounded_lg()
            .cursor_pointer()
            .bg(rgb(if primary { 0x80d4b0 } else { 0x24312c }))
            .text_color(rgb(if primary { 0x102019 } else { 0xdae9e1 }))
            .hover(|s| s.bg(rgb(if primary { 0x9ae5c4 } else { 0x35463e })))
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
    }
    fn restore(&mut self, recovery: bool, cx: &mut Context<Self>) {
        let Some(game) = self.prefs.game.clone() else {
            return;
        };
        let job = cx.background_executor().spawn(async move {
            if recovery {
                recover(&game).map(|_| ())
            } else {
                restore_last(&game)
            }
        });
        self.step = Step::Busy;
        *self.progress.lock().unwrap() = "Restoring / Восстановление…".into();
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.step = Step::Source;
                    }
                    Err(e) => this.fail(format!("{e:#}"), cx),
                };
                cx.notify();
            });
        })
        .detach();
    }
}

fn nickname_caret(focused: bool) -> Div {
    div()
        .w(px(2.))
        .h(px(20.))
        .flex_shrink_0()
        .bg(rgb(0x80d4b0))
        .when(!focused, |caret| caret.invisible())
}

impl Render for Wizard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nickname_focused = self.focus.is_focused(window);
        let mut body = div().flex().flex_col().gap_4();
        let title: String;
        match self.step {
            Step::Language => {
                title = "Добро пожаловать / Welcome".into();
                body = body
                    .child("Forever SMP")
                    .child(self.button("ru", "Русский".into(), true, cx, |s, _, cx| {
                        s.prefs.language = "ru".into();
                        if let Err(e) = s.save() {
                            s.fail(e, cx);
                            return;
                        }
                        s.load_release(cx, false);
                    }))
                    .child(self.button("en", "English".into(), false, cx, |s, _, cx| {
                        s.prefs.language = "en".into();
                        if let Err(e) = s.save() {
                            s.fail(e, cx);
                            return;
                        }
                        s.load_release(cx, false);
                    }));
            }
            Step::Source => {
                title = self.t("Your pack", "Ваша сборка").into();
                body = body.child(self.t(
                    "Continue to set up Forever SMP. Updates are configured already.",
                    "Продолжите установку Forever SMP. Источник обновлений уже настроен.",
                ));
                if let Some(feedback) = &self.feedback {
                    body = body.child(feedback.clone());
                }
                body = body.child(self.button(
                    "continue",
                    self.t("Continue", "Продолжить").into(),
                    true,
                    cx,
                    |s, _, cx| s.load_release(cx, false),
                ));
                body = body.child(self.advanced_controls(cx));
            }
            Step::Folder => {
                title = self
                    .t("Choose the game folder", "Выберите папку игры")
                    .into();
                let r = self.release.as_ref().unwrap();
                body = body.child(format!("Minecraft {} · Fabric {} · Java {}+", r.minecraft, r.fabric, r.java))
                    .child(self.t("Set these up in your launcher first. Use a separate game folder. Close Minecraft before continuing.", "Сначала настройте их в лаунчере. Используйте отдельную папку игры. Закройте Minecraft."))
                    .child(self.selected_folder())
                    .child(self.button("folder", self.t("Choose folder", "Выбрать папку").into(), true, cx, |s, _, cx| s.pick(Selection::Folder, cx)));
                if self.prefs.game.as_ref().is_some_and(|p| p.is_dir()) {
                    body = body.child(self.button(
                        "use-folder",
                        self.t("Use this folder", "Использовать эту папку").into(),
                        false,
                        cx,
                        |s, _, cx| match s.select_game_folder(s.prefs.game.clone().unwrap()) {
                            Ok(()) => cx.notify(),
                            Err(error) => s.fail(format!("{error:#}"), cx),
                        },
                    ));
                }
                body = body.child(self.button(
                    "folder-back",
                    self.t("Back", "Назад").into(),
                    false,
                    cx,
                    |s, _, cx| {
                        s.step = s.folder_return.take().unwrap_or(Step::Language);
                        cx.notify();
                    },
                ));
            }
            Step::Review => {
                let p = self.plan.as_ref().unwrap();
                title = self.review_title(p);
                body = body.child(self.selected_folder());
                let same_version = p
                    .previous
                    .as_ref()
                    .is_some_and(|s| s.version == p.release.version);
                if let Some(app) = &self.app_update {
                    body = body
                        .child(format!(
                            "{}: {} → {}",
                            self.t("App", "Приложение"),
                            env!("CARGO_PKG_VERSION"),
                            app.release.version
                        ))
                        .child(if self.prefs.language == "ru" {
                            app.release.notes_ru.clone()
                        } else {
                            app.release.notes_en.clone()
                        });
                }
                let pack_changes = !same_version
                    || !p.install.is_empty()
                    || !p.remove.is_empty()
                    || !p.presets.is_empty()
                    || !p.conflicts.is_empty();
                if pack_changes {
                    body = body.child(format!("Forever SMP {}", p.release.version));
                    body = body
                        .child(if same_version {
                            self.t(
                                "Review the changes to your current pack.",
                                "Проверьте изменения в текущей сборке.",
                            )
                            .into()
                        } else if self.prefs.language == "ru" {
                            p.release.notes_ru.clone()
                        } else {
                            p.release.notes_en.clone()
                        })
                        .child(format!(
                            "{}: {} · {}: {} · {:.1} MB",
                            self.t("Download", "Скачать"),
                            p.install.len(),
                            self.t("Remove", "Убрать"),
                            p.remove.len(),
                            p.install.iter().map(|m| m.size).sum::<u64>() as f64 / 1048576.0
                        ));
                }
                if self.app_update.is_some() {
                    body = body.child(self.t(
                        "One update. The app restarts and finishes automatically.",
                        "Приложение перезапустится и завершит обновление автоматически.",
                    ));
                }
                if let Some(previous) = &p.previous {
                    let additions: Vec<_> = p
                        .release
                        .features
                        .iter()
                        .filter(|feature| !previous.choices.contains_key(&feature.id))
                        .collect();
                    if !additions.is_empty() {
                        body = body.child(self.t("New optional mods", "Новые необязательные моды"))
                            .child(self.t("Recommended choices are selected. Turn off anything you don't want before updating.",
                                "Рекомендуемые варианты выбраны. Отключите ненужные перед обновлением."));
                        for feature in additions {
                            body = body.child(self.feature_row(
                                feature,
                                format!("new-feature-{}", feature.id),
                                cx,
                            ));
                        }
                    }
                }
                if !p.conflicts.is_empty() {
                    body = body
                        .child(format!(
                            "{}: {}",
                            self.t("Customised settings", "Изменённые вами настройки"),
                            p.conflicts.join(", ")
                        ))
                        .child(
                            self.button(
                                "configs",
                                self.t(
                                    if self.replace {
                                        "Use new presets (back up mine)"
                                    } else {
                                        "Keep my settings ✓"
                                    },
                                    if self.replace {
                                        "Применить новые настройки (с копией старых)"
                                    } else {
                                        "Сохранить мои настройки ✓"
                                    },
                                )
                                .into(),
                                false,
                                cx,
                                |s, _, cx| {
                                    s.replace = !s.replace;
                                    cx.notify();
                                },
                            ),
                        );
                }
                if !p.extras.is_empty() {
                    body = body.child(format!(
                        "{}: {}",
                        self.t(
                            "Extra mods kept; outside the tested pack",
                            "Ваши моды сохранены; совместимость не проверена"
                        ),
                        p.extras.len()
                    ));
                }
                body = body
                    .child(self.button(
                        "apply",
                        self.review_action(p).into(),
                        true,
                        cx,
                        |s, _, cx| s.update(cx),
                    ))
                    .child(self.button(
                        "customise",
                        self.t("Customise mods", "Настроить моды").into(),
                        false,
                        cx,
                        |s, _, cx| {
                            s.step = Step::Options;
                            cx.notify();
                        },
                    ))
                    .child(self.button(
                        "change-folder",
                        self.t("Change game folder", "Изменить папку игры").into(),
                        false,
                        cx,
                        |s, _, cx| s.open_folder(cx),
                    ));
                if p.previous.is_none() && !p.extras.is_empty() {
                    body = body.child(
                        self.button(
                            "adopt",
                            self.t(
                                "Adopt an older Prism pack (choose its release.json)",
                                "Подключить старую сборку Prism (выберите её release.json)",
                            )
                            .into(),
                            false,
                            cx,
                            |s, _, cx| s.adopt_folder(cx),
                        ),
                    );
                }
            }
            Step::Options => {
                title = self.t("Your optional mods", "Необязательные моды").into();
                body = body.gap_2().child(self.t(
                    "Click a mod to enable or disable it. Review changes before applying.",
                    "Нажмите на мод, чтобы включить или отключить его. Затем проверьте изменения.",
                ));
                for (i, feature) in self.release.as_ref().unwrap().features.iter().enumerate() {
                    body = body.child(self.feature_row(feature, format!("feature-{i}"), cx));
                }
                body = body
                    .child(
                        self.button(
                            "recommended",
                            self.t("Reset to recommended", "Рекомендуемый состав")
                                .into(),
                            false,
                            cx,
                            |s, _, cx| {
                                s.choices = s.release.as_ref().unwrap().choices(None);
                                s.recommended = true;
                                cx.notify();
                            },
                        ),
                    )
                    .child(self.button(
                        "options-done",
                        self.t("Review changes", "Проверить изменения").into(),
                        true,
                        cx,
                        |s, _, cx| {
                            match plan(
                                s.prefs.game.as_ref().unwrap(),
                                s.release.as_ref().unwrap(),
                                s.choices.clone(),
                                s.recommended,
                            ) {
                                Ok(p) => {
                                    s.plan = Some(p);
                                    s.choose_review_step();
                                }
                                Err(e) => s.fail(format!("{e:#}"), cx),
                            };
                            cx.notify();
                        },
                    ));
            }
            Step::CheckResult => {
                title = self
                    .t(
                        if self.prefs.use_local_release {
                            "Local release checked"
                        } else {
                            "No updates available"
                        },
                        if self.prefs.use_local_release {
                            "Локальный выпуск проверен"
                        } else {
                            "Обновлений нет"
                        },
                    )
                    .into();
                body = body
                    .child(self.feedback.clone().unwrap_or_default())
                    .child(self.button(
                        "check-done",
                        self.t("Continue", "Продолжить").into(),
                        true,
                        cx,
                        |s, _, cx| {
                            s.step = if s.prefs.nickname.is_empty() {
                                Step::Access
                            } else {
                                Step::Ready
                            };
                            cx.notify();
                        },
                    ));
            }
            Step::Busy => {
                title = self.t("Working…", "Подготавливаем…").into();
                body = body
                    .child(self.progress.lock().unwrap().clone())
                    .child(self.t("Keep this window open.", "Оставьте это окно открытым."));
            }
            Step::Access => {
                title = self.t("Request access", "Заявка на доступ").into();
                body = body.child(self.t("For your first join, get owner approval, the server address and registration password privately. Choose your own 12+ character login password; never send it to anyone. Disable TLauncher's skin/cape override.",
                    "Для первого входа получите одобрение владельца, адрес сервера и пароль регистрации лично. Придумайте свой пароль от 12 символов и никому его не отправляйте. Отключите замену скинов и плащей TLauncher."));
                if let Some(feedback) = &self.feedback {
                    body = body.child(div().text_color(rgb(0x80d4b0)).child(feedback.clone()));
                }
                body = body
                    .child(self.t(
                        "Enter your exact Minecraft nickname. Send the request to the owner.",
                        "Введите точный ник Minecraft. Отправьте заявку владельцу.",
                    ))
                    .child(
                        div()
                            .id("nickname")
                            .debug_selector(|| "nickname".into())
                            .track_focus(&self.focus)
                            .flex()
                            .items_center()
                            .gap_1()
                            .p_3()
                            .rounded_md()
                            .border_2()
                            .border_color(rgb(0x456052))
                            .bg(rgb(0x25352d))
                            .hover(|s| {
                                if nickname_focused {
                                    s
                                } else {
                                    s.bg(rgb(0x2c4035)).border_color(rgb(0x679c80))
                                }
                            })
                            .focus(|s| s.bg(rgb(0x1c3026)).border_color(rgb(0x80d4b0)))
                            .cursor_text()
                            .when(self.prefs.nickname.is_empty(), |field| {
                                field.child(nickname_caret(nickname_focused))
                            })
                            .child(
                                div()
                                    .when(self.prefs.nickname.is_empty(), |text| {
                                        text.text_color(rgb(0xa4baad))
                                    })
                                    .child(if self.prefs.nickname.is_empty() {
                                        self.t("Click and type nickname", "Нажмите и введите ник")
                                            .to_owned()
                                    } else {
                                        self.prefs.nickname.clone()
                                    }),
                            )
                            .when(!self.prefs.nickname.is_empty(), |field| {
                                field.child(nickname_caret(nickname_focused))
                            })
                            .on_mouse_down_out(cx.listener(|s, _, w, _| {
                                if s.focus.is_focused(w) {
                                    w.blur();
                                }
                            }))
                            .on_click(cx.listener(|s, _, w, _| s.focus.focus(w)))
                            .on_key_down(cx.listener(|s, event: &KeyDownEvent, _, cx| {
                                if event.keystroke.key == "backspace" {
                                    s.prefs.nickname.pop();
                                } else if event.keystroke.key == "v"
                                    && (event.keystroke.modifiers.control
                                        || event.keystroke.modifiers.platform)
                                {
                                    if let Some(text) =
                                        cx.read_from_clipboard().and_then(|v| v.text())
                                        && access_request(text.trim()).is_ok()
                                    {
                                        s.prefs.nickname = text.trim().into();
                                    }
                                } else if let Some(text) = &event.keystroke.key_char
                                    && s.prefs.nickname.len() + text.len() <= 16
                                    && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                                {
                                    s.prefs.nickname.push_str(text);
                                }
                                s.request_copied = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        self.button(
                            "copy",
                            self.t(
                                if self.request_copied {
                                    "Copied ✓"
                                } else {
                                    "Copy access request"
                                },
                                if self.request_copied {
                                    "Скопировано ✓"
                                } else {
                                    "Скопировать заявку"
                                },
                            )
                            .into(),
                            true,
                            cx,
                            |s, _, cx| {
                                match access_request(&s.prefs.nickname) {
                                    Ok(text) => {
                                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                                        s.request_copied = true;
                                        if let Err(e) = s.save() {
                                            s.fail(e, cx);
                                        }
                                    }
                                    Err(e) => s.fail(e, cx),
                                }
                                cx.notify();
                            },
                        ),
                    )
                    .child(self.button(
                        "access-done",
                        self.t("Continue", "Продолжить").into(),
                        false,
                        cx,
                        |s, _, cx| {
                            if let Err(e) = s.save() {
                                s.fail(e, cx);
                                return;
                            }
                            s.step = Step::Ready;
                            cx.notify();
                        },
                    ));
            }
            Step::Ready => {
                title = self.t("Ready", "Готово").into();
                if let Some(release) = &self.release {
                    body = body.child(format!("Forever SMP {}", release.version));
                }
                body = body.child(self.selected_folder());
                if let Some(feedback) = &self.feedback {
                    body = body.child(div().text_color(rgb(0x80d4b0)).child(feedback.clone()));
                }
                body = body
                    .child(self.t(
                        "Launch this game folder in your Minecraft launcher.",
                        "Запустите эту папку игры в своём лаунчере Minecraft.",
                    ))
                    .child(
                        self.button(
                            "check",
                            if self.prefs.use_local_release {
                                self.t("Check local files", "Проверить файлы сборки")
                            } else {
                                self.t("Check for updates", "Проверить обновления")
                            }
                            .into(),
                            false,
                            cx,
                            |s, _, cx| s.load_release(cx, true),
                        ),
                    )
                    .child(
                        self.button(
                            "ready-options",
                            self.t("Change optional mods", "Изменить необязательные моды")
                                .into(),
                            false,
                            cx,
                            |s, _, cx| {
                                match s.make_plan() {
                                    Ok(()) => s.step = Step::Options,
                                    Err(e) => s.fail(format!("{e:#}"), cx),
                                }
                                cx.notify();
                            },
                        ),
                    )
                    .child(self.button(
                        "request-again",
                        self.t("Access request", "Заявка на доступ").into(),
                        false,
                        cx,
                        |s, _, cx| {
                            s.step = Step::Access;
                            cx.notify();
                        },
                    ))
                    .child(self.button(
                        "ready-folder",
                        self.t("Change game folder", "Изменить папку игры").into(),
                        false,
                        cx,
                        |s, _, cx| s.open_folder(cx),
                    ))
                    .child(self.advanced_controls(cx));
            }
            Step::Error => {
                title = self.t("Couldn't finish", "Не удалось завершить").into();
                body = body
                    .child(self.error.clone())
                    .child(self.button(
                        "retry",
                        self.t("Try again", "Попробовать снова").into(),
                        true,
                        cx,
                        |s, _, cx| {
                            s.step = Step::Source;
                            cx.notify();
                        },
                    ))
                    .child(
                        self.button(
                            "recover",
                            self.t(
                                "Recover interrupted update",
                                "Восстановить прерванное обновление",
                            )
                            .into(),
                            false,
                            cx,
                            |s, _, cx| s.restore(true, cx),
                        ),
                    )
                    .child(
                        self.button(
                            "restore",
                            self.t("Restore previous pack", "Вернуть предыдущую сборку")
                                .into(),
                            false,
                            cx,
                            |s, _, cx| s.restore(false, cx),
                        ),
                    )
                    .child(self.button(
                        "copy-error",
                        self.t("Copy error", "Скопировать ошибку").into(),
                        false,
                        cx,
                        |s, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(s.error.clone()))
                        },
                    ));
                if self.release.is_some() {
                    body = body.child(
                        self.button(
                            "error-folder",
                            self.t("Choose another game folder", "Выбрать другую папку игры")
                                .into(),
                            false,
                            cx,
                            |s, _, cx| {
                                s.step = Step::Folder;
                                cx.notify();
                            },
                        ),
                    );
                }
            }
        }
        div()
            .size_full()
            .bg(rgb(0x142019))
            .text_color(rgb(0xdae9e1))
            .font_family("Segoe UI")
            .p_8()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x80d4b0))
                    .child("FOREVER SMP"),
            )
            .child(div().text_2xl().child(title))
            .child(
                div()
                    .id(ElementId::Name(format!("content-{:?}", self.step).into()))
                    .flex_1()
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let flag = args.first().and_then(|s| s.to_str());
    let job = args.get(1).map(PathBuf::from);
    if flag == Some("--self-update-helper") {
        if let Some(job) = job
            && let Err(error) = run_app_update_helper(&job, INSTALLER_PUBLIC_KEY)
        {
            eprintln!("{error:#}");
        }
        return;
    }
    let finished = if flag == Some("--finish-app-update") {
        job.clone()
    } else {
        None
    };
    let failure = if flag == Some("--app-update-failed") {
        job.as_ref().map(|path| {
            app_update_failure(path, INSTALLER_PUBLIC_KEY)
                .unwrap_or_else(|error| format!("{error:#}"))
        })
    } else {
        None
    };
    // Helpers may still hold their EXE/lock briefly after acknowledging the
    // new window. Retry off the UI thread; cleanup never blocks installation.
    if let Ok(executable) = std::env::current_exe() {
        std::thread::spawn(move || {
            for seconds in [1, 2, 5] {
                std::thread::sleep(std::time::Duration::from_secs(seconds));
                let _ = cleanup_installer_updates(&executable, INSTALLER_PUBLIC_KEY);
            }
        });
    }
    Application::new().run(move |cx| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let window = cx
            .open_window(
                WindowOptions {
                    display_id: cx.primary_display().map(|display| display.id()),
                    window_bounds: Some(WindowBounds::Windowed(cx.primary_display()
                        .map(|display| fitted_window_bounds(display.bounds()))
                        .unwrap_or_else(|| Bounds::centered(None, size(px(660.), px(680.)), cx)))),
                    titlebar: Some(TitlebarOptions {
                        title: Some(format!("Forever SMP · {}", env!("CARGO_PKG_VERSION")).into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let mut wizard = Wizard::new(cx);
                        #[cfg(debug_assertions)]
                        if let Ok(screen) = std::env::var("FOREVER_SMP_PREVIEW") {
                            if let Some(path) = &wizard.prefs.recipe {
                                wizard.release = fs::read(path)
                                    .ok()
                                    .and_then(|b| serde_json::from_slice(&b).ok());
                            }
                            if wizard.release.is_some() && wizard.prefs.game.is_some() {
                                let _ = wizard.make_plan();
                            }
                            wizard.step = match screen.as_str() {
                                "options" if wizard.release.is_some() => Step::Options,
                                "review" if wizard.plan.is_some() => Step::Review,
                                "access" => Step::Access,
                                "ready" => Step::Ready,
                                _ => Step::Language,
                            };
                            return wizard;
                        }
                        if let Some(error) = &failure {
                            wizard.prefs.resume_update = None;
                            let _ = wizard.save();
                            wizard.fail(format!("{}\n{error}", wizard.t("Update failed. The previous app was restored.", "Обновление не удалось. Предыдущее приложение восстановлено.")), cx);
                        } else if wizard.prefs.resume_update.is_some() {
                            // A helper restart must acknowledge its window
                            // before touching the Minecraft folder.
                            if finished.is_none() {
                                wizard.resume_approved_update(cx);
                            } else {
                                wizard.step = Step::Busy;
                            }
                        } else if !wizard.prefs.language.is_empty() {
                            wizard.load_release(cx, false);
                        }
                        wizard
                    })
                },
            )
            .expect("Open Forever SMP window");
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, _| window.activate_window());
        });
        if let Some(job) = finished {
            cx.defer(move |cx| {
                let acknowledged = acknowledge_app_update(&job, INSTALLER_PUBLIC_KEY, env!("CARGO_PKG_VERSION"));
                let _ = window.update(cx, |wizard, _, cx| match acknowledged {
                    Ok(()) => wizard.resume_approved_update(cx),
                    Err(error) => wizard.fail(format!("{error:#}"), cx),
                });
            });
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("FOREVER_SMP_DIALOG_SMOKE").is_some() {
            cx.defer(move |cx| {
                let _ = window.update(cx, |wizard, _, cx| wizard.pick(Selection::Folder, cx));
            });
        }
        #[cfg(not(debug_assertions))]
        let _ = window;
    });
}

#[cfg(test)]
mod tests {
    use super::{Preferences, ResumeUpdate, Step, Wizard, fitted_window_bounds, preference_path};
    use forever_core::*;
    use gpui::{Modifiers, TestAppContext, px, size};
    use std::{collections::BTreeMap, fs, sync::Arc};

    #[test]
    fn initial_window_fits_small_and_offset_displays() {
        for display in [
            gpui::Bounds::new(gpui::point(px(0.), px(0.)), size(px(640.), px(480.))),
            gpui::Bounds::new(
                gpui::point(px(-1920.), px(120.)),
                size(px(1920.), px(1080.)),
            ),
            gpui::Bounds::new(gpui::point(px(1920.), px(-900.)), size(px(800.), px(450.))),
        ] {
            let bounds = fitted_window_bounds(display);
            assert!(bounds.origin.x >= display.origin.x && bounds.origin.y >= display.origin.y);
            assert!(bounds.origin.x + bounds.size.width <= display.origin.x + display.size.width);
            assert!(bounds.origin.y + bounds.size.height <= display.origin.y + display.size.height);
            assert_eq!(bounds.center(), display.center());
        }
    }

    #[test]
    fn official_updates_are_ready_by_default() {
        let prefs = Preferences::default();
        assert!(!prefs.use_local_release);
        let channel = prefs.channel();
        assert_eq!(channel.repository, "rzxx/forever-smp-releases");
        assert_eq!(channel.public_key.len(), 64);
        let legacy: Preferences = serde_json::from_str(r#"{"language":"ru","game":null,"recipe":null,"nickname":"TestPlayer","invitation":{"server":"legacy.example"}}"#).unwrap();
        assert_eq!(legacy.nickname, "TestPlayer");
        assert_eq!(legacy.channel().repository, channel.repository);
    }

    #[gpui::test]
    fn nickname_field_focuses_types_and_blurs(cx: &mut TestAppContext) {
        for language in ["en", "ru"] {
            let (view, visual) = cx.add_window_view(|_, cx| {
                let mut wizard = Wizard::new(cx);
                wizard.prefs.language = language.into();
                wizard.prefs.nickname.clear();
                wizard.step = Step::Access;
                wizard
            });
            visual.simulate_resize(size(px(660.), px(680.)));
            visual.run_until_parked();
            let bounds = visual.debug_bounds("nickname").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            visual.update(|window, cx| {
                assert!(view.read(cx).focus.is_focused(window));
            });
            visual.simulate_keystrokes("T e s t _ 1 backspace 2");
            assert_eq!(
                view.read_with(visual, |wizard, _| wizard.prefs.nickname.clone()),
                "Test_2"
            );
            let copy = visual.debug_bounds("copy").unwrap();
            visual.simulate_click(copy.center(), Modifiers::none());
            visual.update(|window, cx| {
                assert!(!view.read(cx).focus.is_focused(window));
                assert!(view.read(cx).request_copied);
            });
            visual.simulate_keystrokes("x");
            assert_eq!(
                view.read_with(visual, |wizard, _| wizard.prefs.nickname.clone()),
                "Test_2"
            );
            let bounds = visual.debug_bounds("nickname").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            visual.simulate_keystrokes("3");
            assert_eq!(
                view.read_with(visual, |wizard, _| wizard.prefs.nickname.clone()),
                "Test_23"
            );
        }
    }

    #[gpui::test]
    fn existing_install_can_toggle_optional_mods(cx: &mut TestAppContext) {
        for language in ["en", "ru"] {
            let game = tempfile::tempdir().unwrap();
            let cache = tempfile::tempdir().unwrap();
            let name = "CameraOverhaul-v2.1.2-fabric+mc[26.3-plus].jar";
            fs::write(cache.path().join(name), b"camera").unwrap();
            let release = Release {
                schema: 1,
                pack_id: PACK_ID.into(),
                version: "0.1.12".into(),
                minecraft: "26.3".into(),
                fabric: "0.19.5".into(),
                java: 25,
                notes_en: "Test".into(),
                notes_ru: "Проверка".into(),
                features: vec![Feature {
                    id: "camera-overhaul".into(),
                    en: "Camera movements".into(),
                    ru: "Движения камеры".into(),
                    default: true,
                    requires: vec![],
                }],
                mods: vec![ModFile {
                    id: "camera".into(),
                    path: format!("mods/{name}"),
                    sha512: sha512(b"camera"),
                    size: 6,
                    urls: vec!["https://example.invalid/camera.jar".into()],
                    feature: Some("camera-overhaul".into()),
                }],
                presets: vec![],
            };
            let initial = plan(game.path(), &release, release.choices(None), true).unwrap();
            apply(game.path(), &initial, false, Some(cache.path()), |_| {}).unwrap();
            let (view, visual) = cx.add_window_view(|_, cx| {
                let mut wizard = Wizard {
                    prefs: Preferences {
                        language: language.into(),
                        game: Some(game.path().into()),
                        nickname: "TestPlayer".into(),
                        use_local_release: true,
                        ..Default::default()
                    },
                    step: Step::Ready,
                    release: Some(release.clone()),
                    plan: None,
                    choices: BTreeMap::new(),
                    recommended: true,
                    replace: false,
                    error: String::new(),
                    progress: Arc::default(),
                    focus: cx.focus_handle(),
                    request_copied: false,
                    feedback: None,
                    advanced: false,
                    app_update: None,
                    folder_return: None,
                };
                wizard.make_plan().unwrap();
                wizard
            });
            visual.simulate_resize(size(px(660.), px(680.)));
            visual.run_until_parked();
            assert!(visual.debug_bounds("selected-folder").is_some());
            let bounds = visual.debug_bounds("ready-folder").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::Folder));
            let bounds = visual.debug_bounds("folder-back").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(
                view.read_with(visual, |wizard, _| wizard.step == Step::Ready
                    && wizard.prefs.game.as_deref() == Some(game.path()))
            );
            let bounds = visual.debug_bounds("advanced").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(view.read_with(visual, |wizard, _| wizard.advanced));
            let bounds = visual.debug_bounds("advanced").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(view.read_with(visual, |wizard, _| !wizard.advanced));
            // Switching between two managed folders reloads their individual
            // choices without touching either installation or showing reapply.
            let other = tempfile::tempdir().unwrap();
            let other_plan = plan(
                other.path(),
                &release,
                BTreeMap::from([("camera-overhaul".into(), false)]),
                false,
            )
            .unwrap();
            apply(other.path(), &other_plan, false, Some(cache.path()), |_| {}).unwrap();
            let original_receipt = fs::read(game.path().join(".forever-smp/state.json")).unwrap();
            view.update(visual, |wizard, cx| {
                wizard.select_game_folder(other.path().into()).unwrap();
                assert!(wizard.step == Step::Ready);
                assert!(!wizard.choices["camera-overhaul"]);
                wizard.select_game_folder(game.path().into()).unwrap();
                assert!(wizard.step == Step::Ready);
                assert!(wizard.choices["camera-overhaul"]);
                cx.notify();
            });
            assert_eq!(
                fs::read(game.path().join(".forever-smp/state.json")).unwrap(),
                original_receipt
            );
            let bounds = visual.debug_bounds("ready-options").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::Options));
            let saved_state = fs::read(game.path().join(".forever-smp/state.json")).unwrap();
            let bounds = visual.debug_bounds("options-done").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::Ready));
            assert_eq!(
                fs::read(game.path().join(".forever-smp/state.json")).unwrap(),
                saved_state
            );
            let bounds = visual.debug_bounds("ready-options").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            // Turning a choice off and back on also requires no reapply.
            for _ in 0..2 {
                let bounds = visual.debug_bounds("feature-0").unwrap();
                visual.simulate_click(bounds.center(), Modifiers::none());
            }
            let bounds = visual.debug_bounds("options-done").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::Ready));
            let bounds = visual.debug_bounds("ready-options").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            let bounds = visual.debug_bounds("feature-0").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(!view.read_with(visual, |wizard, _| wizard.choices["camera-overhaul"]));
            let bounds = visual.debug_bounds("options-done").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::Review));
            let removal = view.read_with(visual, |wizard, _| wizard.plan.clone().unwrap());
            assert_eq!(removal.remove.len(), 1);
            let title = view.read_with(visual, |wizard, _| wizard.review_title(&removal));
            assert_eq!(
                title,
                if language == "ru" {
                    "Изменить необязательные моды"
                } else {
                    "Change optional mods"
                }
            );
            apply(game.path(), &removal, false, Some(cache.path()), |_| {}).unwrap();
            view.update(visual, |wizard, cx| {
                wizard.make_plan().unwrap();
                wizard.step = Step::Ready;
                cx.notify();
            });
            visual.run_until_parked();
            let bounds = visual.debug_bounds("ready-options").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            let bounds = visual.debug_bounds("feature-0").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(view.read_with(visual, |wizard, _| wizard.choices["camera-overhaul"]));
            let bounds = visual.debug_bounds("options-done").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            let installation = view.read_with(visual, |wizard, _| wizard.plan.clone().unwrap());
            assert_eq!(installation.install.len(), 1);
            apply(
                game.path(),
                &installation,
                false,
                Some(cache.path()),
                |_| {},
            )
            .unwrap();
            assert!(game.path().join(&release.mods[0].path).exists());
            let recipe_path = game.path().join("release.json");
            fs::write(&recipe_path, serde_json::to_vec(&release).unwrap()).unwrap();
            view.update(visual, |wizard, cx| {
                wizard.prefs.recipe = Some(recipe_path.clone());
                wizard.after_source().unwrap();
                cx.notify();
            });
            visual.run_until_parked();
            let bounds = visual.debug_bounds("check").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            visual.run_until_parked();
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::CheckResult));
            let feedback = view.read_with(visual, |wizard, _| wizard.feedback.clone().unwrap());
            assert!(feedback.contains(if language == "ru" {
                "Локальный выпуск установлен"
            } else {
                "Local release installed"
            }));
            // A successfully fetched online release uses the same result handler.
            view.update(visual, |wizard, _| {
                wizard.prefs.use_local_release = false;
                wizard.finish_check(release.clone(), true).unwrap();
            });
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::CheckResult));
            let feedback = view.read_with(visual, |wizard, _| wizard.feedback.clone().unwrap());
            assert!(feedback.contains(if language == "ru" {
                "Установлена последняя версия"
            } else {
                "You're up to date"
            }));
            visual.run_until_parked();
            let bounds = visual.debug_bounds("check-done").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::Ready));

            let mut next = release.clone();
            next.version = "0.1.13".into();
            fs::write(&recipe_path, serde_json::to_vec(&next).unwrap()).unwrap();
            view.update(visual, |wizard, cx| {
                wizard.prefs.use_local_release = true;
                cx.notify();
            });
            visual.run_until_parked();
            let bounds = visual.debug_bounds("check").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            visual.run_until_parked();
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::Review));
            let pending = view.read_with(visual, |wizard, _| wizard.plan.clone().unwrap());
            let title = view.read_with(visual, |wizard, _| wizard.review_title(&pending));
            assert!(title.contains(if language == "ru" {
                "Доступно обновление"
            } else {
                "Update available"
            }));
            assert!(title.contains("0.1.12 → 0.1.13"));
            assert_eq!(
                view.read_with(visual, |wizard, _| wizard.review_action(&pending)),
                if language == "ru" {
                    "Установить обновление"
                } else {
                    "Install update"
                }
            );
            assert_eq!(
                load_state(game.path(), &next).unwrap().unwrap().version,
                "0.1.12"
            );
            let bounds = visual.debug_bounds("apply").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            visual.run_until_parked();
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::Ready));
            assert_eq!(
                load_state(game.path(), &next).unwrap().unwrap().version,
                "0.1.13"
            );
            let feedback = view.read_with(visual, |wizard, _| wizard.feedback.clone().unwrap());
            assert!(feedback.contains("0.1.13"));

            // Both updates have one review. After restart the saved approval
            // resumes directly, retaining personal files and profile choices.
            let mut combined = next.clone();
            combined.version = "0.1.14".into();
            fs::create_dir_all(game.path().join("config")).unwrap();
            fs::write(game.path().join("config/personal.txt"), b"my settings").unwrap();
            view.update(visual, |wizard, cx| {
                wizard.app_update = Some(AppOffer {
                    release: AppRelease {
                        schema: 1,
                        app_id: INSTALLER_ID.into(),
                        version: "0.1.8".into(),
                        notes_en: "App fix".into(),
                        notes_ru: "Исправление приложения".into(),
                        assets: vec![],
                    },
                    envelope: vec![], // Rendering fixture; never used for staging.
                });
                wizard.finish_check(combined.clone(), true).unwrap();
                cx.notify();
            });
            visual.run_until_parked();
            assert!(visual.debug_bounds("apply").is_some());
            let approved = view.read_with(visual, |wizard, _| {
                assert!(wizard.step == Step::Review);
                assert_eq!(
                    wizard.review_action(wizard.plan.as_ref().unwrap()),
                    if language == "ru" {
                        "Обновить"
                    } else {
                        "Update"
                    }
                );
                wizard.plan.clone().unwrap()
            });
            let stale_approval = approved.clone();
            view.update(visual, |wizard, _| {
                wizard.prefs.resume_update = Some(ResumeUpdate {
                    expected_app_version: "0.1.8".into(),
                    game: game.path().into(),
                    plan: approved,
                    replace: false,
                });
                wizard.save().unwrap();
            });
            let saved: Preferences =
                serde_json::from_slice(&fs::read(preference_path()).unwrap()).unwrap();
            assert_eq!(saved.language, language);
            assert_eq!(saved.nickname, "TestPlayer");
            view.update(visual, |wizard, cx| {
                wizard.prefs = saved;
                wizard.resume_approved_update_at("0.1.8", cx);
                assert!(wizard.step == Step::Busy);
            });
            visual.run_until_parked();
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::Ready));
            assert!(view.read_with(visual, |wizard, _| wizard.prefs.resume_update.is_none()));
            assert_eq!(
                load_state(game.path(), &combined).unwrap().unwrap().version,
                "0.1.14"
            );
            assert_eq!(
                fs::read(game.path().join("config/personal.txt")).unwrap(),
                b"my settings"
            );
            let saved: Preferences =
                serde_json::from_slice(&fs::read(preference_path()).unwrap()).unwrap();
            assert!(saved.resume_update.is_none());
            assert_eq!(saved.game.as_deref(), Some(game.path()));
            assert_eq!(saved.language, language);
            // A crash after the pack commit must not reapply a stale plan.
            view.update(visual, |wizard, cx| {
                wizard.prefs.resume_update = Some(ResumeUpdate {
                    expected_app_version: "0.1.8".into(),
                    game: game.path().into(),
                    plan: stale_approval,
                    replace: false,
                });
                wizard.resume_approved_update_at("0.1.8", cx);
            });
            visual.run_until_parked();
            assert!(view.read_with(visual, |wizard, _| wizard.step == Step::Ready));
            // A skipped release introduces recommended options even after
            // the player disabled CameraOverhaul. They can opt out inline.
            let disabled = plan(
                game.path(),
                &combined,
                BTreeMap::from([("camera-overhaul".into(), false)]),
                false,
            )
            .unwrap();
            apply(game.path(), &disabled, false, Some(cache.path()), |_| {}).unwrap();
            let mut additions = combined.clone();
            additions.version = "0.1.16".into();
            additions.features.push(Feature {
                id: "new-social".into(),
                en: "New social tool".into(),
                ru: "Новый инструмент общения".into(),
                default: true,
                requires: vec![],
            });
            additions.features.push(Feature {
                id: "experimental".into(),
                en: "Experimental".into(),
                ru: "Экспериментальный".into(),
                default: false,
                requires: vec![],
            });
            for name in ["social.jar", "library.jar"] {
                fs::write(cache.path().join(name), b"new").unwrap();
                additions.mods.push(ModFile {
                    id: name.into(),
                    path: format!("mods/{name}"),
                    size: 3,
                    sha512: sha512(b"new"),
                    urls: vec!["https://example.invalid/new.jar".into()],
                    feature: Some("new-social".into()),
                });
            }
            // A newer CameraOverhaul must still be skipped.
            additions.mods[0].path = "mods/camera-new.jar".into();
            additions.mods[0].sha512 = sha512(b"new camera");
            additions.mods[0].size = 10;
            view.update(visual, |wizard, cx| {
                wizard.finish_check(additions.clone(), true).unwrap();
                assert!(!wizard.choices["camera-overhaul"]);
                assert!(wizard.choices["new-social"]);
                assert!(!wizard.choices["experimental"]);
                assert_eq!(wizard.plan.as_ref().unwrap().install.len(), 2);
                cx.notify();
            });
            visual.simulate_resize(size(px(660.), px(1200.)));
            visual.run_until_parked();
            assert!(visual.debug_bounds("new-feature-experimental").is_some());
            let bounds = visual.debug_bounds("new-feature-new-social").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            assert!(
                view.read_with(visual, |wizard, _| !wizard.choices["new-social"]
                    && wizard.plan.as_ref().unwrap().install.is_empty())
            );
            let bounds = visual.debug_bounds("new-feature-new-social").unwrap();
            visual.simulate_click(bounds.center(), Modifiers::none());
            let approved = view.read_with(visual, |wizard, _| wizard.plan.clone().unwrap());
            assert_eq!(approved.install.len(), 2);
            apply(game.path(), &approved, false, Some(cache.path()), |_| {}).unwrap();
            view.update(visual, |wizard, cx| {
                wizard.finish_check(additions.clone(), true).unwrap();
                cx.notify();
            });
            visual.run_until_parked();
            assert!(view.read_with(visual, |wizard, _| {
                wizard.step == Step::CheckResult
                    && wizard
                        .plan
                        .as_ref()
                        .unwrap()
                        .previous
                        .as_ref()
                        .unwrap()
                        .choices
                        .contains_key("new-social")
            }));
            let state = load_state(game.path(), &additions).unwrap().unwrap();
            assert!(!state.choices["camera-overhaul"]);
            assert!(state.choices["new-social"]);
            assert!(!state.choices["experimental"]);
            assert!(!game.path().join("mods/camera-new.jar").exists());
            let mut retired = additions.clone();
            retired.version = "0.1.17".into();
            retired.features.retain(|f| f.id != "new-social");
            retired
                .mods
                .retain(|m| m.feature.as_deref() != Some("new-social"));
            view.update(visual, |wizard, cx| {
                wizard.finish_check(retired.clone(), true).unwrap();
                cx.notify();
            });
            let retirement = view.read_with(visual, |wizard, _| {
                assert!(!wizard.choices.contains_key("new-social"));
                wizard.plan.clone().unwrap()
            });
            assert_eq!(retirement.remove.len(), 2);
            apply(game.path(), &retirement, false, Some(cache.path()), |_| {}).unwrap();
            assert!(!game.path().join("mods/social.jar").exists());
            assert!(!game.path().join("mods/library.jar").exists());
            assert_eq!(
                fs::read(game.path().join("config/personal.txt")).unwrap(),
                b"my settings"
            );
        }
    }
}
