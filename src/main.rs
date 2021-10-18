use std::fs::rename;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::ErrorKind;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use encoding_rs_io::DecodeReaderBytes;
use lazy_static::lazy_static;
use log::LevelFilter;
#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use regex::Regex;
use structopt::StructOpt;

mod lib;
use crate::lib::*;

fn main() -> Result<()> {
    let opt = init()?;

    if opt.extract {
        extract_subtitles(&opt.path)
    } else {
        let mut subs = get_subtitles(&opt.path).context("Error processing subtitles")?;
        modify(&mut subs, &opt)?;
        backup(&opt.path)?;
        if let Err(err) = write_to_disk(subs, &opt.path) {
            restore(&opt.path)?;
            bail!(err);
        }
        Ok(())
    }
}

fn init() -> Result<OptFinal> {
    let mut log_builder = env_logger::Builder::new();
    if cfg!(debug_assertions) {
        log_builder.filter_level(LevelFilter::Trace);
    } else {
        log_builder.filter_level(LevelFilter::Warn);
        // Output looks better in releases if it's not written like a log file:
        log_builder.format_module_path(false);
        log_builder.format_level(false);
        log_builder.format_timestamp(None);
    }
    log_builder.init();

    Opt::from_args().validate()
}

/// Extract subtitles to .srt from a video file or other format subtitle.
/// Needs ffmpeg.
fn extract_subtitles(path: &std::path::PathBuf) -> Result<()> {
    let output = path.with_extension("srt");
    // NOTE: If run in WSL, this can invoke ffmpeg.exe if ffmpeg isn't found,
    // but paths may not be valid for Windows executables. It works for paths
    // without leading directory parts.
    for executable in ["ffmpeg", "ffmpeg.exe"] {
        let mut command = Command::new(executable);
        let handle = command
            .arg("-i")
            .arg(path)
            .arg("-loglevel")
            .arg("quiet")
            .arg(&output)
            .spawn();
        match handle {
            Ok(mut handle) => {
                handle.wait()?;
                return Ok(());
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                info!("Will try to continue after error: {}", err);
                continue;
            }
            Err(err) => bail!(err),
        };
    }
    bail!("Cannot extract subtitles: could not find `ffmpeg` or `ffmpeg.exe`.");
}

fn get_subtitles(path: &std::path::PathBuf) -> Result<SubData> {
    info!("Opening input file: {:#?}", &path);
    let file = File::open(&path)?;
    // This library will detect the encoding and remove the BOM if present:
    let decoder = DecodeReaderBytes::new(file);
    let mut reader = BufReader::new(decoder);

    let mut subs = Vec::new();
    let mut line_ending = None;

    let mut part_number: Option<i64> = None;
    let mut part_times: Option<TimeSpan> = None; // each is milliseconds
    let mut part_position: Option<Position> = None;
    let mut part_lines: Option<Vec<String>> = None;

    loop {
        let mut buf = String::new();
        if let Ok(0) = reader.read_line(&mut buf) {
            if part_lines.is_some() {
                subs.push(Subtitle {
                    number: part_number.take().unwrap(),
                    time_span: part_times.take().unwrap(),
                    position: part_position.take(),
                    lines: part_lines.take().unwrap(),
                });
            }
            return Ok(SubData {
                subs,
                line_ending: line_ending.unwrap_or("\n".to_string()),
            });
        }
        line_ending.get_or_insert_with(|| {
            if buf.ends_with("\r\n") {
                "\r\n".to_string()
            } else {
                "\n".to_string()
            }
        });

        if buf.trim().is_empty() {
            if part_lines.is_some() {
                // a Subtitle struct is now finished
                if part_lines.is_some() {
                    subs.push(Subtitle {
                        number: part_number.take().unwrap(),
                        time_span: part_times.take().unwrap(),
                        position: part_position.take(),
                        lines: part_lines.take().unwrap(),
                    });
                }
            }
        } else if part_number.is_none() {
            part_number = Some(
                buf.trim()
                    .parse::<i64>()
                    .with_context(|| format!("Was expecting integer, found {:#?}", buf))?,
            );
        } else if part_times.is_none() {
            // looking for 00:00:08,614 --> 00:00:10,373
            // or          00:00:08,614 --> 00:00:10,373  X1:201 X2:516 Y1:397 Y2:423
            // Negative timestamps are not part of the standard AFAIK, but they need to be created
            // and parsed so moving a subtitle back too far doesn't permanently remove its timing data.
            lazy_static! {
                static ref RE: Regex =
                    Regex::new(r"(.*) --> (.*)(\s+X1:(-?\d+) X2:(-?\d+) Y1:(-?\d+) Y2:(-?\d+))?")
                        .unwrap();
            }
            let captures = RE
                .captures(&buf)
                .with_context(|| format!("Expecting time --> time; got: {:#?}", buf))?;
            let start_ms = parse_ms(
                captures
                    .get(1)
                    .ok_or(anyhow!("Missing start time"))?
                    .as_str(),
            );
            let end_ms = parse_ms(captures.get(2).ok_or(anyhow!("Missing end time"))?.as_str());
            part_times = Some(TimeSpan::new(start_ms?, end_ms?));

            part_position = if captures.get(3).is_some() {
                Some(Position {
                    x1: captures.get(4).unwrap().as_str().parse()?,
                    x2: captures.get(5).unwrap().as_str().parse()?,
                    y1: captures.get(6).unwrap().as_str().parse()?,
                    y2: captures.get(7).unwrap().as_str().parse()?,
                })
            } else {
                None
            };
        } else {
            part_lines.get_or_insert_with(|| vec![]).push(buf);
        }
    }
}

