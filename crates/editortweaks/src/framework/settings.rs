//! The mod's settings: switches, choices and counts that change how features behave, shown in the game's
//! options under "Editor tweaks" (`features::settings_menu`) and kept in `EditorTweaks.ini` next to
//! the game's own config files.
//!
//! A feature declares each setting as a `static` next to the code it changes ([`Toggle`],
//! [`Choice`], [`Count`], or [`Note`] for something the mod remembers rather than a choice) and
//! reads it where it uses it, so a change applies at once. Which settings the menu
//! shows, and in what order, is listed in `features::SETTINGS`.

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Mutex, PoisonError};

/// A setting that is on or off.
pub struct Toggle {
    /// Its key in `EditorTweaks.ini`: never rename it, or players lose what they chose.
    pub key: &'static str,
    /// Its name and description in the menu.
    pub name: &'static str,
    pub description: &'static str,
    value: AtomicBool,
}

impl Toggle {
    pub const fn new(
        key: &'static str,
        name: &'static str,
        description: &'static str,
        default: bool,
    ) -> Self {
        Self {
            key,
            name,
            description,
            value: AtomicBool::new(default),
        }
    }

    pub fn get(&self) -> bool {
        self.value.load(Ordering::Relaxed)
    }

    pub fn set(&self, value: bool) {
        self.value.store(value, Ordering::Relaxed);
    }
}

/// A setting that is one of a few options, by index in [`Choice::options`].
pub struct Choice {
    /// Its key in `EditorTweaks.ini`: never rename it, or players lose what they chose.
    pub key: &'static str,
    /// Its name and description in the menu.
    pub name: &'static str,
    pub description: &'static str,
    pub options: &'static [Alternative],
    value: AtomicU8,
}

/// One option of a [`Choice`].
pub struct Alternative {
    /// How `EditorTweaks.ini` names it: never rename it either.
    pub key: &'static str,
    /// Its name in the menu.
    pub name: &'static str,
}

impl Choice {
    /// `default` is an index in `options`.
    pub const fn new(
        key: &'static str,
        name: &'static str,
        description: &'static str,
        options: &'static [Alternative],
        default: u8,
    ) -> Self {
        assert!((default as usize) < options.len());
        Self {
            key,
            name,
            description,
            options,
            value: AtomicU8::new(default),
        }
    }

    /// The index of the option chosen.
    pub fn get(&self) -> u8 {
        self.value.load(Ordering::Relaxed)
    }

    /// Chooses the option at `index`, if there is one.
    pub fn set(&self, index: u8) {
        if usize::from(index) < self.options.len() {
            self.value.store(index, Ordering::Relaxed);
        }
    }
}

/// A setting that is a whole number from `min` to `max`, shown as a slider.
pub struct Count {
    /// Its key in `EditorTweaks.ini`: never rename it, or players lose what they chose.
    pub key: &'static str,
    /// Its name and description in the menu.
    pub name: &'static str,
    pub description: &'static str,
    pub min: u32,
    pub max: u32,
    value: AtomicU32,
}

impl Count {
    /// `default` must be within `min` and `max`.
    pub const fn new(
        key: &'static str,
        name: &'static str,
        description: &'static str,
        min: u32,
        max: u32,
        default: u32,
    ) -> Self {
        assert!(min <= default && default <= max);
        Self {
            key,
            name,
            description,
            min,
            max,
            value: AtomicU32::new(default),
        }
    }

    pub fn get(&self) -> u32 {
        self.value.load(Ordering::Relaxed)
    }

    /// Sets the number, or the nearest one from `min` to `max` if it is outside.
    pub fn set(&self, value: u32) {
        self.value
            .store(value.clamp(self.min, self.max), Ordering::Relaxed);
    }
}

/// A short line of text the mod remembers (e.g. the update not to remind about again), kept in
/// `EditorTweaks.ini` only: never in the menu's `settings`, only a section's `unlisted`.
pub struct Note {
    /// Its key in `EditorTweaks.ini`: never rename it, or players lose what it remembered.
    pub key: &'static str,
    /// What it is, for whoever reads the code (the menu never shows it).
    pub description: &'static str,
    value: Mutex<String>,
}

impl Note {
    /// Starts empty.
    pub const fn new(key: &'static str, description: &'static str) -> Self {
        Self {
            key,
            description,
            value: Mutex::new(String::new()),
        }
    }

