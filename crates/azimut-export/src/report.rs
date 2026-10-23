//! Quality report of one flight: a JSON summary for programs and agents, and a
//! self-contained HTML page for people, in English or French.
//!
//! The report gathers what a user must check before using the positions: the scan findings,
//! the base used, the fix status of each exposure, the uncertainties (median and 95th
//! percentile), the problems, and the files written. It states facts; it does not grade the
//! flight. Points that need a look are listed in `attention`.
//!
//! The page has no script and no external file. Charts are inline SVG: a plan view of the
//! cameras (status by colour and shape, flight line, scale, north, direction of the base)
//! and the 1-sigma East, North and Up of each exposure in time order. Hovering a mark shows
//! its values (SVG titles); the exposure table holds every value.

use std::fmt::Write as _;
use std::path::PathBuf;

use azimut_geo::GeoidGrid;
use azimut_io::rinex::RinexSummary;
use azimut_io::scan::{BaseCheck, Flight, Severity};
use azimut_io::time::{datetime_from_gps_seconds, GPS_UTC_OFFSET_S};
use azimut_solve::geodesy::ecef_to_geodetic;
use azimut_solve::{FlightSolution, Method};
use serde::{Deserialize, Serialize};

/// Language of the HTML page. The JSON keys and the scan messages stay in English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    Fr,
}

/// Base station used for PPK.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseInfo {
    pub file: PathBuf,
    pub marker: Option<String>,
    pub receiver: Option<String>,
    pub antenna_type: Option<String>,
    pub interval_s: Option<f64>,
    pub distance_km: Option<f64>,
    /// Base start before the first exposure and end after the last, seconds.
    pub margin_before_s: Option<f64>,
    pub margin_after_s: Option<f64>,
    /// Approximate position from the RINEX header (for the map only), degrees.
    #[serde(default)]
    pub approx_lat_lon_deg: Option<[f64; 2]>,
}

/// A scan finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFinding {
    pub severity: String,
    pub message: String,
}

/// Counts and uncertainties.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub exposures: usize,
    pub fixed: usize,
    pub float: usize,
    pub other: usize,
    pub without_position: usize,
    pub files: usize,
    pub files_positioned: usize,
    pub files_without_position: usize,
    /// PPK trajectory epochs and the share fixed, percent (PPK only).
    pub epochs: usize,
    pub epochs_fixed_pct: Option<f64>,
    /// 1-sigma East, North, Up over positioned exposures, metres.
    pub sigma_median_enu_m: Option<[f64; 3]>,
    pub sigma_p95_enu_m: Option<[f64; 3]>,
    pub exposures_without_sigma: usize,
    pub max_height_check_m: f64,
    /// GPS time of the first and last exposures, seconds since the GPS epoch.
    #[serde(default)]
    pub first_gps_s: Option<f64>,
    #[serde(default)]
    pub last_gps_s: Option<f64>,
}

/// One exposure in the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportExposure {
    pub photo: Option<u32>,
    #[serde(default)]
    pub gps_s: Option<f64>,
    pub quality: Option<String>,
    pub lat_deg: Option<f64>,
    pub lon_deg: Option<f64>,
    pub h_m: Option<f64>,
    pub orthometric_h_m: Option<f64>,
    pub sigma_enu_m: Option<[f64; 3]>,
    pub files: usize,
    pub problem: Option<String>,
}

/// What was written into the images.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagesInfo {
    /// `in_place`, `copy` or `dry_run`.
    pub mode: String,
    pub written: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Edit record (in place) or folder of the copies.
    pub location: PathBuf,
}

/// A point to check: a code, its numbers, and the English text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    /// `scan_errors`, `scan_warnings`, `without_position`, `not_fixed`,
    /// `files_without_position`, `without_sigma`, `low_fix_rate`, `relative_frame`,
    /// `height_check`.
    pub code: String,
    pub values: Vec<f64>,
    pub text: String,
}

/// Quality report of one flight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlightReport {
    pub flight: String,
    pub dir: PathBuf,
    pub azimut_version: String,
    pub method: String,
    pub frame: String,
    pub vertical_datum: Option<String>,
    pub base: Option<BaseInfo>,
    pub findings: Vec<ReportFinding>,
    pub summary: Summary,
    /// Points to look at before using the positions.
    pub attention: Vec<Note>,
    pub exposures: Vec<ReportExposure>,
    pub outputs: Vec<PathBuf>,
    pub images: Option<ImagesInfo>,
}

fn percentile(mut v: Vec<f64>, p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    Some(v[i])
}

fn note(code: &str, values: Vec<f64>, text: String) -> Note {
    Note {
        code: code.into(),
        values,
        text,
    }
}

impl Note {
    /// The note in the page's language.
    pub fn localized(&self, lang: Lang, frame: &str) -> String {
        if lang == Lang::En {
            return self.text.clone();
        }
        let v = |i: usize| self.values.get(i).copied().unwrap_or(0.0);
        let n = |i: usize| v(i) as usize;
        match self.code.as_str() {
            "scan_errors" => format!("{} erreur(s) dans le diagnostic.", n(0)),
            "scan_warnings" => format!("{} avertissement(s) dans le diagnostic.", n(0)),
            "without_position" => format!("{} des {} captures n'ont pas de position.", n(0), n(1)),
            "not_fixed" => format!(
                "{} capture(s) non fixée(s) ({} flottante(s), {} autre(s)).",
                n(0),
                n(1),
                n(2)
            ),
            "files_without_position" => format!(
                "{} fichier(s) n'ont pas pu être positionnés à leur propre heure d'exposition.",
                n(0)
            ),
            "without_sigma" => format!("{} capture(s) sans incertitude.", n(0)),
            "low_fix_rate" => format!(
                "Seulement {} % des époques de la trajectoire sont fixées.",
                dec(Lang::Fr, v(0), 1)
            ),
            "relative_frame" => {
                format!("Cadre : {frame}. Les positions sont précises entre elles, pas en absolu.")
            }
            "height_check" => format!(
                "Contrôle de hauteur du bras de levier décalé de {} m.",
                dec(Lang::Fr, v(0), 4)
            ),
            _ => self.text.clone(),
        }
    }
}

