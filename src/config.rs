//! Configuration for the modern Graphic Packet rewrite.
//!
//! Classic GP stored settings in `CONFIG.GP` (and baked defaults into
//! `GP2862K.EXE`). This module:
//!
//! 1. Exposes a typed [`Config`] tree for `Config.toml`.
//! 2. Accepts the historical key names (`MYCALL`, `SERBAUD`, `CHANNELS`, …)
//!    when loading a legacy-style key=value file.
//! 3. Normalises both sources into one runtime structure.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ui::channel_bits::{
    MASK_BELL, MASK_ECHO, MASK_REMOTE, MASK_SEVEN_PLUS, MASK_SOFTSCROLL, MASK_UMLAUT,
};

/// Accept a legacy bool (`true` → all bits of `mask`) or an integer bit mask.
fn deserialize_channel_bits<'de, D>(deserializer: D, mask: u32) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    struct BitsVisitor {
        mask: u32,
    }
    impl<'de> Visitor<'de> for BitsVisitor {
        type Value = u32;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "bool or integer channel bit mask")
        }
        fn visit_bool<E: de::Error>(self, v: bool) -> Result<u32, E> {
            Ok(if v { self.mask } else { 0 })
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u32, E> {
            Ok((v as u32) & self.mask)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u32, E> {
            if v < 0 {
                return Ok(0);
            }
            Ok((v as u32) & self.mask)
        }
    }
    deserializer.deserialize_any(BitsVisitor { mask })
}

fn de_umlaut<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    deserialize_channel_bits(d, MASK_UMLAUT)
}
fn de_softscroll<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    deserialize_channel_bits(d, MASK_SOFTSCROLL)
}
fn de_echo<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    deserialize_channel_bits(d, MASK_ECHO)
}
fn de_bell<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    deserialize_channel_bits(d, MASK_BELL)
}
fn de_seven_plus<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    deserialize_channel_bits(d, MASK_SEVEN_PLUS)
}
fn de_remote<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    deserialize_channel_bits(d, MASK_REMOTE)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config from {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid TOML in {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("legacy config parse error in {path}: {message}")]
    Legacy { path: PathBuf, message: String },
}

// ---------------------------------------------------------------------------
// Root configuration
// ---------------------------------------------------------------------------

/// Complete runtime configuration (modern + legacy-mapped).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub station: StationConfig,
    pub serial: SerialConfig,
    pub session: SessionConfig,
    pub buffers: BufferConfig,
    pub paths: PathConfig,
    pub tnc: TncConfig,
    pub colors: ColorConfig,
    pub beacon: BeaconConfig,
    pub print: PrintConfig,
    /// Last native window place and size.
    pub window: WindowConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            station: StationConfig::default(),
            serial: SerialConfig::default(),
            session: SessionConfig::default(),
            buffers: BufferConfig::default(),
            paths: PathConfig::default(),
            tnc: TncConfig::default(),
            colors: ColorConfig::default(),
            beacon: BeaconConfig::default(),
            print: PrintConfig::default(),
            window: WindowConfig::default(),
        }
    }
}

impl Config {
    /// Load `Config.toml` if present; otherwise return defaults.
    ///
    /// Also tries a sibling legacy `CONFIG.GP` / `config.gp` and merges
    /// recognised keys on top of the TOML (or defaults).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let mut cfg = if path.exists() {
            let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let upgraded = upgrade_document(&text).map_err(|source| ConfigError::Toml {
                path: path.to_path_buf(),
                source,
            })?;
            if !upgraded.missing.is_empty() {
                let names = upgraded
                    .missing
                    .iter()
                    .map(|(key, _)| key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let patched = insert_missing_keys(&text, &upgraded.missing);
                if patched != text {
                    match fs::write(path, patched) {
                        Ok(()) => {
                            tracing::info!("stored missing settings in {}: {names}", path.display())
                        }
                        Err(err) => tracing::warn!(
                            "could not store missing settings in {}: {err}",
                            path.display()
                        ),
                    }
                }
            }
            for key in &upgraded.replaced {
                tracing::warn!(
                    "config {key} cannot be read by this version — using the built-in default"
                );
            }
            upgraded.cfg
        } else {
            tracing::warn!(
                "config file {} not found — using built-in defaults",
                path.display()
            );
            Self::default()
        };

        // Optional legacy overlay next to the modern file, or classic name.
        let legacy_candidates = [
            path.with_file_name("CONFIG.GP"),
            path.with_file_name("config.gp"),
            PathBuf::from("CONFIG.GP"),
        ];
        for candidate in &legacy_candidates {
            if candidate.exists() && candidate != path {
                tracing::info!("merging legacy settings from {}", candidate.display());
                cfg.merge_legacy_file(candidate)?;
                break;
            }
        }

