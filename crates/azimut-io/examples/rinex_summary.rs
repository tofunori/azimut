fn main() {
    for p in std::env::args().skip(1) {
        let r = azimut_io::rinex::summarize(std::path::Path::new(&p));
        let t = |s: Option<f64>| s.map(azimut_io::time::format_dji_clock).unwrap_or_default();
        println!(
            "{p}\n  v{:?} {:?} first={} last={} int={:?} bands={} ant={:?} dH={:?} err={:?} read={}/{}",
            r.version,
            r.kind,
            t(r.first_epoch_s),
            t(r.last_epoch_s),
            r.interval_s,
            r.bands_label(),
            r.antenna_type,
            r.antenna_delta_hen_m.map(|d| d[0]),
            r.error,
            r.bytes_read,
            r.size
        );
    }
}
