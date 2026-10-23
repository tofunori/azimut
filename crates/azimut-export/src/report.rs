//! Quality report of one flight: a JSON summary for programs and agents, and a
//! self-contained HTML page for people.
//!
//! The report gathers what a user must check before using the positions: the scan findings,
//! the base used, the fix status of each exposure, the uncertainties (median and 95th
//! percentile), the problems, and the files written. It states facts; it does not grade the
//! flight. Points that need a look are listed in `attention`.

use std::fmt::Write as _;
use std::path::PathBuf;

use azimut_geo::GeoidGrid;
use azimut_io::rinex::RinexSummary;
use azimut_io::scan::{BaseCheck, Flight, Severity};
use azimut_solve::pos::SolQuality;
use azimut_solve::{FlightSolution, Method};
use serde::{Deserialize, Serialize};

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
}

/// One exposure in the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportExposure {
    pub photo: Option<u32>,
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
    pub attention: Vec<String>,
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

fn quality_label(q: SolQuality) -> String {
    q.label().to_string()
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
                quality: e.camera.map(|c| quality_label(c.quality)),
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
        let errors = findings.iter().filter(|f| f.severity == "error").count();
        let warnings = findings.iter().filter(|f| f.severity == "warning").count();
        if errors > 0 {
            attention.push(format!("{errors} error(s) in the scan."));
        }
        if warnings > 0 {
            attention.push(format!("{warnings} warning(s) in the scan."));
        }
        if summary.without_position > 0 {
            attention.push(format!(
                "{} of {} exposures have no position.",
                summary.without_position, summary.exposures
            ));
        }
        if summary.float + summary.other > 0 {
            attention.push(format!(
                "{} exposure(s) not fixed ({} float, {} other).",
                summary.float + summary.other,
                summary.float,
                summary.other
            ));
        }
        if summary.files_without_position > 0 {
            attention.push(format!(
                "{} file(s) could not be positioned at their own exposure time.",
                summary.files_without_position
            ));
        }
        if summary.exposures_without_sigma > 0 {
            attention.push(format!(
                "{} exposure(s) without an uncertainty.",
                summary.exposures_without_sigma
            ));
        }
        if let Some(p) = summary.epochs_fixed_pct {
            if p < 90.0 {
                attention.push(format!("Only {p:.1} % of trajectory epochs are fixed."));
            }
        }
        let lower = sol.frame.to_lowercase();
        if lower.contains("approximate") || lower.contains("not stated") {
            attention.push(format!(
                "Frame: {}. Positions are precise relative to each other, not in absolute terms.",
                sol.frame
            ));
        }
        if summary.max_height_check_m > 0.001 {
            attention.push(format!(
                "Lever-arm height check off by {:.4} m.",
                summary.max_height_check_m
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

    /// Self-contained HTML page (no external files or scripts).
    pub fn html(&self) -> String {
        let mut s = String::new();
        let _ = write!(
            s,
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <title>Azimut report: {}</title><style>{CSS}</style></head><body><main>",
            esc(&self.flight)
        );
        let _ = write!(
            s,
            "<h1>{}</h1><p class=\"sub\">{} &middot; frame: {}{} &middot; Azimut {}</p>",
            esc(&self.flight),
            esc(&self.method),
            esc(&self.frame),
            self.vertical_datum
                .as_deref()
                .map(|v| format!(" &middot; heights: ellipsoidal and {}", esc(v)))
                .unwrap_or_default(),
            esc(&self.azimut_version)
        );

        let m = &self.summary;
        let fmt3 = |v: Option<[f64; 3]>| {
            v.map(|[e, n, u]| format!("{:.3} / {:.3} / {:.3} m", e, n, u))
                .unwrap_or_else(|| "&ndash;".into())
        };
        s.push_str("<section class=\"tiles\">");
        tile(&mut s, "Exposures", &m.exposures.to_string(), "");
        tile(
            &mut s,
            "Fixed",
            &m.fixed.to_string(),
            &format!(
                "{} float, {} other, {} without position",
                m.float, m.other, m.without_position
            ),
        );
        if let Some(p) = m.epochs_fixed_pct {
            tile(
                &mut s,
                "Trajectory fixed",
                &format!("{p:.1} %"),
                &format!("{} epochs", m.epochs),
            );
        }
        tile(
            &mut s,
            "Sigma E / N / U, median",
            &fmt3(m.sigma_median_enu_m),
            "1-sigma, from the solution",
        );
        tile(&mut s, "Sigma E / N / U, 95th pct", &fmt3(m.sigma_p95_enu_m), "");
        tile(
            &mut s,
            "Files positioned",
            &format!("{} / {}", m.files_positioned, m.files),
            "each file at its own exposure time",
        );
        s.push_str("</section>");

        s.push_str("<h2>To check</h2>");
        if self.attention.is_empty() {
            s.push_str("<p class=\"ok\">Nothing flagged.</p>");
        } else {
            s.push_str("<ul class=\"attention\">");
            for a in &self.attention {
                let _ = write!(s, "<li>{}</li>", esc(a));
            }
            s.push_str("</ul>");
        }

        if let Some(b) = &self.base {
            s.push_str("<h2>Base</h2><table>");
            row(&mut s, "File", &esc(&b.file.display().to_string()));
            row(&mut s, "Marker", &opt(&b.marker));
            row(&mut s, "Receiver", &opt(&b.receiver));
            row(&mut s, "Antenna", &opt(&b.antenna_type));
            row(&mut s, "Interval", &num(b.interval_s, "s", 1));
            row(&mut s, "Distance to the flight", &num(b.distance_km, "km", 1));
            row(
                &mut s,
                "Recording before the first exposure",
                &num(b.margin_before_s, "s", 0),
            );
            row(
                &mut s,
                "Recording after the last exposure",
                &num(b.margin_after_s, "s", 0),
            );
            s.push_str("</table>");
        }

        s.push_str("<h2>Camera positions</h2>");
        s.push_str(&self.map_svg());

        if !self.findings.is_empty() {
            s.push_str("<h2>Scan findings</h2><ul class=\"findings\">");
            for f in &self.findings {
                let _ = write!(
                    s,
                    "<li class=\"{}\"><b>{}</b> {}</li>",
                    esc(&f.severity),
                    esc(&f.severity),
                    esc(&f.message)
                );
            }
            s.push_str("</ul>");
        }

        if let Some(i) = &self.images {
            s.push_str("<h2>Images</h2><table>");
            row(&mut s, "Mode", &esc(&i.mode));
            row(&mut s, "Written", &i.written.to_string());
            row(&mut s, "Skipped", &i.skipped.to_string());
            row(&mut s, "Failed", &i.failed.to_string());
            row(
                &mut s,
                "Record or folder",
                &esc(&i.location.display().to_string()),
            );
            s.push_str("</table>");
        }

        s.push_str(
            "<h2>Exposures</h2><div class=\"scroll\"><table class=\"exp\"><thead><tr><th>Photo</th>\
             <th>Status</th><th>Latitude</th><th>Longitude</th><th>h (m)</th>",
        );
        if self.vertical_datum.is_some() {
            s.push_str("<th>H (m)</th>");
        }
        s.push_str("<th>&sigma; E / N / U (m)</th><th>Files</th><th>Problem</th></tr></thead><tbody>");
        for e in &self.exposures {
            let _ = write!(
                s,
                "<tr><td>{}</td><td class=\"q {}\">{}</td><td>{}</td><td>{}</td><td>{}</td>",
                e.photo.map(|p| p.to_string()).unwrap_or_default(),
                esc(e.quality.as_deref().unwrap_or("none")),
                esc(e.quality.as_deref().unwrap_or("none")),
                e.lat_deg.map(|v| format!("{v:.9}")).unwrap_or_default(),
                e.lon_deg.map(|v| format!("{v:.9}")).unwrap_or_default(),
                e.h_m.map(|v| format!("{v:.3}")).unwrap_or_default(),
            );
            if self.vertical_datum.is_some() {
                let _ = write!(
                    s,
                    "<td>{}</td>",
                    e.orthometric_h_m.map(|v| format!("{v:.3}")).unwrap_or_default()
                );
            }
            let _ = write!(
                s,
                "<td>{}</td><td>{}</td><td>{}</td></tr>",
                e.sigma_enu_m
                    .map(|[a, b, c]| format!("{a:.3} / {b:.3} / {c:.3}"))
                    .unwrap_or_default(),
                e.files,
                esc(e.problem.as_deref().unwrap_or(""))
            );
        }
        s.push_str("</tbody></table></div>");

        if !self.outputs.is_empty() {
            s.push_str("<h2>Files written</h2><ul>");
            for o in &self.outputs {
                let _ = write!(s, "<li><code>{}</code></li>", esc(&o.display().to_string()));
            }
            s.push_str("</ul>");
        }
        s.push_str("</main></body></html>\n");
        s
    }

    /// Plan view of the exposures, coloured by status, in a local East/North frame.
    fn map_svg(&self) -> String {
        let pts: Vec<(f64, f64, &str)> = self
            .exposures
            .iter()
            .filter_map(|e| Some((e.lat_deg?, e.lon_deg?, e.quality.as_deref().unwrap_or("none"))))
            .collect();
        if pts.is_empty() {
            return "<p>No positioned exposure.</p>".into();
        }
        let lat0 = pts.iter().map(|p| p.0).sum::<f64>() / pts.len() as f64;
        let lon0 = pts.iter().map(|p| p.1).sum::<f64>() / pts.len() as f64;
        let m_per_deg = 111_320.0;
        let xy: Vec<(f64, f64, &str)> = pts
            .iter()
            .map(|&(la, lo, q)| {
                (
                    (lo - lon0) * m_per_deg * lat0.to_radians().cos(),
                    (la - lat0) * m_per_deg,
                    q,
                )
            })
            .collect();
        let span = xy.iter().map(|p| p.0.abs().max(p.1.abs())).fold(1.0f64, f64::max);
        let (w, pad) = (600.0, 20.0);
        let scale = (w / 2.0 - pad) / span;
        let mut s = format!(
            "<figure><svg viewBox=\"0 0 {w} {w}\" role=\"img\" aria-label=\"Camera positions\">\
             <rect class=\"bg\" width=\"{w}\" height=\"{w}\"/>"
        );
        for (x, y, q) in &xy {
            let _ = write!(
                s,
                "<circle class=\"{}\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"4\"/>",
                esc(q),
                w / 2.0 + x * scale,
                w / 2.0 - y * scale
            );
        }
        let _ = write!(
            s,
            "</svg><figcaption>North up; the frame spans {:.0} m. \
             <span class=\"q fixed\">fixed</span> <span class=\"q float\">float</span> \
             <span class=\"q single\">other</span></figcaption></figure>",
            2.0 * span
        );
        s
    }
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

fn num(v: Option<f64>, unit: &str, decimals: usize) -> String {
    v.map(|x| format!("{x:.decimals$} {unit}"))
        .unwrap_or_else(|| "&ndash;".into())
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

const CSS: &str = "\
:root{--bg:#fbfbfa;--fg:#1d1d1b;--muted:#6b6b66;--line:#e2e1dc;--card:#ffffff;\
--fixed:#2f7d4f;--float:#c27c0e;--other:#b3362e;--accent:#2a5d8f}\
@media (prefers-color-scheme:dark){:root{--bg:#161615;--fg:#ecebe6;--muted:#a3a29b;\
--line:#33332f;--card:#1f1f1d;--fixed:#5bbf85;--float:#e6a63b;--other:#e8695f;--accent:#7fb0e0}}\
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);\
font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\
main{max-width:1000px;margin:0 auto;padding:24px 16px 48px}h1{margin:0 0 4px;font-size:24px}\
h2{margin:32px 0 8px;font-size:17px}.sub{color:var(--muted);margin:0}\
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:10px;margin-top:20px}\
.tile{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:12px}\
.label{color:var(--muted);font-size:12px}.value{font-size:20px;font-variant-numeric:tabular-nums}\
.note{color:var(--muted);font-size:12px}table{border-collapse:collapse;width:100%}\
th,td{text-align:left;padding:5px 8px;border-bottom:1px solid var(--line);font-variant-numeric:tabular-nums}\
th{color:var(--muted);font-weight:500}.scroll{overflow-x:auto}.exp td{white-space:nowrap}\
.attention li{margin:2px 0}.ok{color:var(--fixed)}.findings .error b{color:var(--other)}\
.findings .warning b{color:var(--float)}.findings .info b{color:var(--muted)}\
.q{color:var(--other)}.q.fixed{color:var(--fixed)}.q.float{color:var(--float)}\
svg{width:100%;max-width:600px;height:auto;display:block}.bg{fill:var(--card);stroke:var(--line)}\
circle{fill:var(--other)}circle.fixed{fill:var(--fixed)}circle.float{fill:var(--float)}\
figcaption{color:var(--muted);font-size:13px}code{font-size:13px}";