        cfg.capture_own_calls();
        cfg.normalize_spy_calls();
        cfg.session.language = crate::i18n::migrate_language(&cfg.session.language);
        crate::i18n::set_language(&cfg.session.language);
        // Relative dirs follow Config.toml, so a release copy works even when
        // the process is started with a different working directory.
        let base = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        cfg.paths.user_dir = anchor_to(base, cfg.paths.user_dir);
        cfg.paths.external_dir = anchor_to(base, cfg.paths.external_dir);
        cfg.paths.save_dir = anchor_to(base, cfg.paths.save_dir);
        cfg.paths.log_dir = anchor_to(base, cfg.paths.log_dir);
        cfg.paths.spy_dir = anchor_to(base, cfg.paths.spy_dir);
        Ok(cfg)
    }

    /// Parse a modern TOML configuration file.
    ///
    /// Settings this version added are filled from [`Config::default`] when
    /// the file does not have them. A value the new field cannot read is
    /// replaced with that default so one old line cannot reject the file.
    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        upgrade_document(&text)
            .map(|upgraded| upgraded.cfg)
            .map_err(|source| ConfigError::Toml {
                path: path.to_path_buf(),
                source,
            })
    }

    /// Overlay values from a classic `CONFIG.GP` key=value document.
    pub fn merge_legacy_file(&mut self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        self.merge_legacy_text(&text)
            .map_err(|message| ConfigError::Legacy {
                path: path.to_path_buf(),
                message,
            })
    }

    /// Apply legacy `KEY = value` lines (semicolon comments, case-insensitive keys).
    pub fn merge_legacy_text(&mut self, text: &str) -> Result<(), String> {
        // TNCINI / TNCDEI / TNCDOS may appear multiple times — collect then replace.
        let mut tnc_init = Vec::new();
        let mut tnc_dei = Vec::new();
        let mut tnc_dos = Vec::new();
        let mut saw_tnc_init = false;
        let mut saw_tnc_dei = false;
        let mut saw_tnc_dos = false;

        for (lineno, raw) in text.lines().enumerate() {
            let line = strip_legacy_comment(raw).trim();
            if line.is_empty() {
                continue;
            }

            let (key, value) = split_legacy_kv(line)
                .ok_or_else(|| format!("line {}: expected KEY = VALUE, got `{raw}`", lineno + 1))?;
            let key_upper = key.to_ascii_uppercase();
            let value = value.trim();

            match key_upper.as_str() {
                // --- station ------------------------------------------------
                "MYCALL" => {
                    self.station.mycall = split_csv(value);
                }
                "ZONE" => self.station.zone = value.to_string(),
                "TIME" => {
                    self.station.time_offset_minutes = parse_i32(value, "TIME")?;
                }

                // --- serial -------------------------------------------------
                "SERBAUD" => self.serial.baud = parse_u32(value, "SERBAUD")?,
                "SERNR" => {
                    self.serial.com_number = parse_u32(value, "SERNR")?;
                    // Windows: synthesise COMn when the operator only set SERNR.
                    // Other OSes keep the existing port (or leave it empty).
                    #[cfg(windows)]
                    if self.serial.port.is_empty() || self.serial.port.starts_with("COM") {
                        self.serial.port = format!("COM{}", self.serial.com_number);
                    }
                }
                "SERPORT" => {
                    self.serial.io_port = Some(parse_hex_or_dec(value)?);
                }
                "SERIRQ" => self.serial.irq = Some(parse_u32(value, "SERIRQ")?),
                "FASTPOLL" => self.serial.fast_poll = value.to_ascii_lowercase(),
                "MULTIPORT" => self.serial.multiport = value.to_ascii_lowercase(),

                // --- session ------------------------------------------------
                "CHANNELS" => self.session.channels = parse_u32(value, "CHANNELS")?,
                "MAXSPY" => self.session.max_spy = parse_u32(value, "MAXSPY")?,
                "CONNECTBELL" => {
                    self.session.connect_bell = parse_u32(value, "CONNECTBELL")?;
                }
                "RXCLICK" => self.session.rx_click = parse_u32(value, "RXCLICK")?,
                "SCREENSAVE" => {
                    self.session.screensave_minutes = parse_u32(value, "SCREENSAVE")?;
                }
                "POPUPTIME" => {
                    self.session.popup_time_seconds = parse_u32(value, "POPUPTIME")?;
                }
                "POPUPENABLEKEYBOARD" => {
                    self.session.popup_enable_keyboard = parse_boolish(value);
                }
                "INSERTMONLINE" => self.session.insert_mon_line = parse_boolish(value),
                "QUESTIONS" => self.session.questions = parse_boolish(value),
                "BACKUP" => self.session.backup = parse_boolish(value),
                "ZOOM" => self.session.zoom = parse_u32(value, "ZOOM")?,
                "PADFONT" => {
                    self.session.pad_font_size = parse_u32(value, "PADFONT")?;
                }
                "PROMPT" => self.session.prompt = value.to_string(),
                "NONAME" => self.session.noname = value.to_string(),
                "REMOTELANGUAGE" | "LANGUAGE" => {
                    self.session.language = crate::i18n::migrate_language(value);
                }
                "SECURITY" => self.session.security = parse_boolish(value),
                "REMOTE" => self.session.remote = split_csv(value),
                "NOREMOTE" => self.session.no_remote = split_csv(value),
                // Comma-separated phrases; spaces inside a phrase matter (`fm DB0SGL`).
                "AUTOSPY" => self.session.auto_spy = split_csv(value),
                "NOAUTOSPY" => self.session.no_auto_spy = split_csv(value),
                "AUTOSPYMINIMUMSAVE" => {
                    self.session.auto_spy_minimum_save = parse_u32(value, "AUTOSPYMINIMUMSAVE")?;
                }

                // --- buffers ------------------------------------------------
                "TEXTBUFFER" => {
                    self.buffers.text_buffer = parse_u32_list(value, "TEXTBUFFER")?;
                }
                "EDITBUFFER" => {
                    self.buffers.edit_buffer = parse_u32(value, "EDITBUFFER")?;
                }
                "CHECKBUFFER" => {
                    self.buffers.check_buffer = parse_u32(value, "CHECKBUFFER")?;
                }
                "LISTBUFFER" => {
                    self.buffers.list_buffer = parse_u32(value, "LISTBUFFER")?;
                }
                "SPYBUFFER" => {
                    self.buffers.spy_buffer = parse_u32(value, "SPYBUFFER")?;
                }
                "MHBUFFER" => self.buffers.mh_buffer = parse_u32(value, "MHBUFFER")?,
                "INPUTBUFFER" => {
                    self.buffers.input_buffer = parse_u32(value, "INPUTBUFFER")?;
                }
                "PACLEN" | "PACKETPAYLOAD" => {
                    self.buffers.packet_payload = parse_u32(value, "PACLEN")?;
                }

                // --- paths --------------------------------------------------
                "USERDIR" => self.paths.user_dir = PathBuf::from(value),
                "EXTERNALDIR" => self.paths.external_dir = PathBuf::from(value),
                "SAVEDIR" => self.paths.save_dir = PathBuf::from(value),
                "LOGDIR" => self.paths.log_dir = PathBuf::from(value),
                "SPYDIR" => self.paths.spy_dir = PathBuf::from(value),

                // --- TNC command lists (multi-valued) -----------------------
                "TNCINI" => {
                    saw_tnc_init = true;
                    tnc_init.push(value.to_string());
                }
                "TNCDEI" => {
                    saw_tnc_dei = true;
                    tnc_dei.push(value.to_string());
                }
                "TNCDOS" => {
                    saw_tnc_dos = true;
                    tnc_dos.push(value.to_string());
                }

                // --- colours (legacy VGA names) -----------------------------
                "CLOCKBACKGROUND" => self.colors.clock_background = value.to_string(),
                "INPUTFOREGROUND" => self.colors.input_foreground = value.to_string(),
                "INPUTBACKGROUND" => self.colors.input_background = value.to_string(),
                "MONITORFOREGROUND" => {
                    self.colors.monitor_foreground = value.to_string();
                }
                "MONITORBACKGROUND" => {
                    self.colors.monitor_background = value.to_string();
                }
                "CONNECTFOREGROUND" => {
                    self.colors.connect_foreground = value.to_string();
                }
                "CONNECTBACKGROUND" => {
                    self.colors.connect_background = value.to_string();
                }
                "STATEBACKGROUND" => self.colors.state_background = value.to_string(),
                "STATEFOREGROUND" => self.colors.state_foreground = value.to_string(),
                "MENUBACKGROUND" => self.colors.menu_background = value.to_string(),
                "MENUFOREGROUND" => self.colors.menu_foreground = value.to_string(),
                "MENUINVERSATTR" => self.colors.menu_invers_attr = value.to_string(),
                "BORDERATTR" => self.colors.border_attr = value.to_string(),
                "CONNECTINVERSATTR" => {
                    self.colors.connect_invers_attr = value.to_string();
                }
                "MONITORINVERSATTR" => {
                    self.colors.monitor_invers_attr = value.to_string();
                }
                "MONITORINFOERROR" => {
                    self.colors.monitor_infoerror = value.to_string();
                }
                "EDITORFOREGROUND" => {
                    self.colors.editor_foreground = value.to_string();
                }
                "EDITORBACKGROUND" => {
                    self.colors.editor_background = value.to_string();
                }
                "CHECKFOREGROUND" => self.colors.check_foreground = value.to_string(),
                "CHECKBACKGROUND" => self.colors.check_background = value.to_string(),
                "CHECKINVERSATTR" => {
                    self.colors.check_invers_attr = value.to_string();
                }
                "EDITORINVERSATTR" => {
                    self.colors.editor_invers_attr = value.to_string();
                }
                "LISTBACKGROUND" => self.colors.list_background = value.to_string(),
                "LISTFOREGROUND" => self.colors.list_foreground = value.to_string(),
                "LISTINVERSATTR" => self.colors.list_invers_attr = value.to_string(),
                "MHBACKGROUND" => self.colors.mh_background = value.to_string(),
                "MHFOREGROUND" => self.colors.mh_foreground = value.to_string(),
                "MHINVERSATTR" => self.colors.mh_invers_attr = value.to_string(),
                "SPYBACKGROUND" => self.colors.spy_background = value.to_string(),
                "SPYFOREGROUND" => self.colors.spy_foreground = value.to_string(),
                "SPYINVERSATTR" => self.colors.spy_invers_attr = value.to_string(),

                // --- printer ------------------------------------------------
                "DEFAULTPRINT" => {
                    self.print.default_print = parse_u8_list(value, "DEFAULTPRINT")?;
                }
                "INVERSPRINT" => {
                    self.print.invers_print = parse_u8_list(value, "INVERSPRINT")?;
                }

                "LOG" => {
                    self.session.log = parse_on_off(value);
                }
                "QRG" => {
                    if let Ok(text) = crate::journal::normalize_qrg(value) {
                        if let Ok(mhz) = text.parse::<f64>() {
                            self.session.chrome.qrg_mhz = mhz;
                        }
                        self.session.qrg_text = text;
                    } else {
                        self.session.qrg_text = value.trim().chars().take(7).collect();
                    }
                }
                "UTC" => {
                    self.session.utc_offset_min = parse_i32(value, "UTC")?;
                }

                // Known but ignored modern-irrelevant keys (e.g. GRAPHIC=VGA).
                "GRAPHIC" => {
                    tracing::debug!("ignoring legacy display key `{key_upper}`");
                }

                other => {
                    tracing::debug!("unrecognised legacy key `{other}` — skipped");
                }
            }
        }

        if saw_tnc_init {
            self.tnc.init = tnc_init;
        }
        if saw_tnc_dei {
            self.tnc.deinit = tnc_dei;
        }
        if saw_tnc_dos {
            self.tnc.dos_shell = tnc_dos;
        }

        Ok(())
    }

    /// Primary station callsign (monitor / channel-0 MYCALL).
    pub fn primary_mycall(&self) -> &str {
        self.station
            .mycall
            .first()
            .map(String::as_str)
            .unwrap_or("N0CALL")
    }

    /// Callsign base without `-SSID` (`NL0MSK-2` → `NL0MSK`).
    pub fn mycall_base(call: &str) -> &str {
        call.split('-').next().unwrap_or(call)
    }

    /// Classic per-channel identity: `{base}-{channel}` (channel `0` = bare base).
    pub fn channel_ssid_mycall(primary: &str, channel: u8) -> String {
        let base = Self::mycall_base(primary);
        if channel == 0 {
            base.to_string()
        } else {
            format!("{base}-{channel}")
        }
    }

    /// True if `call` is our station (same base as primary MYCALL).
    pub fn is_own_call(&self, call: &str) -> bool {
        Self::mycall_base(call).eq_ignore_ascii_case(Self::mycall_base(self.primary_mycall()))
    }

    /// MYCALL for a slot: `0` = monitor / global, `1..=N` = channel.
    /// Channels without an override fall back to the primary call.
    pub fn mycall_for_slot(&self, slot: u8) -> &str {
        let idx = slot as usize;
        self.station
            .mycall
            .get(idx)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.primary_mycall())
    }

    /// Set MYCALL for a slot (extends the list as needed). Returns the
    /// normalised callsign. An empty channel call joins the global pool.
    pub fn set_mycall_for_slot(&mut self, slot: u8, call: &str) -> Result<String, String> {
        if slot != 0 && call.trim().is_empty() {
            let idx = slot as usize;
            let primary = self.primary_mycall().to_string();
            while self.station.mycall.len() <= idx {
                self.station.mycall.push(primary.clone());
            }
            self.station.mycall[idx].clear();
            return Ok(primary);
        }
        let call = normalize_mycall(call)?;
        let idx = slot as usize;
        let primary = self.primary_mycall().to_string();
        while self.station.mycall.len() <= idx {
            self.station.mycall.push(primary.clone());
        }
        self.station.mycall[idx] = call.clone();
        if slot == 0 {
            self.tnc.runtime_mycall = call.clone();
        }
        self.capture_own_calls();
        Ok(call)
    }

    /// Refresh the calls TFPCX treats as local. Those never go into the host file.
    pub fn capture_own_calls(&mut self) {
        self.tnc.runtime_mycall = self.primary_mycall().to_string();
        let mut own: Vec<String> = self
            .station
            .mycall
            .iter()
            .filter(|call| !call.trim().is_empty())
            .cloned()
            .collect();
        if own.is_empty() && !self.tnc.runtime_mycall.is_empty() {
            own.push(self.tnc.runtime_mycall.clone());
        }
        self.tnc.runtime_own_calls = own;
    }

    /// Rewrite the `mycall = [...]` line in a Config.toml, keeping comments.
    pub fn persist_mycall(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        let list = self
            .station
            .mycall
            .iter()
            .map(|c| format!("\"{}\"", c.replace('"', "")))
            .collect::<Vec<_>>()
            .join(", ");
        upsert_toml_key(path, None, "mycall", &format!("[{list}]"))
    }

    /// Rewrite `locator` under `[station]` in Config.toml.
    pub fn persist_locator(&self, path: impl AsRef<Path>) -> Result<(), String> {
        put_key(
            path.as_ref(),
            "station",
            "locator",
            &toml_string(&self.station.locator),
        )
    }

    /// Rewrite / create the `[session.chrome]` block for session-bar buttons.
    pub fn persist_chrome(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        let c = &self.session.chrome;
        let ctext = toml_string(&c.ctext);
        let channel_ctext = c
            .channel_ctext
            .iter()
            .map(|path| toml_string(path))
            .collect::<Vec<_>>()
            .join(", ");
        let qrg = crate::journal::format_mhz(c.qrg_mhz);
        let body = format!(
            "qrg_mhz = {qrg}\n\
             ctext = {ctext}\n\
             channel_ctext = [{channel_ctext}]\n\
             umlaut = {}\n\
             softscroll = {}\n\
             echo = {}\n\
             bell = {}\n\
             seven_plus = {}\n\
             remote = {}\n\
             scr_lock = {}\n\
             hour_gong = {}\n",
            c.umlaut, c.softscroll, c.echo, c.bell, c.seven_plus, c.remote, c.scr_lock, c.hour_gong,
        );
        upsert_toml_section(path, "session.chrome", &body)
    }

    /// Keep `//Language` / Setup language across a restart (`language` in Config.toml).
    pub fn persist_language(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let value = toml_string(&self.session.language);
        upsert_toml_key(path.as_ref(), None, "language", &value)
    }

    /// Ensure `spy_calls` has 10 entries; rewrite the TOML array.
    pub fn normalize_spy_calls(&mut self) {
        self.session.spy_calls.resize(10, String::new());
    }

    pub fn persist_spy_calls(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        let mut calls = self.session.spy_calls.clone();
        calls.resize(10, String::new());
        let list = calls
            .iter()
            .map(|c| toml_string(c))
            .collect::<Vec<_>>()
            .join(", ");
        // Prefer updating existing key; if missing, append under [session].
        match upsert_toml_key(path, None, "spy_calls", &format!("[{list}]")) {
            Ok(()) => Ok(()),
            Err(_) => append_section_key(path, "session", "spy_calls", &format!("[{list}]")),
        }
    }

    /// Setup menu values. Written into Config.toml (DOS used `CSTAT.GPB`).
    pub fn persist_setup(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        let s = &self.session;
        put_key(
            path,
            "session",
            "backup",
            if s.backup { "true" } else { "false" },
        )?;
        put_key(path, "session", "rx_click", &s.rx_click.to_string())?;
        put_key(
            path,
            "session",
            "max_spy",
            &s.max_spy.clamp(1, 8).to_string(),
        )?;
        put_key(path, "session", "connect_bell", &s.connect_bell.to_string())?;
        put_key(
            path,
            "session",
            "insert_mon_line",
            if s.insert_mon_line { "true" } else { "false" },
        )?;
        put_key(path, "session", "prompt", &toml_string(&s.prompt))?;
        put_key(path, "session", "language", &toml_string(&s.language))?;
        put_key(path, "session", "zoom", &s.zoom.to_string())?;
        put_key(
            path,
            "session",
            "pad_font_size",
            &clamp_pad_font(s.pad_font_size).to_string(),
        )?;
        put_key(path, "session", "icon_file", &toml_string(&s.icon_file))?;
        put_key(
            path,
            "session",
            "input_columns",
            &s.input_columns.to_string(),
        )?;
        put_key(path, "session", "editor_wrap", &s.editor_wrap.to_string())?;
        put_key(
            path,
            "session",
            "editor_bak",
            if s.editor_bak { "true" } else { "false" },
        )?;
        put_key(path, "session", "bbs_channel", &s.bbs_channel.to_string())?;
        put_key(path, "session", "list_channel", &s.list_channel.to_string())?;
        put_key(path, "session", "file_drawer", &toml_string(&s.file_drawer))?;
        put_key(path, "session", "auto_spy", &toml_string_list(&s.auto_spy))?;
        put_key(
            path,
            "session",
            "no_auto_spy",
            &toml_string_list(&s.no_auto_spy),
        )?;
        put_key(
            path,
            "session",
            "mheard_clean_days",
            &s.mheard_clean_days.clamp(0, 365).to_string(),
        )?;
        put_key(
            path,
            "buffers",
            "packet_payload",
            &self.buffers.packet_payload.to_string(),
        )?;
        let c = &self.colors;
        for (key, value) in [
            ("clock_background", c.clock_background.as_str()),
            ("input_background", c.input_background.as_str()),
            ("input_foreground", c.input_foreground.as_str()),
            ("input_invers_attr", c.input_invers_attr.as_str()),
            ("monitor_background", c.monitor_background.as_str()),
            ("monitor_foreground", c.monitor_foreground.as_str()),
            ("monitor_invers_attr", c.monitor_invers_attr.as_str()),
            ("monitor_infoerror", c.monitor_infoerror.as_str()),
            ("connect_background", c.connect_background.as_str()),
            ("connect_foreground", c.connect_foreground.as_str()),
            ("connect_invers_attr", c.connect_invers_attr.as_str()),
            ("editor_background", c.editor_background.as_str()),
            ("editor_foreground", c.editor_foreground.as_str()),
            ("editor_invers_attr", c.editor_invers_attr.as_str()),
            ("check_background", c.check_background.as_str()),
            ("check_foreground", c.check_foreground.as_str()),
            ("check_invers_attr", c.check_invers_attr.as_str()),
            ("list_background", c.list_background.as_str()),
            ("list_foreground", c.list_foreground.as_str()),
            ("list_invers_attr", c.list_invers_attr.as_str()),
            ("mh_background", c.mh_background.as_str()),
            ("mh_foreground", c.mh_foreground.as_str()),
            ("mh_invers_attr", c.mh_invers_attr.as_str()),
            ("spy_background", c.spy_background.as_str()),
            ("spy_foreground", c.spy_foreground.as_str()),
            ("spy_invers_attr", c.spy_invers_attr.as_str()),
            ("map_background", c.map_background.as_str()),
            ("map_foreground", c.map_foreground.as_str()),
            ("map_grid", c.map_grid.as_str()),
            ("map_qth", c.map_qth.as_str()),
            ("map_heard", c.map_heard.as_str()),
            ("state_background", c.state_background.as_str()),
            ("state_foreground", c.state_foreground.as_str()),
            ("state_invers_attr", c.state_invers_attr.as_str()),
            ("menu_background", c.menu_background.as_str()),
            ("menu_foreground", c.menu_foreground.as_str()),
            ("menu_invers_attr", c.menu_invers_attr.as_str()),
        ] {
            put_key(path, "colors", key, &toml_string(value))?;
        }
        put_key(
            path,
            "station",
            "locator",
            &toml_string(&self.station.locator),
        )?;
        let b = &self.beacon;
        let body = format!(
            "address = {}\nsource = {}\ntext = {}\nfile = {}\nperiod_minutes = {}\n",
            toml_string(&b.address),
            toml_string(&b.source),
            toml_string(&b.text),
            toml_string(&b.file),
            b.period_minutes,
        );
        upsert_toml_section(path, "beacon", &body)
    }

    /// Startup radio card: protocol, KISS serial/TCP, COM port, and baud.
    pub fn persist_radio(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        put_key(path, "tnc", "protocol", &toml_string(&self.tnc.protocol))?;
        put_key(
            path,
            "tnc",
            "kiss_transport",
            &toml_string(&self.tnc.kiss_transport),
        )?;
        put_key(
            path,
            "tnc",
            "kiss_tcp_host",
            &toml_string(&self.tnc.kiss_tcp_host),
        )?;
        put_key(
            path,
            "tnc",
            "kiss_tcp_port",
            &self.tnc.kiss_tcp_port.to_string(),
        )?;
        put_key(path, "serial", "port", &toml_string(&self.serial.port))?;
        put_key(
            path,
            "serial",
            "port_detail",
            &toml_string(&self.serial.port_detail),
        )?;
        put_key(path, "serial", "baud", &self.serial.baud.to_string())?;
        put_key(
            path,
            "serial",
            "com_number",
            &self.serial.com_number.to_string(),
        )?;
        put_key(path, "tnc", "links", &links_toml(&self.tnc.links))?;
        Ok(())
    }

    /// Outer position and inner size of the native window.
    pub fn persist_window(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let w = &self.window;
        let pos = match (w.x, w.y) {
            (Some(x), Some(y)) => format!("x = {x:.0}\ny = {y:.0}\n"),
            _ => String::new(),
        };
        let body = format!(
            "{pos}width = {width:.0}\nheight = {height:.0}\nmaximized = {}\n",
            w.maximized,
            width = w.width,
            height = w.height,
        );
        upsert_toml_section(path.as_ref(), "window", &body)
    }
}

