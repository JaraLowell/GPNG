//! Configuration for the modern Graphic Packet rewrite.
//!
//! Classic GP stored settings in `CONFIG.GP` (and baked defaults into
//! `GP2862K.EXE`). This module:
//!
//! 1. Exposes a typed [`Config`] tree for `Config.toml`.
//! 2. Accepts the historical key names (`MYCALL`, `SERBAUD`, `CHANNELS`, …)
//!    when loading a legacy-style key=value file.
//! 3. Normalises both sources into one runtime structure.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
            Self::from_toml_file(path)?
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

        cfg.tnc.runtime_mycall = cfg.primary_mycall().to_string();
        cfg.normalize_spy_calls();
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
    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| ConfigError::Toml {
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
                    // If the operator only set SERNR, synthesise a modern port name.
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
                "PROMPT" => self.session.prompt = value.to_string(),
                "NONAME" => self.session.noname = value.to_string(),
                "REMOTELANGUAGE" => {
                    self.session.remote_language = value.to_string();
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
        Ok(call)
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
        let channel_echo = c
            .channel_echo
            .iter()
            .map(|code| code.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let qrg = crate::journal::format_mhz(c.qrg_mhz);
        let body = format!(
            "qrg_mhz = {qrg}\n\
             ctext = {ctext}\n\
             channel_ctext = [{channel_ctext}]\n\
             extended_ascii = {}\n\
             smooth_scroll = {}\n\
             echo = {}\n\
             channel_echo = [{channel_echo}]\n\
             bell = {}\n\
             seven_plus = {}\n\
             remote = {}\n\
             scr_lock = {}\n\
             hour_gong = {}\n",
            c.extended_ascii,
            c.smooth_scroll,
            c.echo,
            c.bell,
            c.seven_plus,
            c.remote,
            c.scr_lock,
            c.hour_gong,
        );
        upsert_toml_section(path, "session.chrome", &body)
    }

    /// Keep `//Language` across a restart (`remote_language` in Config.toml).
    pub fn persist_remote_language(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let value = toml_string(&self.session.remote_language);
        upsert_toml_key(path.as_ref(), None, "remote_language", &value)
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
        put_key(
            path,
            "session",
            "remote_language",
            &toml_string(&s.remote_language),
        )?;
        put_key(path, "session", "zoom", &s.zoom.to_string())?;
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
            ("state_background", c.state_background.as_str()),
            ("state_foreground", c.state_foreground.as_str()),
            ("state_invers_attr", c.state_invers_attr.as_str()),
            ("menu_background", c.menu_background.as_str()),
            ("menu_foreground", c.menu_foreground.as_str()),
            ("menu_invers_attr", c.menu_invers_attr.as_str()),
        ] {
            put_key(path, "colors", key, &toml_string(value))?;
        }
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
}

impl Default for StationConfig {
    fn default() -> Self {
        Self {
            mycall: vec!["N0CALL".into()],
            zone: "UTC".into(),
            time_offset_minutes: 0,
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
    pub prompt: String,
    pub noname: String,
    pub remote_language: String,
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
    /// Channel window: apply ESC `[0m` / `[1m` / `[30m`–`[37m` in a payload.
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
            prompt: "%c to %y>".into(),
            noname: "%c".into(),
            remote_language: "EN".into(),
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
    /// `äöü` extended DOS / CP437 mode.
    pub extended_ascii: bool,
    /// Smooth (stairs) vs hard (`/`) line step.
    pub smooth_scroll: bool,
    pub echo: bool,
    /// Per channel: `1` show our text, `0` hide it, `-1` follow `echo`.
    /// Index is the channel number. Missing entries follow `echo`.
    #[serde(default)]
    pub channel_echo: Vec<i8>,
    /// Session-bar bell (♪).
    pub bell: bool,
    pub seven_plus: bool,
    /// Channel Remote toggle (not the callsign allow-list).
    pub remote: bool,
    pub scr_lock: bool,
    /// Clock ♫ hourly chime (ALT-G). Missing in an older file stays on.
    #[serde(default = "default_hour_gong")]
    pub hour_gong: bool,
}

fn default_hour_gong() -> bool {
    true
}

impl Default for SessionChromeConfig {
    fn default() -> Self {
        Self {
            qrg_mhz: 27.235,
            ctext: String::new(),
            channel_ctext: Vec::new(),
            extended_ascii: false,
            smooth_scroll: false,
            echo: true,
            channel_echo: Vec::new(),
            bell: true,
            seven_plus: false,
            remote: false,
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
        Self {
            user_dir: PathBuf::from("data/USER"),
            external_dir: PathBuf::from("data/EXTERNAL"),
            save_dir: PathBuf::from("data/SAVE"),
            log_dir: PathBuf::from("data/LOG"),
            spy_dir: PathBuf::from("data/SPY"),
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
    /// `"wa8ded"` / `"jhost"`, `"tfpcx"`, or `"kiss"`.
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
    /// Filled at load from `[station].mycall` for the timing banner (not in TOML).
    #[serde(skip)]
    pub runtime_mycall: String,
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
            runtime_mycall: String::new(),
        }
    }
}

impl TncConfig {
    pub fn protocol_kind(&self) -> HostProtocol {
        match self.protocol.to_ascii_lowercase().as_str() {
            "tfpcx" => HostProtocol::Tfpcx,
            "kiss" => HostProtocol::Kiss,
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
}

/// Which modem driver to construct at startup (only one is loaded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProtocol {
    /// Classic WA8DED binary hostmode + timed poll (`JHOST1` / `JHOST0`).
    Wa8ded,
    /// TFPCX soft-TNC (BayCom-class); WA8DED-compatible framing.
    Tfpcx,
    /// TAPR KISS over serial or TCP (`FEND` / `FESC`).
    Kiss,
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
        if self.uses_file() {
            let path = self.file.trim();
            if path.is_empty() {
                return Ok(None);
            }
            let bytes =
                std::fs::read(path).map_err(|e| format!("Beacon file cannot be read ({e})"))?;
            let text = String::from_utf8_lossy(&bytes);
            let flat = text.replace("\r\n", " ").replace(['\n', '\r'], " ");
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
mod tests {
    use super::*;

    #[test]
    fn merges_core_legacy_keys() {
        let mut cfg = Config::default();
        cfg.merge_legacy_text(
            r#"
            MyCall = NL0MSK,NL0MSK-1
            SerBaud = 19200
            SerNr = 1
            Channels = 8
            TNCINI = DateTime
            TNCINI = M UISC
            SerPort = $3F8
            "#,
        )
        .unwrap();

        assert_eq!(cfg.station.mycall, vec!["NL0MSK", "NL0MSK-1"]);
        assert_eq!(cfg.serial.baud, 19200);
        assert_eq!(cfg.serial.com_number, 1);
        assert_eq!(cfg.serial.port, "COM1");
        assert_eq!(cfg.serial.io_port, Some(0x3F8));
        assert_eq!(cfg.session.channels, 8);
        assert_eq!(cfg.tnc.init, vec!["DateTime", "M UISC"]);
    }

    #[test]
    fn window_place_round_trip_and_remember() {
        let missing: Config = toml::from_str("channels = 10\n").unwrap();
        assert_eq!(missing.window, WindowConfig::default());

        let saved: WindowConfig =
            toml::from_str("x = 40\ny = 12\nwidth = 1100\nheight = 760\nmaximized = true\n")
                .unwrap();
        assert_eq!(saved.x, Some(40.0));
        assert_eq!(saved.y, Some(12.0));
        assert!((saved.width - 1100.0).abs() < f32::EPSILON);
        assert!(saved.maximized);

        let partial: WindowConfig = toml::from_str("maximized = false\n").unwrap();
        assert!((partial.width - 1024.0).abs() < f32::EPSILON);
        assert!((partial.height - 820.0).abs() < f32::EPSILON);

        let base = WindowConfig::default();
        let parked = base.remember(Some((10.0, 20.0)), Some((900.0, 700.0)), false, true);
        assert_eq!(parked, base);

        let placed = base.remember(Some((10.4, 20.6)), Some((1100.2, 760.4)), false, false);
        assert_eq!(placed.x, Some(10.0));
        assert_eq!(placed.y, Some(21.0));
        assert!((placed.width - 1100.0).abs() < f32::EPSILON);
        assert!(!placed.maximized);

        let maxed = placed.remember(Some((0.0, 0.0)), Some((1920.0, 1080.0)), true, false);
        assert!(maxed.maximized);
        assert!((maxed.width - 1100.0).abs() < f32::EPSILON);
        assert_eq!(maxed.x, Some(0.0));
    }

    #[test]
    fn radio_settings_keep_trailing_comments() {
        let path = std::env::temp_dir().join(format!("gpng-radio-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            "[serial]\n\
             port = \"COM1\" # device\n\
             baud = 9600 # SERBAUD\n\
             com_number = 1\n\
             \n\
             [tnc]\n\
             protocol = \"wa8ded\"\n\
             kiss_transport = \"serial\"\n\
             kiss_tcp_host = \"127.0.0.1\"\n\
             kiss_tcp_port = 8100 # soundmodem\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.serial.port = "COM6".into();
        cfg.serial.baud = 19200;
        cfg.serial.com_number = 6;
        cfg.tnc.protocol = "kiss".into();
        cfg.tnc.kiss_transport = "tcp".into();
        cfg.tnc.kiss_tcp_host = "192.168.1.9".into();
        cfg.tnc.kiss_tcp_port = 8001;
        cfg.persist_radio(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(text.contains("port = \"COM6\" # device"));
        assert!(text.contains("baud = 19200 # SERBAUD"));
        assert!(text.contains("com_number = 6"));
        assert!(text.contains("protocol = \"kiss\""));
        assert!(text.contains("kiss_transport = \"tcp\""));
        assert!(text.contains("kiss_tcp_host = \"192.168.1.9\""));
        assert!(text.contains("kiss_tcp_port = 8001 # soundmodem"));
    }

    #[test]
    fn old_beacon_table_stays_on_text_and_off_without_text() {
        let beacon: BeaconConfig =
            toml::from_str("address = \"CQ\"\ntext = \"\"\nperiod_minutes = 60\n").unwrap();
        assert!(!beacon.uses_file());
        assert!(beacon.file.is_empty());
        assert!(!beacon.armed());
        assert_eq!(beacon.body().unwrap(), None);
        assert_eq!(beacon_minutes("1440"), 1440);
        assert_eq!(beacon_minutes("9999"), BEACON_MINUTES_MAX);
        assert_eq!(beacon_minutes(""), 0);
    }

    #[test]
    fn beacon_sends_typed_text_or_the_file() {
        let mut beacon = BeaconConfig::default();
        beacon.period_minutes = 15;
        beacon.text = format!("{} extra", "x".repeat(81));
        let text = beacon.body().unwrap().unwrap();
        assert_eq!(text.chars().count(), BEACON_TEXT_MAX);

        let path = std::env::temp_dir().join("gpng-beacon-test.gp");
        std::fs::write(&path, "hello from file\r\n").unwrap();
        beacon.file = path.display().to_string();
        beacon.set_uses_file(true);
        assert_eq!(beacon.body().unwrap().as_deref(), Some("hello from file"));
        let _ = std::fs::remove_file(&path);

        beacon.file = path.display().to_string();
        let err = beacon.body().unwrap_err();
        assert!(err.contains("cannot be read"));
    }

    #[test]
    fn hour_gong_stays_on_unless_saved_off() {
        let missing: SessionChromeConfig = toml::from_str("scr_lock = false\n").unwrap();
        assert!(missing.hour_gong);
        let off: SessionChromeConfig = toml::from_str("hour_gong = false\n").unwrap();
        assert!(!off.hour_gong);
    }
}
