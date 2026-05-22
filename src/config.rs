use serde::Deserialize;
use std::{collections::HashMap, fs};

#[derive(Deserialize)]
struct Binding {
    /// Combo string, e.g. "Caps+D". Supported modifiers: Caps.
    combo: String,
    /// Executable name to find in running processes, e.g. "Discord.exe"
    exe: String,
    /// Optional launch path (supports %ENV_VARS%). If null, `exe` is used directly.
    path: Option<String>,
}

/// Key: (modifier, vk)
pub type BindingMap = HashMap<(Modifier, u32), (String, Option<String>)>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Modifier {
    Caps,
}

#[derive(Deserialize)]
struct Config {
    bindings: Vec<Binding>,
}

#[derive(Debug, Clone)]
struct ParsedBinding {
    modifier: Modifier,
    vk: u32,
    exe: String,
    path: Option<String>,
}

pub fn load_config(path: &str) -> BindingMap {
    let json = fs::read_to_string(path).unwrap_or_else(|e| panic!("Cannot read {path}: {e}"));

    let config: Config =
        serde_json::from_str(&json).unwrap_or_else(|e| panic!("Invalid config JSON: {e}"));

    let mut map = BindingMap::new();
    for binding in config.bindings {
        let parsed = parse_combo(&binding.combo).unwrap_or_else(|| {
            panic!(
                "Invalid combo {:?}. Format: \"Modifier+Key\", e.g. \"Caps+D\"",
                binding.combo
            )
        });
        map.insert((parsed.modifier, parsed.vk), (binding.exe, binding.path));
    }
    map
}

fn parse_combo(combo: &str) -> Option<ParsedBinding> {
    let (modifier_str, key_str) = combo.split_once('+')?;
    let modifier = parse_modifier(modifier_str)?;
    let vk = ascii_char_to_vk(key_str.trim())?;
    Some(ParsedBinding {
        modifier,
        vk,
        exe: String::new(), // filled by caller
        path: None,
    })
}

fn parse_modifier(s: &str) -> Option<Modifier> {
    match s.trim().to_ascii_lowercase().as_str() {
        "caps" | "capslock" | "caps_lock" => Some(Modifier::Caps),
        _ => None,
    }
}

fn ascii_char_to_vk(s: &str) -> Option<u32> {
    let c = s.chars().next()?.to_ascii_uppercase();
    match c {
        'A'..='Z' => Some(c as u32),
        '0'..='9' => Some(c as u32),
        _ => None,
    }
}