fn put_key(path: &Path, section: &str, key: &str, value: &str) -> Result<(), String> {
    match upsert_toml_key(path, None, key, value) {
        Ok(()) => Ok(()),
        Err(_) => append_section_key(path, section, key, value),
    }
}

fn append_section_key(path: &Path, section: &str, key: &str, value: &str) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let header = format!("[{section}]");
    let mut out = String::with_capacity(text.len() + 64);
    let mut inserted = false;
    let ends_nl = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        out.push_str(lines[i]);
        out.push('\n');
        if !inserted && lines[i].trim().eq_ignore_ascii_case(&header) {
            // Insert after [session] header (before next section or at end of session keys).
            let mut j = i + 1;
            while j < lines.len() {
                let t = lines[j].trim();
                if t.starts_with('[') && t.ends_with(']') {
                    break;
                }
                j += 1;
            }
            for line in &lines[i + 1..j] {
                out.push_str(line);
                out.push('\n');
            }
            out.push_str(&format!("{key} = {value}\n"));
            inserted = true;
            i = j;
            continue;
        }
        i += 1;
    }
    if !inserted {
        out.push_str(&format!("\n{header}\n{key} = {value}\n"));
    }
    if !ends_nl && out.ends_with('\n') {
        out.pop();
    }
    fs::write(path, out).map_err(|e| format!("write {}: {e}", path.display()))
}

/// `#` comment after the value. A hash inside quotes stays part of the value.
fn trailing_hash_comment(line: &str) -> &str {
    let mut quote = false;
    let mut escape = false;
    for (i, c) in line.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match c {
            '\\' if quote => escape = true,
            '"' => quote = !quote,
            '#' if !quote => return line[i..].trim_end(),
            _ => {}
        }
    }
    ""
}

