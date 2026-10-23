//! Running RTKLIB-EX command-line programs (`convbin`, `rnx2rtkp`).
//!
//! Azimut does not ship RTKLIB. Point it at an RTKLIB-EX build with `--rtklib DIR`, the
//! `AZIMUT_RTKLIB_DIR` environment variable, or put the programs on the `PATH`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::NaiveDateTime;

use crate::SolveError;

pub const ENV_DIR: &str = "AZIMUT_RTKLIB_DIR";

/// Paths of the RTKLIB programs Azimut uses.
#[derive(Debug, Clone)]
pub struct Rtklib {
    pub convbin: PathBuf,
    pub rnx2rtkp: PathBuf,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn find_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(exe(name));
    p.is_file().then_some(p)
}

impl Rtklib {
    /// Find the programs in `dir`, then `$AZIMUT_RTKLIB_DIR`, then next to the running
    /// program (and in its `rtklib` subfolder, as in the release archives), then the `PATH`.
    pub fn locate(dir: Option<&Path>) -> Result<Self, SolveError> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(d) = dir {
            dirs.push(d.to_path_buf());
        }
        if let Some(d) = std::env::var_os(ENV_DIR) {
            dirs.push(PathBuf::from(d));
        }
        if let Some(own) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
        {
            dirs.push(own.join("rtklib"));
            dirs.push(own);
        }
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        Self::locate_in(&dirs)
    }

    /// Find both programs in the first folders that hold them, in order.
    pub fn locate_in(dirs: &[PathBuf]) -> Result<Self, SolveError> {
        let find = |name: &str| dirs.iter().find_map(|d| find_in(d, name));
        match (find("convbin"), find("rnx2rtkp")) {
            (Some(convbin), Some(rnx2rtkp)) => Ok(Rtklib { convbin, rnx2rtkp }),
            _ => Err(SolveError::RtklibNotFound),
        }
    }

    /// Convert a raw RTCM 3 log (DJI `*_PPKRAW.bin`, `*.RTK`) to RINEX 3.04.
    /// `approx` is any time within the log's day: RTCM 3 messages carry no full date.
    pub fn convert_rtcm3(
        &self,
        raw: &Path,
        out_dir: &Path,
        approx: NaiveDateTime,
    ) -> Result<(PathBuf, PathBuf), SolveError> {
        std::fs::create_dir_all(out_dir).map_err(|e| SolveError::io(out_dir, e))?;
        let stem = raw
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "rover".into());
        let obs = out_dir.join(format!("{stem}.obs"));
        let nav = out_dir.join(format!("{stem}.nav"));
        run(&self.convbin, &convbin_args(raw, &obs, &nav, approx))?;
        for p in [&obs, &nav] {
            if !p.is_file() || std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) == 0 {
                return Err(SolveError::Missing(format!(
                    "convbin did not write {}",
                    p.display()
                )));
            }
        }
        Ok((obs, nav))
    }

    /// Run `rnx2rtkp` with an options file. Returns the solution file path.
    pub fn solve(
        &self,
        rover_obs: &Path,
        base_obs: &Path,
        navs: &[PathBuf],
        conf_text: &str,
        out_dir: &Path,
        stem: &str,
    ) -> Result<PathBuf, SolveError> {
        std::fs::create_dir_all(out_dir).map_err(|e| SolveError::io(out_dir, e))?;
        let conf = out_dir.join(format!("{stem}.conf"));
        std::fs::write(&conf, conf_text).map_err(|e| SolveError::io(&conf, e))?;
        let pos = out_dir.join(format!("{stem}.pos"));
        let _ = std::fs::remove_file(&pos);
        run(
            &self.rnx2rtkp,
            &rnx2rtkp_args(&conf, &pos, rover_obs, base_obs, navs),
        )?;
        if !pos.is_file() {
            return Err(SolveError::Missing(format!(
                "rnx2rtkp did not write {}",
                pos.display()
            )));
        }
        Ok(pos)
    }
}

/// Arguments for `convbin`: RTCM 3 input, approximate time, RINEX 3.04 with Doppler and SNR.
pub fn convbin_args(raw: &Path, obs: &Path, nav: &Path, approx: NaiveDateTime) -> Vec<OsString> {
    vec![
        "-r".into(),
        "rtcm3".into(),
        "-tr".into(),
        approx.format("%Y/%m/%d").to_string().into(),
        approx.format("%H:%M:%S").to_string().into(),
        "-v".into(),
        "3.04".into(),
        "-od".into(),
        "-os".into(),
        "-o".into(),
        obs.into(),
        "-n".into(),
        nav.into(),
        raw.into(),
    ]
}

/// Arguments for `rnx2rtkp`: options file, output, then rover, base and navigation files.
pub fn rnx2rtkp_args(conf: &Path, pos: &Path, rover: &Path, base: &Path, navs: &[PathBuf]) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec![
        "-k".into(),
        conf.into(),
        "-o".into(),
        pos.into(),
        rover.into(),
        base.into(),
    ];
    a.extend(navs.iter().map(|n| n.as_os_str().to_os_string()));
    a
}

fn run(program: &Path, args: &[OsString]) -> Result<(), SolveError> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| SolveError::io(program, e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: String = stderr
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(SolveError::Program {
            program: program.display().to_string(),
            status: out.status.code(),
            stderr: tail,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convbin_arguments() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 6, 15)
            .unwrap()
            .and_hms_opt(14, 3, 0)
            .unwrap();
        let a = convbin_args(Path::new("r.bin"), Path::new("o.obs"), Path::new("n.nav"), t);
        let s: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert_eq!(
            s,
            [
                "-r",
                "rtcm3",
                "-tr",
                "2026/06/15",
                "14:03:00",
                "-v",
                "3.04",
                "-od",
                "-os",
                "-o",
                "o.obs",
                "-n",
                "n.nav",
                "r.bin"
            ]
        );
    }

    #[test]
    fn rnx2rtkp_arguments() {
        let a = rnx2rtkp_args(
            Path::new("c.conf"),
            Path::new("o.pos"),
            Path::new("rover.obs"),
            Path::new("base.obs"),
            &[PathBuf::from("a.nav"), PathBuf::from("b.nav")],
        );
        let s: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert_eq!(
            s,
            [
                "-k",
                "c.conf",
                "-o",
                "o.pos",
                "rover.obs",
                "base.obs",
                "a.nav",
                "b.nav"
            ]
        );
    }
}