impl FlightReport {
    /// Build the report of a solution. `flight` and `base` come from the scan when available.
    pub fn build(
        sol: &FlightSolution,
        flight: Option<&Flight>,
        base: Option<(&BaseCheck, &RinexSummary)>,
        geoid: Option<&GeoidGrid>,
    ) -> Self {
        let st = &sol.stats;
        let positioned: Vec<_> = sol.exposures.iter().filter_map(|e| e.camera).collect();
        let sig = |k: usize, p: f64| {
            percentile(
                positioned
                    .iter()
                    .filter_map(|c| c.sigma_enu_m.map(|s| s[k]))
                    .collect(),
                p,
            )
        };
        let triple = |p: f64| match (sig(0, p), sig(1, p), sig(2, p)) {
            (Some(e), Some(n), Some(u)) => Some([e, n, u]),
            _ => None,
        };
        let times: Vec<f64> = sol.exposures.iter().filter_map(|e| e.gps_s).collect();
        let files = sol.exposures.iter().map(|e| e.files.len()).sum();
        let files_positioned = sol
            .exposures
            .iter()
            .flat_map(|e| &e.files)
            .filter(|f| f.camera.is_some())
            .count();
        let summary = Summary {
            exposures: st.exposures,
            fixed: st.exposures_fixed,
            float: st.exposures_float,
            other: st.exposures_other,
            without_position: st.exposures_missing,
            files,
            files_positioned,
            files_without_position: st.files_without_position,
            epochs: st.epochs,
            epochs_fixed_pct: (st.epochs > 0).then(|| 100.0 * st.epochs_fixed as f64 / st.epochs as f64),
            sigma_median_enu_m: triple(0.5),
            sigma_p95_enu_m: triple(0.95),
            exposures_without_sigma: st.exposures_without_sigma,
            max_height_check_m: st.max_height_check_m,
            first_gps_s: times.iter().copied().reduce(f64::min),
            last_gps_s: times.iter().copied().reduce(f64::max),
        };

        let method = match &sol.method {
            Method::Ppk { base_obs, .. } => format!(
                "PPK (RTKLIB-EX) against {}",
                base_obs
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            Method::RtkShift(s) => format!(
                "RTK positions shifted by E {:+.3} m, N {:+.3} m, U {:+.3} m",
                s.enu_m[0], s.enu_m[1], s.enu_m[2]
            ),
        };
        let base = base.map(|(b, r)| BaseInfo {
            file: r.path.clone(),
            marker: r.marker.clone(),
            receiver: r.receiver.clone(),
            antenna_type: r.antenna_type.clone(),
            interval_s: r.interval_s,
            distance_km: b.distance_km,
            margin_before_s: b.margin_before_s,
            margin_after_s: b.margin_after_s,
            approx_lat_lon_deg: r.approx_xyz_m.map(|xyz| {
                let (lat, lon, _) = ecef_to_geodetic(xyz);
                [lat, lon]
            }),
        });
        let findings: Vec<ReportFinding> = flight
            .map(|f| {
                f.findings
                    .iter()
                    .map(|x| ReportFinding {
                        severity: match x.severity {
                            Severity::Error => "error",
                            Severity::Warning => "warning",
                            Severity::Info => "info",
                        }
                        .into(),
                        message: x.message.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let exposures = sol
            .exposures
            .iter()
            .map(|e| ReportExposure {
                photo: e.index,
                gps_s: e.gps_s,
                quality: e.camera.map(|c| c.quality.label().to_string()),
                lat_deg: e.camera.map(|c| c.lat_deg),
                lon_deg: e.camera.map(|c| c.lon_deg),
                h_m: e.camera.map(|c| c.h_m),
                orthometric_h_m: e
                    .camera
                    .and_then(|c| geoid.and_then(|g| g.orthometric_height(c.lat_deg, c.lon_deg, c.h_m))),
                sigma_enu_m: e.camera.and_then(|c| c.sigma_enu_m),
                files: e.files.len(),
                problem: e.problem.clone(),
            })
            .collect();

        let mut attention = Vec::new();
        let count = |sev: &str| findings.iter().filter(|f| f.severity == sev).count();
        let (errors, warnings) = (count("error"), count("warning"));
        if errors > 0 {
            attention.push(note(
                "scan_errors",
                vec![errors as f64],
                format!("{errors} error(s) in the scan."),
            ));
        }
        if warnings > 0 {
            attention.push(note(
                "scan_warnings",
                vec![warnings as f64],
                format!("{warnings} warning(s) in the scan."),
            ));
        }
        if summary.without_position > 0 {
            attention.push(note(
                "without_position",
                vec![summary.without_position as f64, summary.exposures as f64],
                format!(
                    "{} of {} exposures have no position.",
                    summary.without_position, summary.exposures
                ),
            ));
        }
        if summary.float + summary.other > 0 {
            attention.push(note(
                "not_fixed",
                vec![
                    (summary.float + summary.other) as f64,
                    summary.float as f64,
                    summary.other as f64,
                ],
                format!(
                    "{} exposure(s) not fixed ({} float, {} other).",
                    summary.float + summary.other,
                    summary.float,
                    summary.other
                ),
            ));
        }
        if summary.files_without_position > 0 {
            attention.push(note(
                "files_without_position",
                vec![summary.files_without_position as f64],
                format!(
                    "{} file(s) could not be positioned at their own exposure time.",
                    summary.files_without_position
                ),
            ));
        }
        if summary.exposures_without_sigma > 0 {
            attention.push(note(
                "without_sigma",
                vec![summary.exposures_without_sigma as f64],
                format!(
                    "{} exposure(s) without an uncertainty.",
                    summary.exposures_without_sigma
                ),
            ));
        }
        if let Some(p) = summary.epochs_fixed_pct {
            if p < 90.0 {
                attention.push(note(
                    "low_fix_rate",
                    vec![p],
                    format!("Only {p:.1} % of trajectory epochs are fixed."),
                ));
            }
        }
        let lower = sol.frame.to_lowercase();
        if lower.contains("approximate") || lower.contains("not stated") {
            attention.push(note(
                "relative_frame",
                vec![],
                format!(
                    "Frame: {}. Positions are precise relative to each other, not in absolute terms.",
                    sol.frame
                ),
            ));
        }
        if summary.max_height_check_m > 0.001 {
            attention.push(note(
                "height_check",
                vec![summary.max_height_check_m],
                format!(
                    "Lever-arm height check off by {:.4} m.",
                    summary.max_height_check_m
                ),
            ));
        }

        FlightReport {
            flight: sol
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "flight".into()),
            dir: sol.dir.clone(),
            azimut_version: env!("CARGO_PKG_VERSION").into(),
            method,
            frame: sol.frame.clone(),
            vertical_datum: geoid.map(|g| g.vertical_datum().to_string()),
            base,
            findings,
            summary,
            attention,
            exposures,
            outputs: Vec::new(),
            images: None,
        }
    }

    /// Self-contained HTML page in English.
    pub fn html(&self) -> String {
        self.html_in(Lang::En)
    }

    /// Self-contained HTML page (no script, no external file).
    pub fn html_in(&self, lang: Lang) -> String {
        let t = Text::of(lang);
        let m = &self.summary;
        let mut s = String::new();
        let _ = write!(
            s,
            "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <title>{} {}</title><style>{CSS}</style></head><body><main>",
            t.code,
            t.title,
            esc(&self.flight)
        );

        // Header.
        let when = match (m.first_gps_s, m.last_gps_s) {
            (Some(a), Some(b)) => {
                let a = datetime_from_gps_seconds(a - GPS_UTC_OFFSET_S);
                let b = datetime_from_gps_seconds(b - GPS_UTC_OFFSET_S);
                format!(
                    "{} &middot; {}&ndash;{} UTC",
                    a.format("%Y-%m-%d"),
                    a.format("%H:%M"),
                    b.format("%H:%M")
                )
            }
            _ => String::new(),
        };
        let _ = write!(
            s,
            "<header><div class=\"eyebrow\">{}</div><h1>{}</h1><p class=\"when\">{when}</p>\
             <div class=\"chips\"><span class=\"chip\">{}</span><span class=\"chip\">{} {}</span>{}</div></header>",
            t.title,
            esc(&self.flight),
            esc(&localize_method(&self.method, lang)),
            t.frame,
            esc(&self.frame),
            self.vertical_datum
                .as_deref()
                .map(|v| format!("<span class=\"chip\">{} {}</span>", t.heights, esc(v)))
                .unwrap_or_default()
        );

        // Status banner: what needs a look, with icon and words.
        let errors = self.findings.iter().any(|f| f.severity == "error");
        let (class, icon, headline) = if self.attention.is_empty() {
            ("good", ICON_OK, t.nothing.to_string())
        } else if errors || m.without_position > 0 {
            ("critical", ICON_STOP, t.to_check_n(self.attention.len()))
        } else {
            ("warning", ICON_WARN, t.to_check_n(self.attention.len()))
        };
        let _ = write!(
            s,
            "<section class=\"banner {class}\"><div class=\"banner-head\">{icon}<b>{headline}</b></div>"
        );
        if !self.attention.is_empty() {
            s.push_str("<ul>");
            for a in &self.attention {
                let _ = write!(s, "<li>{}</li>", esc(&a.localized(lang, &self.frame)));
            }
            s.push_str("</ul>");
        }
        s.push_str("</section>");

        // Hero and status bar.
        let _ = write!(
            s,
            "<section class=\"card hero\"><div><div class=\"label\">{}</div>\
             <div class=\"hero-value\">{} <span>/ {}</span></div></div>{}</section>",
            t.captures_fixed,
            m.fixed,
            m.exposures,
            status_bar(m, &t)
        );

        // Tiles.
        s.push_str("<section class=\"tiles\">");
        if let Some(p) = m.epochs_fixed_pct {
            tile(
                &mut s,
                t.trajectory,
                &format!("{} %", dec(lang, p, 1)),
                &format!("{} {}", m.epochs, t.epochs),
            );
        }
        let horiz = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1]).sqrt();
        if let Some(med) = m.sigma_median_enu_m {
            tile(
                &mut s,
                t.sigma_median,
                &format!(
                    "{} &middot; {} <small>cm</small>",
                    dec(lang, 100.0 * horiz(med), 1),
                    dec(lang, 100.0 * med[2], 1)
                ),
                t.sigma_note,
            );
        }
        if let Some(p95) = m.sigma_p95_enu_m {
            tile(
                &mut s,
                t.sigma_p95,
                &format!(
                    "{} &middot; {} <small>cm</small>",
                    dec(lang, 100.0 * horiz(p95), 1),
                    dec(lang, 100.0 * p95[2], 1)
                ),
                t.sigma_note,
            );
        }
        tile(
            &mut s,
            t.files,
            &format!("{} / {}", m.files_positioned, m.files),
            t.files_note,
        );
        if let Some(b) = &self.base {
            tile(
                &mut s,
                t.base,
                &format!(
                    "{} km",
                    b.distance_km.map(|d| dec(lang, d, 1)).unwrap_or("&ndash;".into())
                ),
                &format!(
                    "{} &middot; {} {}",
                    esc(b.marker.as_deref().unwrap_or("")),
                    t.every,
                    b.interval_s
                        .map(|i| format!("{} s", dec(lang, i, 0)))
                        .unwrap_or_default()
                ),
            );
        }
        s.push_str("</section>");

        // Charts.
        let _ = write!(
            s,
            "<section class=\"grid2\"><figure class=\"card\"><figcaption><b>{}</b><span>{}</span></figcaption>{}</figure>\
             <figure class=\"card\"><figcaption><b>{}</b><span>{}</span></figcaption>{}</figure></section>",
            t.map_title,
            t.map_note,
            self.map_svg(lang, &t),
            t.sigma_title,
            t.sigma_chart_note,
            self.sigma_svg(lang, &t)
        );

        // Base.
        if let Some(b) = &self.base {
            let _ = write!(
                s,
                "<section class=\"card\"><h2>{}</h2><table class=\"kv\">",
                t.base
            );
            row(
                &mut s,
                t.file,
                &format!("<code>{}</code>", esc(&file_name(&b.file))),
            );
            row(&mut s, t.marker, &opt(&b.marker));
            row(&mut s, t.receiver, &opt(&b.receiver));
            row(&mut s, t.antenna, &opt(&b.antenna_type));
            row(
                &mut s,
                t.interval,
                &b.interval_s
                    .map(|v| format!("{} s", dec(lang, v, 1)))
                    .unwrap_or("&ndash;".into()),
            );
            row(
                &mut s,
                t.distance,
                &b.distance_km
                    .map(|v| format!("{} km", dec(lang, v, 1)))
                    .unwrap_or("&ndash;".into()),
            );
            row(&mut s, t.before, &duration(lang, b.margin_before_s));
            row(&mut s, t.after, &duration(lang, b.margin_after_s));
            s.push_str("</table></section>");
        }

        // Findings.
        if !self.findings.is_empty() {
            let _ = write!(
                s,
                "<section class=\"card\"><h2>{}</h2><ul class=\"findings\">",
                t.findings
            );
            for f in &self.findings {
                let (icon, label) = match f.severity.as_str() {
                    "error" => (ICON_STOP, t.error),
                    "warning" => (ICON_WARN, t.warning),
                    _ => (ICON_INFO, t.info),
                };
                let _ = write!(
                    s,
                    "<li class=\"{}\"><span class=\"sev\">{icon}{label}</span><span>{}</span></li>",
                    esc(&f.severity),
                    esc(&f.message)
                );
            }
            s.push_str("</ul></section>");
        }

        // Images.
        if let Some(i) = &self.images {
            let mode = match i.mode.as_str() {
                "in_place" => t.in_place,
                "copy" => t.copy,
                _ => t.dry_run,
            };
            let _ = write!(
                s,
                "<section class=\"card\"><h2>{}</h2><table class=\"kv\">",
                t.images
            );
            row(&mut s, t.mode, mode);
            row(&mut s, t.written, &i.written.to_string());
            row(&mut s, t.skipped, &i.skipped.to_string());
            row(&mut s, t.failed, &i.failed.to_string());
            row(
                &mut s,
                t.location,
                &format!("<code>{}</code>", esc(&i.location.display().to_string())),
            );
            s.push_str("</table></section>");
        }

        // Exposure table.
        let _ = write!(
            s,
            "<section class=\"card\"><h2>{}</h2><div class=\"scroll\"><table class=\"exp\"><thead><tr>\
             <th>{}</th><th>{}</th><th>{}</th><th class=\"num\">{}</th><th class=\"num\">{}</th><th class=\"num\">h (m)</th>",
            t.exposures,
            t.photo,
            t.status,
            t.time,
            t.lat,
            t.lon
        );
        if self.vertical_datum.is_some() {
            s.push_str("<th class=\"num\">H (m)</th>");
        }
        let _ = write!(
            s,
            "<th class=\"num\">&sigma; E</th><th class=\"num\">&sigma; N</th><th class=\"num\">&sigma; U</th>\
             <th class=\"num\">{}</th><th>{}</th></tr></thead><tbody>",
            t.files_short, t.problem
        );
        for e in &self.exposures {
            let q = e.quality.as_deref().unwrap_or("none");
            let time = e
                .gps_s
                .map(|g| {
                    datetime_from_gps_seconds(g - GPS_UTC_OFFSET_S)
                        .format("%H:%M:%S")
                        .to_string()
                })
                .unwrap_or_default();
            let _ = write!(
                s,
                "<tr><td>{}</td><td>{}</td><td class=\"num\">{time}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td>",
                e.photo.map(|p| p.to_string()).unwrap_or_default(),
                pill(q, &t),
                e.lat_deg.map(|v| dec(lang, v, 8)).unwrap_or_default(),
                e.lon_deg.map(|v| dec(lang, v, 8)).unwrap_or_default(),
                e.h_m.map(|v| dec(lang, v, 3)).unwrap_or_default(),
            );
            if self.vertical_datum.is_some() {
                let _ = write!(
                    s,
                    "<td class=\"num\">{}</td>",
                    e.orthometric_h_m.map(|v| dec(lang, v, 3)).unwrap_or_default()
                );
            }
            match e.sigma_enu_m {
                Some([a, b, c]) => {
                    let _ = write!(
                        s,
                        "<td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td>",
                        cm(lang, a),
                        cm(lang, b),
                        cm(lang, c)
                    );
                }
                None => s.push_str("<td></td><td></td><td></td>"),
            }
            let problem = e.problem.as_deref().unwrap_or("");
            let _ = write!(
                s,
                "<td class=\"num\">{}</td><td class=\"problem\" title=\"{}\">{}</td></tr>",
                e.files,
                esc(problem),
                esc(problem)
            );
        }
        let _ = write!(
            s,
            "</tbody></table></div><p class=\"note\">{}</p></section>",
            t.table_note
        );

        // Files written.
        if !self.outputs.is_empty() {
            let folder = self
                .outputs
                .first()
                .and_then(|p| p.parent())
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let _ = write!(
                s,
                "<section class=\"card\"><h2>{}</h2><p class=\"note\"><code>{}</code></p><ul class=\"files\">",
                t.outputs,
                esc(&folder)
            );
            for o in &self.outputs {
                let _ = write!(s, "<li><code>{}</code></li>", esc(&file_name(o)));
            }
            s.push_str("</ul></section>");
        }
        let _ = writeln!(
            s,
            "<footer>Azimut {} &middot; {}</footer></main></body></html>",
            esc(&self.azimut_version),
            t.footer
        );
        s
    }

    /// Plan view: flight line in time order, cameras by status (colour and shape), scale
    /// bar, north arrow, and the direction and distance of the base.
    fn map_svg(&self, lang: Lang, t: &Text) -> String {
        let mut pts: Vec<(&ReportExposure, f64, f64)> = self
            .exposures
            .iter()
            .filter_map(|e| Some((e, e.lat_deg?, e.lon_deg?)))
            .collect();
        if pts.is_empty() {
            return format!("<p class=\"note\">{}</p>", t.no_position);
        }
        pts.sort_by(|a, b| {
            a.0.gps_s
                .partial_cmp(&b.0.gps_s)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let lat0 = pts.iter().map(|p| p.1).sum::<f64>() / pts.len() as f64;
        let lon0 = pts.iter().map(|p| p.2).sum::<f64>() / pts.len() as f64;
        let k = 111_320.0;
        let en = |lat: f64, lon: f64| ((lon - lon0) * k * lat0.to_radians().cos(), (lat - lat0) * k);
        let xy: Vec<(f64, f64)> = pts.iter().map(|p| en(p.1, p.2)).collect();
        let span = xy.iter().map(|p| p.0.abs().max(p.1.abs())).fold(5.0f64, f64::max) * 1.1;
        let (w, h, pad) = (560.0, 420.0, 28.0);
        let scale = ((w / 2.0 - pad) / span).min((h / 2.0 - pad) / span);
        let px = |x: f64, y: f64| (w / 2.0 + x * scale, h / 2.0 - y * scale);

        let mut s = format!(
            "<svg viewBox=\"0 0 {w} {h}\" role=\"img\" aria-label=\"{}\" class=\"map\">\
             <rect class=\"plot\" x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" rx=\"8\"/>",
            t.map_title
        );
        // Flight line.
        let path: Vec<String> = xy
            .iter()
            .map(|&(x, y)| {
                let (a, b) = px(x, y);
                format!("{a:.1},{b:.1}")
            })
            .collect();
        let _ = write!(s, "<polyline class=\"track\" points=\"{}\"/>", path.join(" "));
        // Cameras, worst status drawn last.
        let rank = |q: &str| match q {
            "fixed" => 0,
            "float" => 1,
            _ => 2,
        };
        let mut order: Vec<usize> = (0..pts.len()).collect();
        order.sort_by_key(|&i| rank(pts[i].0.quality.as_deref().unwrap_or("none")));
        for i in order {
            let e = pts[i].0;
            let (cx, cy) = px(xy[i].0, xy[i].1);
            let q = e.quality.as_deref().unwrap_or("none");
            let tip = format!(
                "{} {} &middot; {}{}",
                t.photo,
                e.photo.map(|p| p.to_string()).unwrap_or_default(),
                status_word(q, t),
                e.sigma_enu_m
                    .map(|[a, b, c]| format!(
                        " &middot; &sigma; E/N/U {} / {} / {}",
                        cm(lang, a),
                        cm(lang, b),
                        cm(lang, c)
                    ))
                    .unwrap_or_default()
            );
            let _ = write!(
                s,
                "<g class=\"mark {}\"><title>{tip}</title>{}</g>",
                class_of(q),
                shape(q, cx, cy)
            );
        }
        // Scale bar: a round length near a quarter of the width.
        let target = (w * 0.25) / scale;
        let mag = 10f64.powf(target.log10().floor());
        let len = [1.0, 2.0, 5.0, 10.0]
            .iter()
            .map(|f| f * mag)
            .filter(|l| *l <= target)
            .fold(mag, f64::max);
        let (x0, y0) = (pad, h - pad + 8.0);
        let _ = write!(
            s,
            "<g class=\"scale\"><line x1=\"{x0}\" y1=\"{y0}\" x2=\"{:.1}\" y2=\"{y0}\"/>\
             <text x=\"{x0}\" y=\"{:.1}\">{} m</text></g>",
            x0 + len * scale,
            y0 - 6.0,
            dec(lang, len, if len < 1.0 { 1 } else { 0 })
        );
        // North arrow.
        let _ = write!(
            s,
            "<g class=\"north\" transform=\"translate({:.1},{:.1})\"><path d=\"M0,-12 L6,6 L0,2 L-6,6 Z\"/>\
             <text y=\"20\" text-anchor=\"middle\">N</text></g>",
            w - pad,
            pad + 6.0
        );
        // Base: marker when inside the view, else an arrow on the edge towards it.
        if let Some(b) = &self.base {
            if let Some([blat, blon]) = b.approx_lat_lon_deg {
                let (bx, by) = en(blat, blon);
                let label = format!(
                    "{} {}{}",
                    t.base,
                    esc(b.marker.as_deref().unwrap_or("")),
                    b.distance_km
                        .map(|d| format!(" &middot; {} km", dec(lang, d, 1)))
                        .unwrap_or_default()
                );
                if bx.abs() < span && by.abs() < span {
                    let (cx, cy) = px(bx, by);
                    let _ = write!(
                        s,
                        "<g class=\"base\"><path d=\"M{cx:.1},{:.1} l7,12 h-14 z\"/><text x=\"{:.1}\" y=\"{cy:.1}\">{label}</text></g>",
                        cy - 8.0,
                        cx + 10.0
                    );
                } else {
                    let ang = by.atan2(bx);
                    let r = (w / 2.0 - pad).min(h / 2.0 - pad) - 6.0;
                    let (ax, ay) = (w / 2.0 + r * ang.cos(), h / 2.0 - r * ang.sin());
                    let deg = -ang.to_degrees();
                    let anchor = if ang.cos() < 0.0 { "start" } else { "end" };
                    let tx = if ang.cos() < 0.0 { ax + 12.0 } else { ax - 12.0 };
                    let _ = write!(
                        s,
                        "<g class=\"base\"><path transform=\"translate({ax:.1},{ay:.1}) rotate({deg:.1})\" d=\"M8,0 L-6,-6 L-3,0 L-6,6 Z\"/>\
                         <text x=\"{tx:.1}\" y=\"{:.1}\" text-anchor=\"{anchor}\">{label}</text></g>",
                        ay + 4.0
                    );
                }
            }
        }
        s.push_str("</svg>");
        let _ = write!(
            s,
            "<div class=\"legend\"><span>{}{}</span><span>{}{}</span><span>{}{}</span></div>",
            legend_shape("fixed"),
            t.fixed,
            legend_shape("float"),
            t.float,
            legend_shape("single"),
            t.other
        );
        s
    }

    /// 1-sigma East, North and Up of each positioned exposure, in time order, in cm.
    fn sigma_svg(&self, lang: Lang, t: &Text) -> String {
        let mut pts: Vec<(&ReportExposure, [f64; 3])> = self
            .exposures
            .iter()
            .filter_map(|e| Some((e, e.sigma_enu_m?)))
            .collect();
        if pts.is_empty() {
            return format!("<p class=\"note\">{}</p>", t.no_sigma);
        }
        pts.sort_by(|a, b| {
            a.0.gps_s
                .partial_cmp(&b.0.gps_s)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let max_cm = pts.iter().flat_map(|p| p.1).fold(0.0f64, f64::max) * 100.0;
        // Clean axis maximum: 1, 2 or 5 times a power of ten.
        let mag = 10f64.powf(max_cm.max(0.1).log10().floor());
        let (top, steps) = [(1.0, 5), (2.0, 4), (5.0, 5), (10.0, 5)]
            .iter()
            .map(|&(f, n)| (f * mag, n))
            .find(|v| v.0 >= max_cm)
            .unwrap_or((10.0 * mag, 5));
        let step = top / steps as f64;
        let decimals = if step < 0.1 {
            2
        } else if step < 1.0 {
            1
        } else {
            0
        };
        let (w, h) = (560.0, 300.0);
        let (l, r, tp, b) = (44.0, 64.0, 16.0, 30.0);
        let n = pts.len();
        let x = |i: usize| {
            if n == 1 {
                l + (w - l - r) / 2.0
            } else {
                l + (w - l - r) * i as f64 / (n - 1) as f64
            }
        };
        let y = |v_m: f64| tp + (h - tp - b) * (1.0 - (v_m * 100.0 / top).min(1.0));
        let mut s = format!(
            "<svg viewBox=\"0 0 {w} {h}\" role=\"img\" aria-label=\"{}\" class=\"chart\">",
            t.sigma_title
        );
        for k in 0..=steps {
            let v = step * k as f64;
            let yy = y(v / 100.0);
            let _ = write!(
                s,
                "<line class=\"{}\" x1=\"{l}\" x2=\"{:.1}\" y1=\"{yy:.1}\" y2=\"{yy:.1}\"/>\
                 <text class=\"tick\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>",
                if k == 0 { "axis" } else { "grid" },
                w - r,
                l - 6.0,
                yy + 4.0,
                dec(lang, v, decimals)
            );
        }
        let _ = write!(
            s,
            "<text class=\"tick\" x=\"{l}\" y=\"{:.1}\">{}</text><text class=\"tick\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{} &rarr;</text>",
            h - 8.0,
            pts.first().and_then(|p| p.0.photo).map(|p| format!("{} {p}", t.photo)).unwrap_or_default(),
            w - r,
            h - 8.0,
            pts.last().and_then(|p| p.0.photo).map(|p| format!("{} {p}", t.photo)).unwrap_or_default()
        );
        let names = ["E", "N", "U"];
        let mut ends = Vec::new();
        for (k, name) in names.iter().enumerate() {
            let line: Vec<String> = pts
                .iter()
                .enumerate()
                .map(|(i, p)| format!("{:.1},{:.1}", x(i), y(p.1[k])))
                .collect();
            let _ = write!(s, "<polyline class=\"s{k}\" points=\"{}\"/>", line.join(" "));
            for (i, p) in pts.iter().enumerate() {
                let _ = write!(
                    s,
                    "<circle class=\"s{k}\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"4\"><title>{} {} &middot; &sigma; {} {}</title></circle>",
                    x(i),
                    y(p.1[k]),
                    t.photo,
                    p.0.photo.map(|v| v.to_string()).unwrap_or_default(),
                    name,
                    cm(lang, p.1[k])
                );
            }
            ends.push((y(pts[n - 1].1[k]), k));
        }
        // End labels, spread so they do not overlap, with leader lines.
        ends.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut placed: Vec<f64> = Vec::new();
        for &(yy, _) in &ends {
            let mut v = yy;
            if let Some(&last) = placed.last() {
                v = v.max(last + 14.0);
            }
            placed.push(v);
        }
        for ((yy, k), ly) in ends.iter().zip(&placed) {
            let x1 = x(n - 1) + 6.0;
            let _ = write!(
                s,
                "<line class=\"leader\" x1=\"{x1:.1}\" y1=\"{yy:.1}\" x2=\"{:.1}\" y2=\"{ly:.1}\"/>\
                 <text class=\"end\" x=\"{:.1}\" y=\"{:.1}\">&sigma; {}</text>",
                x1 + 10.0,
                x1 + 14.0,
                ly + 4.0,
                names[*k]
            );
        }
        s.push_str("</svg>");
        let _ = write!(
            s,
            "<div class=\"legend\"><span><i class=\"key s0\"></i>{}</span><span><i class=\"key s1\"></i>{}</span>\
             <span><i class=\"key s2\"></i>{}</span><span class=\"muted\">cm</span></div>",
            t.east, t.north, t.up
        );
        s
    }
}

/// Words of the page.
struct Text {
    code: &'static str,
    title: &'static str,
    frame: &'static str,
    heights: &'static str,
    nothing: &'static str,
    to_check: &'static str,
    captures_fixed: &'static str,
    trajectory: &'static str,
    epochs: &'static str,
    sigma_median: &'static str,
    sigma_p95: &'static str,
    sigma_note: &'static str,
    files: &'static str,
    files_note: &'static str,
    base: &'static str,
    every: &'static str,
    map_title: &'static str,
    map_note: &'static str,
    sigma_title: &'static str,
    sigma_chart_note: &'static str,
    file: &'static str,
    marker: &'static str,
    receiver: &'static str,
    antenna: &'static str,
    interval: &'static str,
    distance: &'static str,
    before: &'static str,
    after: &'static str,
    findings: &'static str,
    error: &'static str,
    warning: &'static str,
    info: &'static str,
    images: &'static str,
    mode: &'static str,
    in_place: &'static str,
    copy: &'static str,
    dry_run: &'static str,
    written: &'static str,
    skipped: &'static str,
    failed: &'static str,
    location: &'static str,
    exposures: &'static str,
    photo: &'static str,
    status: &'static str,
    time: &'static str,
    lat: &'static str,
    lon: &'static str,
    files_short: &'static str,
    problem: &'static str,
    table_note: &'static str,
    outputs: &'static str,
    footer: &'static str,
    fixed: &'static str,
    float: &'static str,
    other: &'static str,
    none: &'static str,
    east: &'static str,
    north: &'static str,
    up: &'static str,
    no_position: &'static str,
    no_sigma: &'static str,
}

impl Text {
    fn of(lang: Lang) -> Self {
        match lang {
            Lang::En => Text {
                code: "en",
                title: "Quality report",
                frame: "Frame",
                heights: "Heights",
                nothing: "Nothing flagged",
                to_check: "point(s) to check",
                captures_fixed: "Captures fixed",
                trajectory: "Trajectory fixed",
                epochs: "epochs",
                sigma_median: "Uncertainty, median",
                sigma_p95: "Uncertainty, 95th percentile",
                sigma_note: "horizontal · vertical, 1-sigma",
                files: "Files positioned",
                files_note: "each at its own exposure time",
                base: "Base",
                every: "every",
                map_title: "Camera positions",
                map_note: "North up, in flight order",
                sigma_title: "Uncertainty per capture",
                sigma_chart_note: "1-sigma East, North, Up, in flight order",
                file: "File",
                marker: "Marker",
                receiver: "Receiver",
                antenna: "Antenna",
                interval: "Interval",
                distance: "Distance to the flight",
                before: "Recording before the first capture",
                after: "Recording after the last capture",
                findings: "Scan findings",
                error: "Error",
                warning: "Warning",
                info: "Info",
                images: "Images",
                mode: "Mode",
                in_place: "in place (undo with restore-images)",
                copy: "copies, originals untouched",
                dry_run: "dry run, nothing written",
                written: "Written",
                skipped: "Skipped",
                failed: "Failed",
                location: "Record or folder",
                exposures: "Captures",
                photo: "Photo",
                status: "Status",
                time: "Time (UTC)",
                lat: "Latitude",
                lon: "Longitude",
                files_short: "Files",
                problem: "Problem",
                table_note: "Uncertainties are 1-sigma in centimetres; heights h are ellipsoidal.",
                outputs: "Files written",
                footer: "open-source PPK geotagging for DJI drones",
                fixed: "fixed",
                float: "float",
                other: "other",
                none: "no position",
                east: "East",
                north: "North",
                up: "Up",
                no_position: "No positioned capture.",
                no_sigma: "No uncertainty available.",
            },
            Lang::Fr => Text {
                code: "fr",
                title: "Rapport de qualité",
                frame: "Cadre",
                heights: "Hauteurs",
                nothing: "Rien à signaler",
                to_check: "point(s) à vérifier",
                captures_fixed: "Captures fixées",
                trajectory: "Trajectoire fixée",
                epochs: "époques",
                sigma_median: "Incertitude, médiane",
                sigma_p95: "Incertitude, 95e centile",
                sigma_note: "horizontale · verticale, 1 sigma",
                files: "Fichiers positionnés",
                files_note: "chacun à sa propre heure d'exposition",
                base: "Base",
                every: "aux",
                map_title: "Positions des caméras",
                map_note: "Nord en haut, dans l'ordre du vol",
                sigma_title: "Incertitude par capture",
                sigma_chart_note: "1 sigma Est, Nord, Haut, dans l'ordre du vol",
                file: "Fichier",
                marker: "Repère",
                receiver: "Récepteur",
                antenna: "Antenne",
                interval: "Intervalle",
                distance: "Distance au vol",
                before: "Enregistrement avant la première capture",
                after: "Enregistrement après la dernière capture",
                findings: "Diagnostic",
                error: "Erreur",
                warning: "Avertissement",
                info: "Info",
                images: "Images",
                mode: "Mode",
                in_place: "sur place (annulable avec restore-images)",
                copy: "copies, originaux intacts",
                dry_run: "essai à blanc, rien d'écrit",
                written: "Écrites",
                skipped: "Sautées",
                failed: "Échecs",
                location: "Fichier de sauvegarde ou dossier",
                exposures: "Captures",
                photo: "Photo",
                status: "État",
                time: "Heure (UTC)",
                lat: "Latitude",
                lon: "Longitude",
                files_short: "Fichiers",
                problem: "Problème",
                table_note: "Incertitudes à 1 sigma, en centimètres ; hauteurs h ellipsoïdales.",
                outputs: "Fichiers écrits",
                footer: "géoréférencement PPK libre pour drones DJI",
                fixed: "fixée",
                float: "flottante",
                other: "autre",
                none: "sans position",
                east: "Est",
                north: "Nord",
                up: "Haut",
                no_position: "Aucune capture positionnée.",
                no_sigma: "Aucune incertitude disponible.",
            },
        }
    }

    fn to_check_n(&self, n: usize) -> String {
        format!("{n} {}", self.to_check)
    }
}

fn localize_method(method: &str, lang: Lang) -> String {
    match lang {
        Lang::En => method.to_string(),
        Lang::Fr => method
            .replace("PPK (RTKLIB-EX) against", "PPK (RTKLIB-EX) contre")
            .replace("RTK positions shifted by", "Positions RTK décalées de"),
    }
}

/// Number with the language's decimal separator.
fn dec(lang: Lang, v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    match lang {
        Lang::En => s,
        Lang::Fr => s.replace('.', ","),
    }
}

/// Metres shown in centimetres.
fn cm(lang: Lang, m: f64) -> String {
    format!("{} cm", dec(lang, m * 100.0, 1))
}

fn duration(lang: Lang, s: Option<f64>) -> String {
    match s {
        None => "&ndash;".into(),
        Some(v) if v.abs() >= 3600.0 => format!("{} h", dec(lang, v / 3600.0, 1)),
        Some(v) if v.abs() >= 60.0 => format!("{} min", dec(lang, v / 60.0, 0)),
        Some(v) => format!("{} s", dec(lang, v, 0)),
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn class_of(q: &str) -> &'static str {
    match q {
        "fixed" => "fixed",
        "float" => "float",
        "none" => "none",
        _ => "other",
    }
}

fn status_word<'a>(q: &str, t: &'a Text) -> &'a str {
    match class_of(q) {
        "fixed" => t.fixed,
        "float" => t.float,
        "none" => t.none,
        _ => t.other,
    }
}

/// Status mark: circle (fixed), triangle (float), square (other), so shape carries the
/// status as well as colour.
fn shape(q: &str, x: f64, y: f64) -> String {
    match class_of(q) {
        "fixed" => format!("<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"5\"/>"),
        "float" => format!(
            "<path d=\"M{x:.1},{:.1} L{:.1},{:.1} L{:.1},{:.1} Z\"/>",
            y - 6.0,
            x + 6.0,
            y + 4.5,
            x - 6.0,
            y + 4.5
        ),
        _ => format!(
            "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"10\" height=\"10\"/>",
            x - 5.0,
            y - 5.0
        ),
    }
}

fn legend_shape(q: &str) -> String {
    format!(
        "<svg class=\"key-shape {}\" viewBox=\"0 0 14 14\" aria-hidden=\"true\">{}</svg>",
        class_of(q),
        shape(q, 7.0, 7.0)
    )
}

fn pill(q: &str, t: &Text) -> String {
    let icon = match class_of(q) {
        "fixed" => ICON_OK,
        "float" => ICON_WARN,
        _ => ICON_STOP,
    };
    format!(
        "<span class=\"pill {}\">{icon}{}</span>",
        class_of(q),
        status_word(q, t)
    )
}

/// Stacked bar of capture status, each segment labelled in the legend.
fn status_bar(m: &Summary, t: &Text) -> String {
    let total = m.exposures.max(1) as f64;
    let parts = [
        ("fixed", m.fixed, t.fixed),
        ("float", m.float, t.float),
        ("other", m.other, t.other),
        ("none", m.without_position, t.none),
    ];
    let mut bar = String::from("<div class=\"stack\" role=\"img\">");
    let mut legend = String::from("<div class=\"legend\">");
    for (class, n, label) in parts {
        if n == 0 {
            continue;
        }
        let _ = write!(
            bar,
            "<span class=\"seg {class}\" style=\"flex:{n}\" title=\"{n} {label}\"></span>"
        );
        let _ = write!(
            legend,
            "<span><i class=\"key {class}\"></i>{n} {label} <span class=\"muted\">({:.0} %)</span></span>",
            100.0 * n as f64 / total
        );
    }
    bar.push_str("</div>");
    legend.push_str("</div>");
    format!("<div class=\"hero-bar\">{bar}{legend}</div>")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn opt(v: &Option<String>) -> String {
    v.as_deref().map(esc).unwrap_or_else(|| "&ndash;".into())
}

fn row(s: &mut String, k: &str, v: &str) {
    let _ = write!(s, "<tr><th>{k}</th><td>{v}</td></tr>");
}

fn tile(s: &mut String, label: &str, value: &str, note: &str) {
    let _ = write!(
        s,
        "<div class=\"tile\"><div class=\"label\">{label}</div><div class=\"value\">{value}</div>\
         <div class=\"note\">{note}</div></div>"
    );
}

const ICON_OK: &str =
    "<svg class=\"icon\" viewBox=\"0 0 16 16\" aria-hidden=\"true\"><path d=\"M3 8.5l3 3 7-7\"/></svg>";
const ICON_WARN: &str = "<svg class=\"icon\" viewBox=\"0 0 16 16\" aria-hidden=\"true\"><path d=\"M8 2.5l6 11H2z M8 6.5v3.5 M8 11.8v.4\"/></svg>";
const ICON_STOP: &str =
    "<svg class=\"icon\" viewBox=\"0 0 16 16\" aria-hidden=\"true\"><path d=\"M4 4l8 8 M12 4l-8 8\"/></svg>";
const ICON_INFO: &str =
    "<svg class=\"icon\" viewBox=\"0 0 16 16\" aria-hidden=\"true\"><path d=\"M8 7v5 M8 4.2v.4\"/></svg>";

// Palette: status steps (good, warning, critical) and categorical slots 1-3 (blue, orange,
// aqua) from the dataviz reference palette, validated for colour-vision deficiency in both
// themes; status always comes with an icon or a shape and a word.
const CSS: &str = "\
:root{color-scheme:light;--page:#f9f9f7;--surface:#fcfcfb;--ink:#0b0b0b;--ink2:#52514e;--muted:#898781;\
--grid:#e1e0d9;--axis:#c3c2b7;--ring:rgba(11,11,11,.10);--good:#0ca30c;--good-ink:#006300;--warn:#fab219;\
--warn-ink:#8a5a00;--crit:#d03b3b;--none:#b4b3ab;--s0:#2a78d6;--s1:#eb6834;--s2:#1baf7a;\
--good-bg:rgba(12,163,12,.08);--warn-bg:rgba(250,178,25,.12);--crit-bg:rgba(208,59,59,.08)}\
@media (prefers-color-scheme:dark){:root{color-scheme:dark;--page:#0d0d0d;--surface:#1a1a19;--ink:#f0efec;\
--ink2:#c3c2b7;--muted:#898781;--grid:#2c2c2a;--axis:#383835;--ring:rgba(255,255,255,.10);--good-ink:#0ca30c;\
--warn-ink:#fab219;--none:#5c5b56;--s0:#3987e5;--s1:#d95926;--s2:#199e70;--good-bg:rgba(12,163,12,.14);\
--warn-bg:rgba(250,178,25,.12);--crit-bg:rgba(208,59,59,.16)}}\
*{box-sizing:border-box}html{background:var(--page)}body{margin:0;color:var(--ink);\
font:15px/1.5 system-ui,-apple-system,\"Segoe UI\",Roboto,sans-serif}\
main{max-width:1080px;margin:0 auto;padding:32px 16px 48px;display:flex;flex-direction:column;gap:16px}\
header .eyebrow{color:var(--muted);font-size:13px;letter-spacing:.04em;text-transform:uppercase}\
h1{margin:2px 0 0;font-size:28px;font-weight:600;overflow-wrap:anywhere}.when{margin:2px 0 10px;color:var(--ink2)}\
h2{margin:0 0 10px;font-size:16px;font-weight:600}\
.chips{display:flex;flex-wrap:wrap;gap:6px}.chip{border:1px solid var(--ring);background:var(--surface);\
border-radius:999px;padding:3px 10px;font-size:13px;color:var(--ink2)}\
.card,.tile,.banner{background:var(--surface);border:1px solid var(--ring);border-radius:12px;padding:16px}\
.banner{display:flex;flex-direction:column;gap:6px}.banner-head{display:flex;align-items:center;gap:8px;font-size:16px}\
.banner ul{margin:0;padding-left:30px;color:var(--ink2)}.banner.good{background:var(--good-bg)}\
.banner.warning{background:var(--warn-bg)}.banner.critical{background:var(--crit-bg)}\
.banner.good .icon{stroke:var(--good-ink)}.banner.warning .icon{stroke:var(--warn-ink)}.banner.critical .icon{stroke:var(--crit)}\
.icon{width:16px;height:16px;fill:none;stroke:currentColor;stroke-width:2;stroke-linecap:round;stroke-linejoin:round;flex:none}\
.hero{display:grid;grid-template-columns:auto 1fr;gap:28px;align-items:center}\
.label{color:var(--ink2);font-size:13px}.hero-value{font-size:52px;font-weight:600;line-height:1.1}\
.hero-value span{font-size:24px;color:var(--muted);font-weight:500}\
.stack{display:flex;gap:2px;height:16px;border-radius:4px;overflow:hidden;margin-bottom:10px}\
.seg{min-width:4px}.seg.fixed,.key.fixed{background:var(--good)}.seg.float,.key.float{background:var(--warn)}\
.seg.other,.key.other{background:var(--crit)}.seg.none,.key.none{background:var(--none)}\
.legend{display:flex;flex-wrap:wrap;gap:6px 16px;font-size:13px;color:var(--ink2);align-items:center}\
.legend span{display:inline-flex;align-items:center;gap:6px}.key{display:inline-block;width:10px;height:10px;border-radius:3px}\
.key.s0{background:var(--s0)}.key.s1{background:var(--s1)}.key.s2{background:var(--s2)}.muted{color:var(--muted)}\
.key-shape{width:14px;height:14px}\
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(190px,1fr));gap:12px}\
.tile .value{font-size:22px;font-weight:600;margin:2px 0}.tile .value small{font-size:12px;color:var(--muted);font-weight:500}\
.tile .note,.note{color:var(--muted);font-size:12.5px;margin:0}\
.grid2{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,460px),1fr));gap:12px}\
figure{margin:0}figcaption{display:flex;flex-direction:column;margin-bottom:8px}figcaption span{color:var(--muted);font-size:12.5px}\
svg.map,svg.chart{width:100%;height:auto;display:block}.plot{fill:var(--page);stroke:var(--grid)}\
.track{fill:none;stroke:var(--axis);stroke-width:1.5;stroke-linejoin:round}\
.mark circle,.mark path,.mark rect{stroke:var(--surface);stroke-width:2}\
.mark.fixed>*:not(title),.key-shape.fixed>*{fill:var(--good)}.mark.float>*:not(title),.key-shape.float>*{fill:var(--warn)}\
.mark.other>*:not(title),.key-shape.other>*{fill:var(--crit)}.mark:hover>*:not(title){stroke:var(--ink)}\
.scale line{stroke:var(--ink2);stroke-width:2}.scale text,.north text,.base text,.tick,.end{fill:var(--ink2);font-size:12px}\
.north path{fill:var(--ink2)}.base path{fill:var(--ink)}\
.grid{stroke:var(--grid);stroke-width:1}.axis{stroke:var(--axis);stroke-width:1}.tick{font-variant-numeric:tabular-nums}\
polyline.s0,polyline.s1,polyline.s2{fill:none;stroke-width:2;stroke-linejoin:round;stroke-linecap:round}\
polyline.s0{stroke:var(--s0)}polyline.s1{stroke:var(--s1)}polyline.s2{stroke:var(--s2)}\
circle.s0,circle.s1,circle.s2{stroke:var(--surface);stroke-width:2}circle.s0{fill:var(--s0)}circle.s1{fill:var(--s1)}\
circle.s2{fill:var(--s2)}circle.s0:hover,circle.s1:hover,circle.s2:hover{stroke:var(--ink)}\
.leader{stroke:var(--axis);stroke-width:1}\
table{border-collapse:collapse;width:100%;font-size:14px}th,td{text-align:left;padding:6px 10px;border-bottom:1px solid var(--grid)}\
th{color:var(--ink2);font-weight:500}.kv th{width:42%}.num{text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap}\
.scroll{overflow:auto;max-height:560px;border-radius:8px}.exp thead th{position:sticky;top:0;background:var(--surface);z-index:1}\
.exp td{white-space:nowrap}.exp tbody tr:hover{background:var(--page)}\
.problem{max-width:360px;overflow:hidden;text-overflow:ellipsis;color:var(--ink2)}\
.pill{display:inline-flex;align-items:center;gap:4px;font-size:12.5px;padding:1px 8px 1px 6px;border-radius:999px;border:1px solid var(--ring)}\
.pill .icon{width:12px;height:12px}.pill.fixed .icon{stroke:var(--good-ink)}.pill.float .icon{stroke:var(--warn-ink)}\
.pill.other .icon,.pill.none .icon{stroke:var(--crit)}\
.findings{list-style:none;margin:0;padding:0;display:flex;flex-direction:column;gap:6px}\
.findings li{display:grid;grid-template-columns:150px 1fr;gap:10px;align-items:start}\
.sev{display:inline-flex;align-items:center;gap:6px;font-size:13px;font-weight:500}\
.findings .error .icon{stroke:var(--crit)}.findings .warning .icon{stroke:var(--warn-ink)}.findings .info .icon{stroke:var(--muted)}\
.files{columns:2 280px;margin:6px 0 0;padding-left:18px}code{font-size:12.5px;overflow-wrap:anywhere}\
footer{color:var(--muted);font-size:12.5px;text-align:center;margin-top:8px}\
@media (max-width:640px){.hero{grid-template-columns:1fr}.findings li{grid-template-columns:1fr}.hero-value{font-size:44px}}\
@media print{html{background:#fff}.card,.tile,.banner{break-inside:avoid}.scroll{max-height:none;overflow:visible}}";