fn links_toml(links: &[TncLink]) -> String {
    let body = links
        .iter()
        .map(|link| {
            let ports = link
                .ports
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{{ protocol = {}, ports = [{ports}], transport = {}, serial_port = {}, baud = {}, tcp_host = {}, tcp_port = {} }}",
                toml_string(&link.protocol),
                toml_string(&link.transport),
                toml_string(&link.serial_port),
                link.baud,
                toml_string(&link.tcp_host),
                link.tcp_port,
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{body}]")
}

fn toml_string(s: &str) -> String {
    // Basic TOML basic-string escaping.
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

fn toml_string_list(items: &[String]) -> String {
    let body = items
        .iter()
        .map(|s| toml_string(s))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{body}]")
}

/// Replace or insert `key = value` at file top-level (or under first match of key).
fn upsert_toml_key(
    path: &Path,
    _section: Option<&str>,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let replacement = format!("{key} = {value}");
    let key_lc = key.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len() + 32);
    let mut replaced = false;
    let ends_nl = text.ends_with('\n');
    for line in text.lines() {
        let trimmed = line.trim_start();
        let is_key = {
            let head = trimmed.split('=').next().unwrap_or("").trim();
            !replaced && head.eq_ignore_ascii_case(&key_lc)
        };
        if is_key {
            let indent_len = line.len() - trimmed.len();
            out.push_str(&line[..indent_len]);
            out.push_str(&replacement);
            let comment = trailing_hash_comment(line);
            if !comment.is_empty() {
                out.push(' ');
                out.push_str(comment);
            }
            out.push('\n');
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        return Err(format!("no {key} = line found in {}", path.display()));
    }
    if !ends_nl && out.ends_with('\n') {
        out.pop();
    }
    fs::write(path, out).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Replace an existing `[name]` section body, or append the section.
fn upsert_toml_section(path: &Path, name: &str, body: &str) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let header = format!("[{name}]");
    let header_lc = header.to_ascii_lowercase();
    let ends_nl = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();

    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim().eq_ignore_ascii_case(&header_lc) {
            start = Some(i);
            break;
        }
    }

    let mut out = String::with_capacity(text.len() + body.len() + 32);
    if let Some(s) = start {
        for line in &lines[..s] {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(&header);
        out.push('\n');
        out.push_str(body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
        let mut i = s + 1;
        while i < lines.len() {
            let t = lines[i].trim();
            if t.starts_with('[') && t.ends_with(']') {
                break;
            }
            i += 1;
        }
        for line in &lines[i..] {
            out.push_str(line);
            out.push('\n');
        }
    } else {
        out.push_str(&text);
        if !text.is_empty() && !text.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&header);
        out.push('\n');
        out.push_str(body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
    if !ends_nl && out.ends_with('\n') && start.is_some() {
        // keep trailing newline if original had one; section append always ends with \n
    }
    fs::write(path, out).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Uppercase callsign with optional `-SSID` (1–15).
pub fn normalize_mycall(raw: &str) -> Result<String, String> {
    let t = raw.trim().to_ascii_uppercase();
    if t.is_empty() {
        return Err("mycall is empty".into());
    }
    let (base, ssid) = match t.split_once('-') {
        Some((b, s)) => (b, Some(s)),
        None => (t.as_str(), None),
    };
    if base.is_empty() || base.len() > 6 || !base.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("invalid call '{t}'"));
    }
    if let Some(s) = ssid {
        let n: u8 = s.parse().map_err(|_| format!("invalid SSID in '{t}'"))?;
        if n > 15 {
            return Err(format!("SSID out of range in '{t}'"));
        }
        Ok(format!("{base}-{n}"))
    } else {
        Ok(base.to_string())
    }
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

/// Station identity — legacy `MYCALL`, `ZONE`, `TIME`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StationConfig {
    /// Monitor call first, then optional per-channel overrides.
    pub mycall: Vec<String>,
    pub zone: String,
    pub time_offset_minutes: i32,
    /// Own QTH as a 6- or 8-character Maidenhead locator (`JO32KX` / `JO32MW49`).
    /// Empty = unset.
    #[serde(default)]
    pub locator: String,
}

impl Default for StationConfig {
    fn default() -> Self {
        Self {
            mycall: vec!["N0CALL".into()],
            zone: "UTC".into(),
            time_offset_minutes: 0,
            locator: String::new(),
        }
    }
}

/// Serial / USB-TNC attachment — legacy `SER*`, `FASTPOLL`, `MULTIPORT`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialConfig {
    /// Modern device path (`COM3`, `/dev/ttyUSB0`, …).
    pub port: String,
    /// USB product seen on `port` last time. Empty if the OS gave none.
    /// A later start reuses `port` only while this still matches.
    pub port_detail: String,
    /// Legacy `SERBAUD`.
    pub baud: u32,
    /// Legacy `SERNR` (COM index); informational when `port` is explicit.
    pub com_number: u32,
    /// Legacy `SERPORT` IO base (e.g. `0x3F8`); unused for USB-serial.
    pub io_port: Option<u32>,
    /// Legacy `SERIRQ`; unused for USB-serial.
    pub irq: Option<u32>,
    pub fast_poll: String,
    pub multiport: String,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port: "COM3".into(),
            port_detail: String::new(),
            baud: 9600,
            com_number: 3,
            io_port: None,
            irq: None,
            fast_poll: "auto".into(),
            multiport: "auto".into(),
        }
    }
}

impl SerialConfig {
    /// Hostmode poll sleep from legacy `FASTPOLL` (`on` / `off` / `auto`).
    pub fn poll_interval_ms(&self) -> u64 {
        match self.fast_poll.to_ascii_lowercase().as_str() {
            "on" | "1" | "true" | "yes" => 25,
            "off" | "0" | "false" | "no" => 150,
            _ => 50, // auto
        }
    }
}

/// Session / UI behaviour — channels, spy, remote, bells, …
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub channels: u32,
    /// `MAXSPY`. How many spy windows the session bar shows. GP DOS allows 1..=8.
    pub max_spy: u32,
    /// `CONNECTBELL`. Times `CONNECT.WAV` plays on connect. `0` is off and
    /// also skips `DISCONECT.WAV`. `2`..=`4` play again as soon as the last ends.
    pub connect_bell: u32,
    /// `RXCLICK`. Times `NEWLINE.WAV` plays for a channel line. `0` is off.
    /// `2` is the Graphic Packet default: twice, one second apart.
    pub rx_click: u32,
    pub screensave_minutes: u32,
    pub popup_time_seconds: u32,
    pub popup_enable_keyboard: bool,
    pub insert_mon_line: bool,
    pub questions: bool,
    pub backup: bool,
    /// DOS `Zoom`. `1` is off (JetBrains Mono Regular). `2` is on (Bold).
    pub zoom: u32,
    /// Monospace size for Monitor, channel, BBS-Menu, and Box-List. Default 14.
    #[serde(default = "default_pad_font_size")]
    pub pad_font_size: u32,
    pub prompt: String,
    pub noname: String,
    /// ISO 639-1 UI / remote-help language (`en`, `de`, …). Legacy
    /// `remote_language` / DOS tokens are migrated on load.
    #[serde(default = "default_language", alias = "remote_language")]
    pub language: String,
    /// Icon library file name. Empty or `ICONS.GP` is `data/ICONS.GP`.
    /// Any other name is a file in `data/ICONS`.
    #[serde(default)]
    pub icon_file: String,
    /// `LOG=ON` — append a line to `<base call>.GPL` when a QSO ends.
    pub log: bool,
    /// `UTC=` minutes added to the PC clock for the journal.
    pub utc_offset_min: i32,
    /// `QRG=` text, at most 7 characters (`430.625` or `70cm`).
    pub qrg_text: String,
    /// Compose line length from the setup menu.
    pub input_columns: u32,
    /// Editor "Word wrapping" value. The editor does not hard-wrap yet.
    pub editor_wrap: u32,
    /// Copy the previous file to `name.BAK` before an editor save.
    pub editor_bak: bool,
    /// BBS-Menu reference channel.
    pub bbs_channel: u32,
    /// Box-List reference channel.
    pub list_channel: u32,
    /// Box-List file drawer name.
    pub file_drawer: String,
    pub security: bool,
    pub remote: Vec<String>,
    pub no_remote: Vec<String>,
    pub auto_spy: Vec<String>,
    pub no_auto_spy: Vec<String>,
    pub auto_spy_minimum_save: u32,
    /// MHeard Auto Clean. Stations last heard more than this many days ago
    /// are removed about once an hour. `0` disables. Maximum 365.
    #[serde(default)]
    pub mheard_clean_days: u32,
    /// Channel and monitor: apply ESC `[0m` / `[1m` / `[30m`–`[37m` in a payload.
    /// Background codes are always removed. `false` strips every colour sequence.
    #[serde(default = "default_allow_color_ansi")]
    pub allow_color_ansi: bool,
    /// Session-bar button state (QRG, Echo, ScrLock, …).
    pub chrome: SessionChromeConfig,
    /// Spy window callsigns (`1..=10`); empty string = slot inactive.
    #[serde(default = "default_spy_calls")]
    pub spy_calls: Vec<String>,
}

fn default_spy_calls() -> Vec<String> {
    vec![String::new(); 10]
}

fn default_language() -> String {
    "en".into()
}

fn default_pad_font_size() -> u32 {
    14
}

/// Monitor, channel, BBS-Menu, and Box-List size. `0` is the default.
pub fn clamp_pad_font(size: u32) -> u32 {
    let size = if size == 0 {
        default_pad_font_size()
    } else {
        size
    };
    size.clamp(10, 28)
}

fn default_allow_color_ansi() -> bool {
    true
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            channels: 10,
            max_spy: 1,
            connect_bell: 1,
            rx_click: 2,
            screensave_minutes: 0,
            popup_time_seconds: 3,
            popup_enable_keyboard: false,
            insert_mon_line: false,
            questions: false,
            backup: true,
            zoom: 1,
            pad_font_size: default_pad_font_size(),
            prompt: "%c to %y>".into(),
            noname: "%c".into(),
            language: default_language(),
            icon_file: String::new(),
            log: false,
            utc_offset_min: 0,
            qrg_text: String::new(),
            input_columns: 80,
            editor_wrap: 255,
            editor_bak: false,
            bbs_channel: 1,
            list_channel: 1,
            file_drawer: String::new(),
            security: false,
            remote: Vec::new(),
            no_remote: Vec::new(),
            auto_spy: Vec::new(),
            no_auto_spy: Vec::new(),
            auto_spy_minimum_save: 512,
            mheard_clean_days: 0,
            allow_color_ansi: default_allow_color_ansi(),
            chrome: SessionChromeConfig::default(),
            spy_calls: default_spy_calls(),
        }
    }
}

/// Session-strip toggles persisted under `[session.chrome]`.
///
/// Umlaut / softscroll / echo / bell / 7+ / remote are compact `u32` bit
/// masks (see [`crate::ui::channel_bits`]). Legacy bool values still load.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionChromeConfig {
    pub qrg_mhz: f64,
    /// Connect-text path (`CT:` on the Monitor). Channels with an empty
    /// entry use this file.
    pub ctext: String,
    /// Connect text per channel. Index is the channel number. Empty uses `ctext`.
    #[serde(default)]
    pub channel_ctext: Vec<String>,
    /// Per-slot umlaut / CP437. Legacy key `extended_ascii` (bool) still loads.
    #[serde(
        default = "default_zero_bits",
        alias = "extended_ascii",
        deserialize_with = "de_umlaut",
        serialize_with = "ser_u32"
    )]
    pub umlaut: u32,
    /// Per-slot softscroll. Legacy key `smooth_scroll` (bool) still loads.
    #[serde(
        default = "default_zero_bits",
        alias = "smooth_scroll",
        deserialize_with = "de_softscroll",
        serialize_with = "ser_u32"
    )]
    pub softscroll: u32,
    /// Per-slot echo bit mask. Legacy bool still loads.
    #[serde(
        default = "default_echo_bits",
        deserialize_with = "de_echo",
        serialize_with = "ser_u32"
    )]
    pub echo: u32,
    /// Legacy per-channel echo overrides (`1`/`0`/`-1`). Applied once on load.
    #[serde(default)]
    #[serde(skip_serializing)]
    pub channel_echo: Vec<i8>,
    /// Per-channel ♪ bit mask. Legacy bool still loads.
    #[serde(
        default = "default_bell_bits",
        deserialize_with = "de_bell",
        serialize_with = "ser_u32"
    )]
    pub bell: u32,
    #[serde(
        default = "default_zero_bits",
        deserialize_with = "de_seven_plus",
        serialize_with = "ser_u32"
    )]
    pub seven_plus: u32,
    /// Per-channel Remote bit mask (not the callsign allow-list).
    #[serde(
        default = "default_zero_bits",
        deserialize_with = "de_remote",
        serialize_with = "ser_u32"
    )]
    pub remote: u32,
    pub scr_lock: bool,
    /// Clock ♫ hourly chime (ALT-G). Missing in an older file stays on.
    #[serde(default = "default_hour_gong")]
    pub hour_gong: bool,
}