fn backup(path: &Path) -> Result<()> {
    let mut dest_path = path.as_os_str().to_owned();
    dest_path.push(".bak");
    info!("Backing up file to {:#?}", dest_path);
    rename(path, &dest_path)?;
    Ok(())
}

fn restore(path: &Path) -> Result<()> {
    info!("Restoring .bak file");
    let mut backup_path = path.as_os_str().to_owned();
    backup_path.push(".bak");
    rename(backup_path, path)?;
    Ok(())
}

fn modify(data: &mut SubData, opt: &OptFinal) -> Result<()> {
    info!("Applying changes to the subtitle in memory.");

    for i in 0..data.subs.len() {
        let ref mut sub = data.subs[i];
        if opt.renumber_offset {
            sub.number = (i + 1) as i64;
        }

        // Move the subtitle up or down if needed:
        if opt
            .to_top
            .iter()
            .any(|interval| interval.contains(sub.time_span.start_ms))
        {
            if sub.position.is_some() {
                bail!("Cannot override subtitle position information at {} because it has hard coded position.", sub.time_span.start_ms);
            }

            // Add a position tag at the beginning, replacing any existing position tag:
            lazy_static! {
                static ref RE: Regex = Regex::new(r"^(\{\\an\d+\})?").unwrap();
            }
            sub.lines[0] = RE.replace(sub.lines[0].as_str(), r"{\an8}").to_string();
        } else if opt
            .to_bottom
            .iter()
            .any(|interval| interval.contains(sub.time_span.start_ms))
        {
            // Remove any hard coded coordinates and any {\anX} positions:
            sub.position.take();
            lazy_static! {
                static ref RE: Regex = Regex::new(r"^\{\\an\d+\}").unwrap();
            }
            sub.lines[0] = RE.replace(sub.lines[0].as_str(), "").to_string();
        }

        // Apply the offset (if it's active at the current time):
        if sub.time_span.start_ms >= opt.offset_start_ms {
            sub.time_span.start_ms += opt.offset_ms;
            sub.time_span.end_ms += opt.offset_ms;

            if let Some(scale) = opt.scale {
                let pivot = opt.scale_pivot.unwrap_or_default();
                sub.time_span.start_ms =
                    pivot + (scale * (sub.time_span.start_ms - pivot) as f64) as i64;
                sub.time_span.end_ms =
                    pivot + (scale * (sub.time_span.end_ms - pivot) as f64) as i64;
            }
        }
    }
    Ok(())
}

fn write_to_disk(data: SubData, path: &Path) -> Result<()> {
    info!("Writing modified subtitle to disk: {:#?}", path);
    let file = File::create(path)?;
    write!(BufWriter::new(file), "{}", data)?;
    Ok(())
}