    pub fn get(&self) -> String {
        self.value
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Remembers `value`, made one line (`EditorTweaks.ini` holds one value per line).
    pub fn set(&self, value: &str) {
        let line = value.lines().next().unwrap_or_default().trim();
        *self.value.lock().unwrap_or_else(PoisonError::into_inner) = line.to_owned();
    }
}

/// A setting of any kind.
#[derive(Clone, Copy)]
pub enum Setting {
    Toggle(&'static Toggle),
    Choice(&'static Choice),
    Count(&'static Count),
    /// Only in a section's `unlisted`: the menu can't show it.
    Note(&'static Note),
}

impl Setting {
    pub fn key(&self) -> &'static str {
        match self {
            Setting::Toggle(toggle) => toggle.key,
            Setting::Choice(choice) => choice.key,
            Setting::Count(count) => count.key,
            Setting::Note(note) => note.key,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Setting::Toggle(toggle) => toggle.name,
            Setting::Choice(choice) => choice.name,
            Setting::Count(count) => count.name,
            Setting::Note(note) => note.key,
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Setting::Toggle(toggle) => toggle.description,
            Setting::Choice(choice) => choice.description,
            Setting::Count(count) => count.description,
            Setting::Note(note) => note.description,
        }
    }

    /// The value as `EditorTweaks.ini` writes it.
    fn saved(&self) -> Cow<'static, str> {
        match self {
            Setting::Toggle(toggle) if toggle.get() => "True".into(),
            Setting::Toggle(_) => "False".into(),
            Setting::Choice(choice) => choice.options[usize::from(choice.get())].key.into(),
            Setting::Count(count) => count.get().to_string().into(),
            Setting::Note(note) => note.get().into(),
        }
    }

    /// Takes the value `EditorTweaks.ini` wrote; whether it was one this setting can have.
    fn load(&self, saved: &str) -> bool {
        match self {
            Setting::Toggle(toggle) => {
                let value = match saved.to_ascii_lowercase().as_str() {
                    "true" | "1" => true,
                    "false" | "0" => false,
                    _ => return false,
                };
                toggle.set(value);
                true
            }
            Setting::Choice(choice) => {
                let found = choice
                    .options
                    .iter()
                    .position(|option| option.key.eq_ignore_ascii_case(saved));
                if let Some(index) = found {
                    choice.set(index as u8);
                }
                found.is_some()
            }
            Setting::Count(count) => match saved.parse::<u32>() {
                Ok(number) => {
                    // A number out of range is still a number: `set` takes the nearest one.
                    count.set(number);
                    true
                }
                Err(_) => false,
            },
            Setting::Note(note) => {
                note.set(saved);
                true
            }
        }
    }
}

/// Settings shown together in the menu, under `title`.
pub struct Section {
    pub title: &'static str,
    pub settings: &'static [Setting],
    /// Settings saved with these but not in the menu: each is changed elsewhere (a row of the
    /// feature's own panel), or only in `EditorTweaks.ini` (too rarely wanted to show).
    pub unlisted: &'static [Setting],
}

/// Every setting of `sections` (each feature's list of sections), listed or not.
pub fn all(sections: &[&[Section]]) -> impl Iterator<Item = Setting> {
    sections
        .iter()
        .flat_map(|sections| sections.iter())
        .flat_map(|section| section.settings.iter().chain(section.unlisted).copied())
}

/// The header line of `EditorTweaks.ini`'s only section.
const INI_SECTION: &str = "[EditorTweaks]";

/// Where the settings are kept: with the game's own config files (`Input.ini`, ...).
fn path() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local)
            .join(r"BrickRigs\SavedRemastered\Config\WindowsNoEditor\EditorTweaks.ini"),
    )
}

/// Reads `EditorTweaks.ini` into the settings of `sections`: what it doesn't mention (or no file)
/// keeps its default. What it held that no setting took, to log.
pub fn load(sections: &[&[Section]]) -> Result<Vec<String>, String> {
    let path = path().ok_or("no LOCALAPPDATA folder")?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{} not read: {error}", path.display())),
    };
    let mut ignored = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with([';', '#', '[']) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            ignored.push(line.to_owned());
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let taken = all(sections)
            .find(|setting| setting.key().eq_ignore_ascii_case(key))
            .is_some_and(|setting| setting.load(value));
        if !taken {
            ignored.push(line.to_owned());
        }
    }
    Ok(ignored)
}

/// Writes every setting of `sections` to `EditorTweaks.ini`.
pub fn save(sections: &[&[Section]]) -> Result<(), String> {
    let path = path().ok_or("no LOCALAPPDATA folder")?;
    let mut text = format!(
        "; editortweaks settings, also set in the game's options (Editor tweaks).\n{INI_SECTION}\n"
    );
    for setting in all(sections) {
        text += &format!("{}={}\n", setting.key(), setting.saved());
    }
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    }
    fs::write(&path, text).map_err(|error| format!("{} not written: {error}", path.display()))
}