fn default_hour_gong() -> bool {
    true
}

fn ser_u32<S: serde::Serializer>(v: &u32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u32(*v)
}

fn default_zero_bits() -> u32 {
    0
}

fn default_echo_bits() -> u32 {
    MASK_ECHO
}

fn default_bell_bits() -> u32 {
    MASK_BELL
}

impl Default for SessionChromeConfig {
    fn default() -> Self {
        Self {
            qrg_mhz: 27.235,
            ctext: String::new(),
            channel_ctext: Vec::new(),
            umlaut: 0,
            softscroll: 0,
            echo: default_echo_bits(),
            channel_echo: Vec::new(),
            bell: default_bell_bits(),
            seven_plus: 0,
            remote: 0,
            scr_lock: true,
            hour_gong: true,
        }
    }
}

/// Scroll-back sizes in lines (classic GP: ~80 columns).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BufferConfig {
    pub text_buffer: Vec<u32>,
    pub edit_buffer: u32,
    pub check_buffer: u32,
    pub list_buffer: u32,
    pub spy_buffer: u32,
    pub mh_buffer: u32,
    pub input_buffer: u32,
    /// Max bytes of operator text in one TX packet (classic GP: 128).
    #[serde(default = "default_packet_payload")]
    pub packet_payload: u32,
}

/// Classic GP info-field size.
fn default_packet_payload() -> u32 {
    128
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            text_buffer: vec![600, 300],
            edit_buffer: 1000,
            check_buffer: 200,
            list_buffer: 200,
            spy_buffer: 100,
            mh_buffer: 50,
            input_buffer: 200,
            packet_payload: 128,
        }
    }
}

/// Filesystem layout — legacy `*DIR` keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathConfig {
    pub user_dir: PathBuf,
    pub external_dir: PathBuf,
    pub save_dir: PathBuf,
    pub log_dir: PathBuf,
    pub spy_dir: PathBuf,
}

impl Default for PathConfig {
    fn default() -> Self {
        let data = PathBuf::from("data");
        Self {
            user_dir: data.join("USER"),
            external_dir: data.join("EXTERNAL"),
            save_dir: data.join("SAVE"),
            log_dir: data.join("LOG"),
            spy_dir: data.join("SPY"),
        }
    }
}

/// Hostmode dialect and startup/shutdown command lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TncConfig {
    /// Legacy `TNCINI` rows (including the GP pseudo-command `DateTime`).
    pub init: Vec<String>,
    /// Legacy `TNCDEI` rows.
    pub deinit: Vec<String>,
    /// Legacy `TNCDOS` rows (applied around an external shell).
    pub dos_shell: Vec<String>,
    /// `"wa8ded"` / `"jhost"` (TNC2 TheFirmware), `"tfpcx"`, `"kiss"`, or `"baycom"` (hardware unavailable).
    pub protocol: String,
    /// KISSINIT-style file (default `KISS.INI`) — used when `protocol = "kiss"`.
    pub kiss_ini: String,
    /// Section name inside `kiss_ini` (e.g. `TM-D700`, `TS2000_9600`).
    pub kiss_section: String,
    /// Delay after each character when sending KISS.INI (ms). Classic default 30.
    pub kiss_char_delay_ms: u64,
    /// Delay after each CR line (ms). Classic default 300.
    pub kiss_line_delay_ms: u64,
    /// Settle delay after the section (ms). Classic `-D` exit time; Kenwood often 5000.
    pub kiss_exit_delay_ms: u64,
    /// WA8DED open/close INI (`[open]` / `[status]` / `[close]`). Empty → use `init`/`deinit` vectors.
    pub wa8ded_ini: String,
    /// Pause after each WA8DED.INI hostmode command while waiting for a reply (ms).
    pub wa8ded_line_delay_ms: u64,
    /// Monitor flags sent at connect (`M IUSC` / `%M` in INI). Classic default IUSC.
    pub monitor_mode: String,
    /// KISS.INI section used on shutdown (default `close`).
    pub kiss_close_section: String,
    /// KISS wire: `"serial"` (Kenwood COM) or `"tcp"` (Soundmodem / Direwolf).
    pub kiss_transport: String,
    /// TCP KISS host (only when `kiss_transport = "tcp"`).
    pub kiss_tcp_host: String,
    /// TCP KISS port — UZ7HO Soundmodem often `8100`, Direwolf `8001`.
    pub kiss_tcp_port: u16,
    /// KISS radio port for monitor/unproto and modem parameters (0..15).
    pub kiss_port: u8,
    /// KISS radio port per terminal channel, starting at channel 1.
    /// Missing entries use `kiss_port`. Hostmode adapters currently require port 0.
    pub channel_ports: Vec<u8>,
    /// Filled at load from `[station].mycall` for the timing banner (not in TOML).
    #[serde(skip)]
    pub runtime_mycall: String,
    /// Station calls TFPCX must not write into the known-host file (not in TOML).
    #[serde(skip)]
    pub runtime_own_calls: Vec<String>,
    /// TFPCX hardware links. Empty unless protocol is `tfpcx`. At most three are used.
    pub links: Vec<TncLink>,
}

/// One modem behind the TFPCX switch.
///
/// `protocol` is `off`, `kiss`, or `wa8ded`. A WA8DED link owns one serial port.
/// A KISS link owns the logical ports in `ports` (hardware order is 0, 1, …).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TncLink {
    pub protocol: String,
    pub ports: Vec<u8>,
    /// KISS only: `serial` or `tcp`.
    pub transport: String,
    pub serial_port: String,
    pub baud: u32,
    pub tcp_host: String,
    pub tcp_port: u16,
}

impl Default for TncConfig {
    fn default() -> Self {
        Self {
            init: vec!["DateTime".into(), "@V0".into(), "K 2".into(), "M %M".into()],
            deinit: vec!["MN".into(), "U1Away!".into(), "K2".into(), "JHOST0".into()],
            dos_shell: vec!["MN".into()],
            protocol: "wa8ded".into(),
            kiss_ini: "data/KISS.INI".into(),
            kiss_section: "TM-D710".into(),
            kiss_char_delay_ms: 30,
            kiss_line_delay_ms: 300,
            kiss_exit_delay_ms: 5000,
            wa8ded_ini: "data/WA8DED.INI".into(),
            wa8ded_line_delay_ms: 300,
            monitor_mode: "IUSC".into(),
            kiss_close_section: "close".into(),
            kiss_transport: "serial".into(),
            kiss_tcp_host: "127.0.0.1".into(),
            kiss_tcp_port: 8100,
            kiss_port: 0,
            channel_ports: Vec::new(),
            runtime_mycall: String::new(),
            runtime_own_calls: Vec::new(),
            links: Vec::new(),
        }
    }
}

impl Default for TncLink {
    fn default() -> Self {
        Self {
            protocol: "off".into(),
            ports: Vec::new(),
            transport: "serial".into(),
            serial_port: String::new(),
            baud: 9600,
            tcp_host: "127.0.0.1".into(),
            tcp_port: 8100,
        }
    }
}

impl TncLink {
    pub fn is_enabled(&self) -> bool {
        !matches!(
            self.protocol.trim().to_ascii_lowercase().as_str(),
            "" | "off" | "none"
        )
    }

    pub fn is_wa8ded(&self) -> bool {
        self.protocol.trim().eq_ignore_ascii_case("wa8ded")
    }

    pub fn is_kiss(&self) -> bool {
        self.protocol.trim().eq_ignore_ascii_case("kiss")
    }

    pub fn uses_tcp(&self) -> bool {
        self.is_kiss() && self.transport.trim().eq_ignore_ascii_case("tcp")
    }
}

impl TncConfig {
    pub fn channel_port(&self, channel: u8) -> u8 {
        channel
            .checked_sub(1)
            .and_then(|i| self.channel_ports.get(usize::from(i)).copied())
            .unwrap_or(self.kiss_port)
    }

    pub fn protocol_kind(&self) -> HostProtocol {
        match self.protocol.trim().to_ascii_lowercase().as_str() {
            "tfpcx" => HostProtocol::Tfpcx,
            "kiss" => HostProtocol::Kiss,
            "baycom" => HostProtocol::Baycom,
            // `wa8ded`, `jhost`, `hostmode`, or anything else → WA8DED.
            _ => HostProtocol::Wa8ded,
        }
    }

    /// True when KISS should use a TCP socket (Soundmodem, Direwolf, …).
    pub fn kiss_use_tcp(&self) -> bool {
        self.protocol_kind() == HostProtocol::Kiss
            && self.kiss_transport.eq_ignore_ascii_case("tcp")
    }

    /// `host:port` for TCP KISS status / reconnect.
    pub fn kiss_tcp_addr(&self) -> String {
        format!("{}:{}", self.kiss_tcp_host.trim(), self.kiss_tcp_port)
    }

    /// Highest radio port a TFPCX link can use. Ports are `0..=TFPCX_MAX_PORT`.
    pub const TFPCX_MAX_PORT: u8 = 11;

    /// Enabled TFPCX links, in order. Ports are exclusive. At most three.
    pub fn validated_links(&self) -> Result<Vec<TncLink>, String> {
        let active: Vec<TncLink> = self
            .links
            .iter()
            .filter(|link| link.is_enabled())
            .cloned()
            .collect();
        if active.is_empty() {
            return Err("Choose at least one TFPCX link".into());
        }
        if active.len() > 3 {
            return Err("TFPCX allows at most 3 links".into());
        }
        let mut used = Vec::new();
        for (index, link) in active.iter().enumerate() {
            check_link(index + 1, link, &mut used)?;
        }
        Ok(active)
    }

    /// Logical ports owned by enabled KISS links.
    pub fn kiss_logical_ports(&self) -> Vec<u8> {
        self.validated_links()
            .unwrap_or_default()
            .into_iter()
            .filter(|link| link.is_kiss())
            .flat_map(|link| link.ports)
            .collect()
    }

    /// The single WA8DED logical port, when that link is enabled.
    pub fn wa8ded_logical_port(&self) -> Option<u8> {
        self.validated_links()
            .ok()
            .and_then(|links| links.into_iter().find(|link| link.is_wa8ded()))
            .and_then(|link| link.ports.first().copied())
    }
}

fn check_link(slot: usize, link: &TncLink, used: &mut Vec<u8>) -> Result<(), String> {
    if link.is_wa8ded() {
        if link.uses_tcp() || link.transport.trim().eq_ignore_ascii_case("tcp") {
            return Err(format!("Link {slot}: WA8DED is serial only"));
        }
        if link.ports.len() != 1 {
            return Err(format!("Link {slot}: WA8DED uses one port"));
        }
        check_serial(slot, link)?;
    } else if link.is_kiss() {
        if link.ports.is_empty() {
            return Err(format!("Link {slot}: choose at least one KISS port"));
        }
        if link.uses_tcp() {
            if link.tcp_host.trim().is_empty() {
                return Err(format!("Link {slot}: enter the IP address"));
            }
            if link.tcp_port == 0 {
                return Err(format!("Link {slot}: TCP port needs a number (1–65535)"));
            }
        } else {
            check_serial(slot, link)?;
        }
    } else {
        return Err(format!("Link {slot}: protocol must be KISS or WA8DED"));
    }
    for port in &link.ports {
        if *port > TncConfig::TFPCX_MAX_PORT {
            return Err(format!(
                "Link {slot}: port {port} is outside 0–{}",
                TncConfig::TFPCX_MAX_PORT
            ));
        }
        if used.contains(port) {
            return Err(format!("Link {slot}: port {port} is already used"));
        }
        used.push(*port);
    }
    Ok(())
}

fn check_serial(slot: usize, link: &TncLink) -> Result<(), String> {
    if link.serial_port.trim().is_empty() {
        return Err(format!("Link {slot}: choose a serial port"));
    }
    if link.baud == 0 {
        return Err(format!("Link {slot}: choose a baud rate"));
    }
    Ok(())
}

/// Which modem driver to construct at startup (only one is loaded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProtocol {
    /// WA8DED TNC2 / NORD><LINK TheFirmware hostmode (`JHOST1` / `JHOST0`).
    Wa8ded,
    /// TFPCX switch in front of the KISS and WA8DED drivers.
    Tfpcx,
    /// TAPR KISS over serial or TCP (`FEND` / `FESC`).
    Kiss,
    /// Experimental FT232R direct-USB BayCom ser12 backend (1200 bit/s).
    Baycom,
}

impl HostProtocol {
    pub fn uses_software_ax25(self) -> bool {
        matches!(self, Self::Kiss | Self::Baycom)
    }
}

impl TncConfig {
    /// KISS or BayCom, or TFPCX with at least one KISS modem.
    /// Beacons and monitor text are AX.25 UI frames on that path.
    pub fn runs_kiss_ax25(&self) -> bool {
        match self.protocol_kind() {
            HostProtocol::Kiss | HostProtocol::Baycom => true,
            HostProtocol::Tfpcx => self
                .links
                .iter()
                .any(|link| link.is_enabled() && link.is_kiss()),
            HostProtocol::Wa8ded => false,
        }
    }

    /// A WA8DED TNC is on this station, either alone or as one TFPCX modem.
    pub fn has_wa8ded(&self) -> bool {
        match self.protocol_kind() {
            HostProtocol::Wa8ded => true,
            HostProtocol::Tfpcx => self.wa8ded_logical_port().is_some(),
            _ => false,
        }
    }

    /// Port used for a beacon or monitor UI frame.
    /// TFPCX uses `kiss_port` when that radio is one of its KISS ports.
    pub fn kiss_tx_port(&self) -> u8 {
        if self.protocol_kind() != HostProtocol::Tfpcx {
            return self.kiss_port;
        }
        let ports = self.kiss_logical_ports();
        if ports.contains(&self.kiss_port) {
            self.kiss_port
        } else {
            ports.first().copied().unwrap_or(self.kiss_port)
        }
    }
}

fn default_white_name() -> String {
    "White".into()
}

fn default_orange_name() -> String {
    "Orange".into()
}

/// Longest line the beacon text box accepts.
pub const BEACON_TEXT_MAX: usize = 81;
/// Once a day. Zero minutes means the beacon is off.
pub const BEACON_MINUTES_MAX: u32 = 24 * 60;
/// AX.25 information field. A beacon file is cut to this many characters.
pub const BEACON_FILE_MAX: usize = 256;

/// Monitor beacon (setup menu). `period_minutes = 0` is off.
///
/// `source` is `text` or `file`. Text mode stays off while `text` is empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BeaconConfig {
    pub address: String,
    /// `text` sends the typed line. `file` sends the contents of `file`.
    #[serde(default = "default_beacon_source")]
    pub source: String,
    pub text: String,
    /// Path from the file browser. Empty until a file is chosen.
    #[serde(default)]
    pub file: String,
    pub period_minutes: u32,
}

fn default_beacon_source() -> String {
    "text".into()
}

impl Default for BeaconConfig {
    fn default() -> Self {
        Self {
            address: "CQ".into(),
            source: default_beacon_source(),
            text: String::new(),
            file: String::new(),
            period_minutes: 0,
        }
    }
}

impl BeaconConfig {
    pub fn uses_file(&self) -> bool {
        self.source.eq_ignore_ascii_case("file")
    }

    pub fn set_uses_file(&mut self, file: bool) {
        self.source = if file {
            "file".into()
        } else {
            default_beacon_source()
        };
    }

    /// A period is set and the chosen source has something to send.
    /// Does not open the file.
    pub fn armed(&self) -> bool {
        if self.period_minutes == 0 {
            return false;
        }
        if self.uses_file() {
            !self.file.trim().is_empty()
        } else {
            !self.text.trim().is_empty()
        }
    }

    /// Body of the next UI frame.
    ///
    /// `Ok(None)` stays quiet: no period, text mode with no text, or an empty
    /// file. `Err` means the chosen file could not be read.
    pub fn body(&self) -> Result<Option<String>, String> {
        if self.period_minutes == 0 {
            return Ok(None);
        }
        self.body_for_transmit()
    }

    pub(crate) fn body_for_transmit(&self) -> Result<Option<String>, String> {
        if self.uses_file() {
            let path = self.file.trim();
            if path.is_empty() {
                return Ok(None);
            }
            let bytes =
                std::fs::read(path).map_err(|e| format!("Beacon file cannot be read ({e})"))?;
            let text = String::from_utf8_lossy(&bytes);
            let flat = text
                .trim_start_matches('\u{feff}')
                .replace("\r\n", " ")
                .replace(['\n', '\r'], " ");
            let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
            if flat.is_empty() {
                return Ok(None);
            }
            return Ok(Some(clip_beacon(&flat, BEACON_FILE_MAX)));
        }
        let text = clip_beacon(self.text.trim(), BEACON_TEXT_MAX);
        if text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }
}

pub fn clip_beacon(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Minutes typed in the beacon box. Empty is off. Above once a day clamps.
pub fn beacon_minutes(text: &str) -> u32 {
    text.trim()
        .parse::<u32>()
        .unwrap_or(0)
        .min(BEACON_MINUTES_MAX)
}

/// Classic VGA colour attribute names (kept as strings for theme migration).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorConfig {
    pub clock_background: String,
    pub input_foreground: String,
    pub input_background: String,
    #[serde(default = "default_white_name")]
    pub input_invers_attr: String,
    pub monitor_foreground: String,
    pub monitor_background: String,
    pub connect_foreground: String,
    pub connect_background: String,
    pub state_background: String,
    pub state_foreground: String,
    #[serde(default = "default_white_name")]
    pub state_invers_attr: String,
    pub menu_background: String,
    pub menu_foreground: String,
    pub menu_invers_attr: String,
    pub border_attr: String,
    pub connect_invers_attr: String,
    pub monitor_invers_attr: String,
    /// Monitor warning and info lines (TNC, COM port, errors, "Local call set to").
    #[serde(default = "default_orange_name")]
    pub monitor_infoerror: String,
    pub editor_foreground: String,
    pub editor_background: String,
    pub check_foreground: String,
    pub check_background: String,
    pub check_invers_attr: String,
    pub editor_invers_attr: String,
    pub list_background: String,
    pub list_foreground: String,
    pub list_invers_attr: String,
    pub mh_background: String,
    pub mh_foreground: String,
    pub mh_invers_attr: String,
    pub spy_background: String,
    pub spy_foreground: String,
    pub spy_invers_attr: String,
    /// Map pane sea / background.
    #[serde(default = "default_map_background")]
    pub map_background: String,
    /// Map country outlines and labels.
    #[serde(default = "default_map_foreground")]
    pub map_foreground: String,
    /// Maidenhead grid lines.
    #[serde(default = "default_map_grid")]
    pub map_grid: String,
    /// Own QTH marker.
    #[serde(default = "default_map_qth")]
    pub map_qth: String,
    /// Heard-station markers.
    #[serde(default = "default_map_heard")]
    pub map_heard: String,
}

fn default_map_background() -> String {
    // Sea — same VGA Blue as the Monitor pad.
    "Blue".into()
}
fn default_map_foreground() -> String {
    // Land fill — same VGA DarkGreen as the clock face.
    "DarkGreen".into()
}
fn default_map_grid() -> String {
    // Field/square labels need to read on blue sea + green land.
    "Orange".into()
}
fn default_map_qth() -> String {
    "Red".into()
}
fn default_map_heard() -> String {
    "Green".into()
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            clock_background: "DarkGreen".into(),
            input_foreground: "White".into(),
            input_background: "LightBlue".into(),
            input_invers_attr: "White".into(),
            monitor_foreground: "Yellow".into(),
            monitor_background: "Blue".into(),
            connect_foreground: "White".into(),
            connect_background: "DarkGray".into(),
            state_background: "Magenta".into(),
            state_foreground: "White".into(),
            state_invers_attr: "White".into(),
            menu_background: "Red".into(),
            menu_foreground: "LightGray".into(),
            menu_invers_attr: "White".into(),
            border_attr: "White".into(),
            connect_invers_attr: "LightGreen".into(),
            monitor_invers_attr: "LightGray".into(),
            monitor_infoerror: "Orange".into(),
            editor_foreground: "White".into(),
            editor_background: "Blue".into(),
            check_foreground: "White".into(),
            check_background: "Blue".into(),
            check_invers_attr: "Yellow".into(),
            editor_invers_attr: "LightGreen".into(),
            list_background: "Cyan".into(),
            list_foreground: "White".into(),
            list_invers_attr: "Yellow".into(),
            mh_background: "LightGray".into(),
            mh_foreground: "Black".into(),
            mh_invers_attr: "LightBlue".into(),
            spy_background: "Blue".into(),
            spy_foreground: "White".into(),
            spy_invers_attr: "LightCyan".into(),
            map_background: default_map_background(),
            map_foreground: default_map_foreground(),
            map_grid: default_map_grid(),
            map_qth: default_map_qth(),
            map_heard: default_map_heard(),
        }
    }
}

/// Native window, restored on the next start. Missing keys keep the built-in size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    /// Outer left edge, in points. Empty until the window has been shown.
    pub x: Option<f32>,
    /// Outer top edge, in points.
    pub y: Option<f32>,
    /// Inner width, in points.
    #[serde(default = "default_window_width")]
    pub width: f32,
    /// Inner height, in points.
    #[serde(default = "default_window_height")]
    pub height: f32,
    pub maximized: bool,
}

fn default_window_width() -> f32 {
    1024.0
}

fn default_window_height() -> f32 {
    820.0
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            width: default_window_width(),
            height: default_window_height(),
            maximized: false,
        }
    }
}

impl WindowConfig {
    /// Fold one frame of the native window into the saved place.
    ///
    /// A minimized or not-yet-shown window is ignored. While maximized, the
    /// restored size stays as it was and only the screen corner is updated.
    pub fn remember(
        &self,
        outer_xy: Option<(f32, f32)>,
        inner_wh: Option<(f32, f32)>,
        maximized: bool,
        minimized: bool,
    ) -> Self {
        if minimized {
            return self.clone();
        }
        let Some((w, h)) = inner_wh else {
            return self.clone();
        };
        let w = w.round();
        let h = h.round();
        if !(200.0..=16000.0).contains(&w) || !(200.0..=16000.0).contains(&h) {
            return self.clone();
        }
        let pos = outer_xy.and_then(|(x, y)| {
            let (x, y) = (x.round(), y.round());
            (x.is_finite()
                && y.is_finite()
                && (-16000.0..=16000.0).contains(&x)
                && (-16000.0..=16000.0).contains(&y))
            .then_some((x, y))
        });
        let close = |a: f32, b: f32| (a - b).abs() < 3.0;
        if maximized {
            let mut next = self.clone();
            next.maximized = true;
            if let Some((x, y)) = pos {
                let same = self
                    .x
                    .zip(self.y)
                    .is_some_and(|(ox, oy)| close(ox, x) && close(oy, y));
                if !same {
                    next.x = Some(x);
                    next.y = Some(y);
                }
            }
            return next;
        }
        let Some((x, y)) = pos else {
            return self.clone();
        };
        let unchanged = !self.maximized
            && self
                .x
                .zip(self.y)
                .is_some_and(|(ox, oy)| close(ox, x) && close(oy, y))
            && close(self.width, w)
            && close(self.height, h);
        if unchanged {
            return self.clone();
        }
        Self {
            x: Some(x),
            y: Some(y),
            width: w,
            height: h,
            maximized: false,
        }
    }
}

/// ESC/P printer attribute streams — legacy `DEFAULTPRINT` / `INVERSPRINT`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintConfig {
    pub default_print: Vec<u8>,
    pub invers_print: Vec<u8>,
}

impl Default for PrintConfig {
    fn default() -> Self {
        Self {
            default_print: vec![27, 53, 27, 70],
            invers_print: vec![27, 52, 27, 69],
        }
    }
}

// ---------------------------------------------------------------------------
// Old Config.toml files
// ---------------------------------------------------------------------------

/// A file from an older GPNG. Missing settings were filled, and values this
/// version cannot read were replaced.
struct UpgradedToml {
    cfg: Config,
    /// Dotted keys that were not in the file, with the default value.
    missing: Vec<(String, toml::Value)>,
    /// Dotted keys whose saved value was replaced with the default.
    replaced: Vec<String>,
}

/// Canonical name → key an older file may still use.
const SETTING_ALIASES: &[(&str, &str)] = &[
    ("language", "remote_language"),
    ("umlaut", "extended_ascii"),
    ("softscroll", "smooth_scroll"),
];

fn upgrade_document(text: &str) -> Result<UpgradedToml, toml::de::Error> {
    let mut doc: toml::Value = toml::from_str(text)?;
    let Some(defaults) = default_document() else {
        return Ok(UpgradedToml {
            cfg: toml::from_str(text)?,
            missing: Vec::new(),
            replaced: Vec::new(),
        });
    };
    let mut missing = Vec::new();
    fill_missing(&mut doc, &defaults, "", &mut missing);
    let mut replaced = Vec::new();
    if Config::deserialize(doc.clone()).is_err() {
        scrub_incompatible(&mut doc, &defaults, &defaults, &[], &mut replaced);
        strip_extra_keys(&mut doc, &defaults, &mut replaced);
    }
    if Config::deserialize(doc.clone()).is_err() {
        tracing::warn!(
            "config could not be repaired — built-in defaults replace the unreadable parts"
        );
        doc = defaults;
        replaced.push("(file)".into());
    }
    let cfg = Config::deserialize(doc).unwrap_or_else(|_| Config::default());
    Ok(UpgradedToml {
        cfg,
        missing,
        replaced,
    })
}

fn default_document() -> Option<toml::Value> {
    match toml::Value::try_from(Config::default()) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!("built-in defaults could not be prepared: {err}");
            None
        }
    }
}

fn alias_for(key: &str) -> Option<&'static str> {
    SETTING_ALIASES
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, alias)| *alias)
}

fn table_has(table: &toml::Table, key: &str) -> bool {
    table.contains_key(key) || alias_for(key).is_some_and(|alias| table.contains_key(alias))
}

fn fill_missing(
    user: &mut toml::Value,
    proto: &toml::Value,
    path: &str,
    missing: &mut Vec<(String, toml::Value)>,
) {
    let toml::Value::Table(proto_map) = proto else {
        return;
    };
    let proto_keys: Vec<(String, toml::Value)> = proto_map
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let toml::Value::Table(user_map) = user else {
        return;
    };
    for (key, proto_val) in proto_keys {
        let child = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        if table_has(user_map, &key) {
            if proto_val.is_table() {
                if let Some(child_val) = user_map.get_mut(&key) {
                    if child_val.is_table() {
                        fill_missing(child_val, &proto_val, &child, missing);
                    }
                }
            }
            continue;
        }
        if proto_val.is_table() {
            record_leaves(&child, &proto_val, missing);
        } else {
            missing.push((child, proto_val.clone()));
        }
        user_map.insert(key, proto_val);
    }
}

fn record_leaves(path: &str, value: &toml::Value, missing: &mut Vec<(String, toml::Value)>) {
    if let toml::Value::Table(map) = value {
        for (key, child) in map {
            let child_path = format!("{path}.{key}");
            if child.is_table() {
                record_leaves(&child_path, child, missing);
            } else {
                missing.push((child_path, child.clone()));
            }
        }
    } else {
        missing.push((path.to_string(), value.clone()));
    }
}

fn scrub_incompatible(
    doc: &mut toml::Value,
    defaults: &toml::Value,
    proto: &toml::Value,
    path: &[String],
    replaced: &mut Vec<String>,
) {
    let toml::Value::Table(proto_map) = proto else {
        return;
    };
    let keys: Vec<(String, toml::Value)> = proto_map
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for (key, proto_child) in keys {
        let mut child_path = path.to_vec();
        child_path.push(key);
        let Some(current) = get_path(doc, &child_path).cloned() else {
            continue;
        };
        if current.is_table() && proto_child.is_table() {
            scrub_incompatible(doc, defaults, &proto_child, &child_path, replaced);
            continue;
        }
        if isolated_ok(defaults, &child_path, &current) {
            continue;
        }
        if current.is_array()
            && scrub_array(doc, defaults, &child_path, &current, &proto_child, replaced)
        {
            continue;
        }
        set_path(doc, &child_path, proto_child);
        replaced.push(child_path.join("."));
    }
}

fn scrub_array(
    doc: &mut toml::Value,
    defaults: &toml::Value,
    path: &[String],
    current: &toml::Value,
    proto: &toml::Value,
    replaced: &mut Vec<String>,
) -> bool {
    let Some(items) = current.as_array() else {
        return false;
    };
    let elem_proto = proto
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .or_else(|| {
            if path.last().map(String::as_str) == Some("links") {
                toml::Value::try_from(TncLink::default()).ok()
            } else {
                None
            }
        });
    let Some(elem_proto) = elem_proto else {
        return false;
    };
    let mut fixed = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if array_item_ok(defaults, path, item) {
            fixed.push(item.clone());
            continue;
        }
        if item.is_table() && elem_proto.is_table() {
            let mut elem = elem_proto.clone();
            if let Some(user_map) = item.as_table() {
                for (key, user_val) in user_map {
                    let Some(saved) = elem.as_table().and_then(|table| table.get(key)).cloned()
                    else {
                        continue;
                    };
                    if let Some(table) = elem.as_table_mut() {
                        table.insert(key.clone(), user_val.clone());
                    }
                    if !array_item_ok(defaults, path, &elem) {
                        if let Some(table) = elem.as_table_mut() {
                            table.insert(key.clone(), saved);
                        }
                        replaced.push(format!("{}[{index}].{key}", path.join(".")));
                    }
                }
            }
            fixed.push(elem);
        } else {
            fixed.push(elem_proto.clone());
            replaced.push(format!("{}[{index}]", path.join(".")));
        }
    }
    set_path(doc, path, toml::Value::Array(fixed));
    get_path(doc, path).is_some_and(|value| isolated_ok(defaults, path, value))
}

/// Drop a saved key that is not part of the current defaults (for example a
/// field that is no longer written) when that key is why the file will not load.
fn strip_extra_keys(doc: &mut toml::Value, proto: &toml::Value, replaced: &mut Vec<String>) {
    if Config::deserialize(doc.clone()).is_ok() {
        return;
    }
    let extras = extra_key_paths(doc, proto, &[]);
    for path in &extras {
        let mut trial = doc.clone();
        remove_path(&mut trial, path);
        if Config::deserialize(trial.clone()).is_ok() {
            *doc = trial;
            replaced.push(path.join("."));
            return;
        }
    }
    if extras.is_empty() {
        return;
    }
    let mut trial = doc.clone();
    for path in &extras {
        remove_path(&mut trial, path);
    }
    if Config::deserialize(trial.clone()).is_ok() {
        *doc = trial;
        for path in extras {
            replaced.push(path.join("."));
        }
    }
}

fn extra_key_paths(doc: &toml::Value, proto: &toml::Value, path: &[String]) -> Vec<Vec<String>> {
    let Some(user_map) = get_path(doc, path).and_then(|value| value.as_table()) else {
        return Vec::new();
    };
    let Some(proto_map) = get_path(proto, path).and_then(|value| value.as_table()) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for key in user_map.keys() {
        if proto_map.contains_key(key) || SETTING_ALIASES.iter().any(|(_, alias)| alias == key) {
            continue;
        }
        let mut child = path.to_vec();
        child.push(key.clone());
        found.push(child);
    }
    for key in user_map.keys() {
        if proto_map.get(key).is_some_and(|value| value.is_table()) {
            let mut child = path.to_vec();
            child.push(key.clone());
            found.extend(extra_key_paths(doc, proto, &child));
        }
    }
    found
}

fn remove_path(doc: &mut toml::Value, path: &[String]) -> bool {
    if path.is_empty() {
        return false;
    }
    let mut cur = doc;
    for key in &path[..path.len() - 1] {
        cur = match cur.as_table_mut().and_then(|table| table.get_mut(key)) {
            Some(next) => next,
            None => return false,
        };
    }
    cur.as_table_mut()
        .is_some_and(|table| table.remove(&path[path.len() - 1]).is_some())
}

fn isolated_ok(defaults: &toml::Value, path: &[String], candidate: &toml::Value) -> bool {
    let mut doc = defaults.clone();
    set_path(&mut doc, path, candidate.clone()) && Config::deserialize(doc).is_ok()
}

fn array_item_ok(defaults: &toml::Value, path: &[String], item: &toml::Value) -> bool {
    isolated_ok(defaults, path, &toml::Value::Array(vec![item.clone()]))
}

fn get_path<'a>(doc: &'a toml::Value, path: &[String]) -> Option<&'a toml::Value> {
    let mut cur = doc;
    for key in path {
        cur = cur.as_table()?.get(key)?;
    }
    Some(cur)
}

fn set_path(doc: &mut toml::Value, path: &[String], value: toml::Value) -> bool {
    if path.is_empty() {
        return false;
    }
    let mut cur = doc;
    for key in &path[..path.len() - 1] {
        cur = match cur.as_table_mut().and_then(|table| table.get_mut(key)) {
            Some(next) => next,
            None => return false,
        };
    }
    let Some(table) = cur.as_table_mut() else {
        return false;
    };
    table.insert(path[path.len() - 1].clone(), value);
    true
}

fn insert_missing_keys(text: &str, missing: &[(String, toml::Value)]) -> String {
    let mut text = text.to_string();
    for (path, value) in missing {
        let Some(rendered) = format_toml_value(value) else {
            continue;
        };
        let (section, key) = match path.rfind('.') {
            Some(split) => (Some(&path[..split]), &path[split + 1..]),
            None => (None, path.as_str()),
        };
        text = match section {
            Some(section) => insert_key_in_section(&text, section, key, &rendered),
            None => {
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                format!("{text}{key} = {rendered}\n")
            }
        };
    }
    text
}

fn format_toml_value(value: &toml::Value) -> Option<String> {
    let mut table = toml::Table::new();
    table.insert("v".into(), value.clone());
    let text = toml::to_string(&table).ok()?;
    let line = text.lines().next()?.trim();
    line.strip_prefix("v = ").map(str::to_string)
}

fn section_name(line: &str) -> Option<&str> {
    let inner = line.trim().strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() || inner.contains('[') || inner.contains(']') {
        None
    } else {
        Some(inner)
    }
}

fn line_key(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let head = trimmed.split('=').next()?.trim();
    if head.is_empty() || head.starts_with('[') {
        None
    } else {
        Some(head)
    }
}

fn insert_key_in_section(text: &str, section: &str, key: &str, rendered: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let header_at = lines
        .iter()
        .position(|line| section_name(line).is_some_and(|name| name.eq_ignore_ascii_case(section)));
    let Some(start) = header_at else {
        if let Some((parent, rest)) = section.rsplit_once('.') {
            if lines.iter().any(|line| {
                section_name(line).is_some_and(|name| name.eq_ignore_ascii_case(parent))
            }) {
                return insert_key_in_section(text, parent, &format!("{rest}.{key}"), rendered);
            }
        }
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&format!("[{section}]\n{key} = {rendered}\n"));
        return out;
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| section_name(line).map(|_| index))
        .unwrap_or(lines.len());
    if lines[start + 1..end]
        .iter()
        .any(|line| line_key(line).is_some_and(|have| have.eq_ignore_ascii_case(key)))
    {
        return text.to_string();
    }
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index == end {
            out.push_str(&format!("{key} = {rendered}\n"));
        }
        out.push_str(line);
        out.push('\n');
    }
    if end == lines.len() {
        out.push_str(&format!("{key} = {rendered}\n"));
    }
    out
}

// ---------------------------------------------------------------------------
// Legacy parsing helpers
// ---------------------------------------------------------------------------

fn anchor_to(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn strip_legacy_comment(line: &str) -> &str {
    match line.find(';') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn split_legacy_kv(line: &str) -> Option<(&str, &str)> {
    let eq = line.find('=')?;
    Some((line[..eq].trim(), line[eq + 1..].trim()))
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_boolish(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "1" | "ON" | "YES" | "TRUE" | "ENABLE" | "ENABLED"
    )
}

fn parse_u32(value: &str, key: &str) -> Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .map_err(|e| format!("{key}: cannot parse `{value}` as u32 ({e})"))
}

fn parse_on_off(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "on" | "1" | "true" | "yes"
    )
}

fn parse_i32(value: &str, key: &str) -> Result<i32, String> {
    value
        .trim()
        .parse::<i32>()
        .map_err(|e| format!("{key}: cannot parse `{value}` as i32 ({e})"))
}

fn parse_hex_or_dec(value: &str) -> Result<u32, String> {
    let v = value.trim();
    if let Some(hex) = v
        .strip_prefix("$")
        .or_else(|| v.strip_prefix("0x"))
        .or_else(|| v.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16).map_err(|e| format!("SERPORT: invalid hex `{v}` ({e})"))
    } else {
        parse_u32(v, "SERPORT")
    }
}

fn parse_u32_list(value: &str, key: &str) -> Result<Vec<u32>, String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| parse_u32(s, key))
        .collect()
}

fn parse_u8_list(value: &str, key: &str) -> Result<Vec<u8>, String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<u8>()
                .map_err(|e| format!("{key}: cannot parse `{s}` as u8 ({e})"))
        })
        .collect()
}

#[cfg(test)]
mod load_old_config_tests {
    use super::*;

    const OLD_FILE: &str = r#"
# keep this comment
[station]
mycall = ["DL0ABC"]

[serial]
port = "COM4"
baud = 9600

[session]
channels = 4
remote_language = "DE"
remote = []

[session.chrome]
echo = false
bell = true
qrg_mhz = "70cm"

[tnc]
protocol = "kiss"
links = [{ protocol = "kiss", ports = [0], transport = "serial", serial_port = "COM9", baud = "fast", tcp_host = "127.0.0.1", tcp_port = 8001 }]
"#;

    #[test]
    fn old_config_keeps_saved_values_and_fills_new_settings() {
        let dir = std::env::temp_dir().join(format!(
            "gpng-old-cfg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Config.toml");
        std::fs::write(&path, OLD_FILE).unwrap();

        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.station.mycall, vec!["DL0ABC".to_string()]);
        assert_eq!(cfg.serial.port, "COM4");
        assert_eq!(cfg.serial.baud, 9600);
        assert_eq!(cfg.session.channels, 4);
        assert_eq!(cfg.session.language, "de");
        assert_eq!(cfg.session.pad_font_size, 14);
        assert_eq!(cfg.station.locator, "");
        assert_eq!(cfg.colors.map_background, "Blue");
        assert_eq!(cfg.colors.map_qth, "Red");
        assert_eq!(cfg.session.chrome.echo, 0);
        assert_eq!(
            cfg.session.chrome.qrg_mhz,
            SessionChromeConfig::default().qrg_mhz
        );
        assert_eq!(cfg.tnc.protocol, "kiss");
        assert_eq!(cfg.tnc.links.len(), 1);
        assert_eq!(cfg.tnc.links[0].serial_port, "COM9");
        assert_eq!(cfg.tnc.links[0].baud, TncLink::default().baud);
        assert_eq!(cfg.tnc.links[0].ports, vec![0]);

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("# keep this comment"));
        assert!(written.contains("remote_language = \"DE\""));
        assert!(written.contains("mycall = [\"DL0ABC\"]"));
        assert!(written.contains("pad_font_size = 14"));
        assert!(written.contains("map_background = \"Blue\""));
        assert!(
            !written
                .lines()
                .any(|line| line.trim().starts_with("language =")),
            "alias remote_language must not be overridden by a default language key:\n{written}"
        );

        let again = std::fs::read_to_string(&path).unwrap();
        Config::load(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), again);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod tfpcx_link_tests {
    use super::*;

    fn kiss(ports: &[u8]) -> TncLink {
        TncLink {
            protocol: "kiss".into(),
            ports: ports.to_vec(),
            transport: "serial".into(),
            serial_port: "COM3".into(),
            baud: 9600,
            ..TncLink::default()
        }
    }

    #[test]
    fn wa8ded_is_one_serial_port_and_kiss_ports_cannot_overlap() {
        let mut cfg = TncConfig::default();
        cfg.links = vec![
            kiss(&[0, 1]),
            TncLink {
                protocol: "wa8ded".into(),
                ports: vec![2],
                serial_port: "COM4".into(),
                baud: 9600,
                ..TncLink::default()
            },
        ];
        let links = cfg.validated_links().unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(cfg.wa8ded_logical_port(), Some(2));
        assert_eq!(cfg.kiss_logical_ports(), vec![0, 1]);

        cfg.links[1].ports = vec![2, 3];
        assert!(cfg.validated_links().unwrap_err().contains("one port"));

        cfg.links[1].ports = vec![2];
        cfg.links[1].transport = "tcp".into();
        assert!(cfg.validated_links().unwrap_err().contains("serial only"));

        cfg.links[1].transport = "serial".into();
        cfg.links[1].ports = vec![1];
        assert!(cfg.validated_links().unwrap_err().contains("already used"));

        cfg.links.push(kiss(&[4]));
        cfg.links.push(kiss(&[5]));
        cfg.links[1].ports = vec![3];
        assert!(cfg.validated_links().unwrap_err().contains("at most 3"));
    }

    #[test]
    fn kiss_only_tfpcx_sends_ui_frames_and_skips_the_host_identity() {
        let mut cfg = TncConfig::default();
        cfg.protocol = "tfpcx".into();
        cfg.kiss_port = 9;
        cfg.links = vec![kiss(&[0, 1]), kiss(&[2])];
        cfg.links[1].transport = "tcp".into();
        cfg.links[1].tcp_host = "127.0.0.1".into();
        cfg.links[1].tcp_port = 8200;
        assert!(cfg.runs_kiss_ax25());
        assert!(!cfg.has_wa8ded());
        assert_eq!(cfg.kiss_tx_port(), 0);
    }
}
