use std::{
    fmt::{Display, Write},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Result};
use lazy_static::lazy_static;
use regex::Regex;
use structopt::*;

const PAL: f64 = 25.0;
const NTSC: f64 = 23.976;

#[derive(Debug, StructOpt)]
#[structopt(about = "Adjust subtitle timing or positions in SRT files.")]
/// Use this program to fix the time offset or time scale of subtitles that were meant for a different
/// cut or a different playback speed.
///
/// This program knows about offset, scale, and an offset start time. The offset is in
/// seconds, and can be negative to move the subtitles sooner. Scale is good for
/// compensating for different playback speeds.
/// `--subs-are-fast` and `--subs-are-slow` fix the most common speed errors (related to
/// the differing PAL and NTSC frame rates).
///
/// Subtitles can also be moved to the top or bottom of the frame without applying any
/// timing changes.
///
/// Times are input as [[hh:]mm:]ss[,ms], a decimal number of seconds, or a mix like 1:30.4.

pub struct Opt {
    /// Input file in the SubRip (.srt) format.
    #[structopt(parse(from_os_str), name("input"))]
    path: PathBuf,

    #[structopt(flatten)]
    scale_opts: ScaleOpts,

    #[structopt(flatten)]
    offset_opts: OffsetOpts,

    /// Move subtitles in this time range to the top of the screen.
    /// This operation can't be used with subtitles that have pixel-based positions.
    /// The time given is before any timing adjustments.
    /// The start or end time may be omitted, for example: 10-20, -1:00.5, 300-, -. Negative times are allowed.
    /// This may not be supported by all players.
    #[structopt(long, parse(try_from_str = parse_timespan), allow_hyphen_values(true))]
    to_top: Vec<TimeSpan>,

    /// Move subtitles in this time range to the bottom of the screen.
    /// This operation has no effect on subtitles that don't currently have an overridden position;
    /// the only effect is to remove position tags.
    /// The time given is before any timing adjustments.
    /// The start or end time may be omitted, for example: 10-20, -1:00.5, 300-, -. Negative times are allowed.
    /// This may not be supported by all players.
    #[structopt(long, parse(try_from_str = parse_timespan), allow_hyphen_values(true))]
    to_bottom: Vec<TimeSpan>,

    /// Should the number of the subtitles be recounted/rewritten?
    #[structopt(short, long)]
    renumber: bool,

    /// If ffmpeg or ffmpeg.exe is found, use it to extract .srt subtitles from a video or other
    /// subtitle file format.
    #[structopt(short, long)]
    extract: bool,
}

#[derive(Debug, StructOpt)]
struct OffsetOpts {
    /// `--from` and `--to` can be used together to create an offset, instead of `--offset`.
    #[structopt(short, long, parse(try_from_str = parse_ms), allow_hyphen_values(true))]
    from: Option<i64>,
    /// `--from` and `--to` can be used together to create an offset, instead of `--offset`.
    #[structopt(short, long, parse(try_from_str = parse_ms), allow_hyphen_values(true))]
    to: Option<i64>,

    /// How much should the subtitle be shifted forward? Negative values will shift the subtitles backward.
    #[structopt(short, long, parse(try_from_str = parse_ms), allow_hyphen_values(true))]
    offset: Option<i64>,

    /// At what timestamp should subtitles start to be adjusted? Adjustment will occur from this
    /// point to the end.
    #[structopt(short = "s", long, parse(try_from_str = parse_ms), allow_hyphen_values(true))]
    offset_start: Option<i64>,
}

#[derive(Debug, StructOpt)]
struct ScaleOpts {
    /// Scale the subtitle speed slower (<1) or faster (>1).
    #[structopt(long)]
    scale: Option<f64>,

    /// This is the time that's assumed to be perfectly matched already
    /// when scaling subtitles faster or slower.
    #[structopt(long, parse(try_from_str = parse_ms), allow_hyphen_values(true))]
    scale_pivot: Option<i64>,

    /// If the subtitles are continually lagging more and more behind, use this option. It will guess
    /// the values for the most common scenario.
    #[structopt(long)]
    subs_are_slow: bool,
    /// If the subtitles are continually jumping further and further ahead, use this option. It will guess
    /// the values for the most common scenario.
    #[structopt(long)]
    subs_are_fast: bool,
}

impl Opt {
    pub fn validate(&mut self) -> Result<OptFinal> {
        if !Path::exists(self.path.as_path()) {
            bail!("Input path does not exist: {:#?}", self.path);
        } else if std::fs::read_link(self.path.as_path()).is_ok() {
            // Note: we're not checking for special file types. That's rare and requires
            // platform specific code.
            bail!("Will not modify a symlink.");
        }

        if self.offset_opts.from.is_some() != self.offset_opts.to.is_some() {
            bail!("The `--from` and `--to` arguments must be used together.")
        }
        if self.offset_opts.from.is_some() && self.offset_opts.offset.is_some() {
            bail!("The `--from`/`--to` arguments can't be uset with `--offset`.")
        }
        if self.scale_opts.subs_are_fast as i32
            + self.scale_opts.subs_are_slow as i32
            + self.scale_opts.scale.is_some() as i32
            + self.extract as i32
            > 1
        {
            bail!(
                "Only one of the --extract, --scale, --subs-are-fast, and --subs-are-slow options are allowed."
            )
        }

        // Convert from subs are fast/slow to scale
        if self.scale_opts.subs_are_fast {
            self.scale_opts.scale.replace(PAL / NTSC);
        } else if self.scale_opts.subs_are_slow {
            self.scale_opts.scale.replace(NTSC / PAL);
        }

        if self.offset_opts.offset_start.is_some() && self.scale_opts.scale.is_some() {
            // If this turns out to be useful, I'll add the feature.
            bail!("Cannot both scale and set an offset start, because the meaning is unclear.");
        }

        if self.offset_opts.offset.is_some() && self.scale_opts.scale.is_some() {
            // If this turns out to be useful, I'll add the feature.
            bail!("Cannot both scale and offset together, because mistakes are too likely. \
                Instead, first sync the subtitles at a point in time then use --scale and --scale-pivot together.");
        }

        if self.scale_opts.scale_pivot.is_some() && self.scale_opts.scale.is_none() {
            bail!("Cannot use a scale pivot without some type of time scaling.");
        }

        // Convert --to/--from to --offset:
        if self.offset_opts.from.is_some() {
            self.offset_opts.offset =
                Some(self.offset_opts.to.take().unwrap() - self.offset_opts.from.take().unwrap());
        }

        if self.offset_opts.offset.is_none()
            && self.scale_opts.scale.is_none()
            && self.to_bottom.is_empty()
            && self.to_top.is_empty()
            && !self.extract
        {
            bail!(
                "`--extract` or one of the offset options, the scale options, or the `--to-top`, `--to-bottom` \
                options much be used.\nSee `--help` for details."
            );
        }

        // This isn't the most efficient check but who cares since there's typically few or no intervals.
        for to_top_interval in &self.to_top {
            for to_bottom_interval in &self.to_bottom {
                if to_top_interval.contains(to_bottom_interval.start_ms)
                    || to_top_interval.contains(to_bottom_interval.end_ms)
                    || to_bottom_interval.contains(to_top_interval.start_ms)
                    || to_bottom_interval.contains(to_top_interval.end_ms)
                {
                    bail!("The times to move subtitles to the top and to the bottom overlap; can't do both at the same time.");
                }
            }
        }

        if self.extract
            && (self.renumber
                || self.scale_opts.scale.is_some()
                || self.scale_opts.scale_pivot.is_some()
                || self.offset_opts.offset.is_some()
                || self.offset_opts.offset_start.is_some()
                || !self.to_bottom.is_empty()
                || !self.to_top.is_empty())
        {
            bail!("Cannot combine `--extract` with other options or operations.");
        }

        Ok(OptFinal {
            path: self.path.clone(),

            scale: self.scale_opts.scale,
            scale_pivot: self.scale_opts.scale_pivot,
            offset_ms: self.offset_opts.offset.unwrap_or_default(),
            offset_start_ms: self.offset_opts.offset_start.unwrap_or(i64::MIN),
            renumber_offset: self.renumber,
            to_top: self.to_top.clone(),
            to_bottom: self.to_bottom.clone(),
            extract: self.extract,
        })
    }
}

/// This is a non-ambiguous version of the program options.
pub struct OptFinal {
    pub scale: Option<f64>,
    pub scale_pivot: Option<i64>,
    pub offset_ms: i64,
    pub offset_start_ms: i64,
    pub renumber_offset: bool,
    pub path: PathBuf,
    pub to_top: Vec<TimeSpan>,
    pub to_bottom: Vec<TimeSpan>,
    pub extract: bool,
}

#[derive(Debug, PartialEq, Clone)]
pub struct TimeSpan {
    pub start_ms: i64,
    pub end_ms: i64,
}

impl TimeSpan {
    pub fn new(start_ms: i64, end_ms: i64) -> Self {
        Self { start_ms, end_ms }
    }

    /// Check whether a time is within this interval.
    pub fn contains(&self, ms: i64) -> bool {
        ms >= self.start_ms && ms <= self.end_ms
    }
}

pub struct Subtitle {
    pub number: i64,
    pub time_span: TimeSpan,
    pub position: Option<Position>,
    pub lines: Vec<String>,
}

/// Data of hard coded pixel-based positions. This format may be dependent on resolution.
/// It's not well documented. Tags like {\an2}, {\an8} work better, but those are stored
/// in the text data.
pub struct Position {
    pub x1: i32, // position left
    pub x2: i32, // position right
    pub y1: i32, // position up
    pub y2: i32, // position down
}

pub(crate) struct Milliseconds(pub i64);

impl Display for Milliseconds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ms = if self.0 < 0 {
            f.write_char('-')?;
            -self.0
        } else {
            self.0
        };

        let hours = ms / 3600_000;
        ms -= hours * 3600_000;
        write!(f, "{:02}:", hours)?;
        let minutes = ms / 60_000;
        ms -= minutes * 60_000;
        write!(f, "{:02}:", minutes)?;
        let seconds = ms / 1_000;
        ms -= seconds * 1_000;
        write!(f, "{:02},", seconds)?;
        write!(f, "{:03}", ms % 1_000)?;
        Ok(())
    }
}

impl Display for TimeSpan {
    /// Format the start and end ms to [-]hh:mm:ss,MMM --> [-]hh:mm:ss,MMM
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} --> {}",
            Milliseconds(self.start_ms),
            Milliseconds(self.end_ms)
        )
    }
}

impl Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "X1:{} X2:{} Y1:{} Y2:{}",
            self.x1, self.x2, self.y1, self.y2
        )
    }
}

pub struct SubData {
    pub subs: Vec<Subtitle>,
    pub line_ending: String,
}

impl Display for SubData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for sub in self.subs.iter() {
            write!(f, "{}{}", sub.number, self.line_ending)?; // add the number
            write!(f, "{}", sub.time_span)?; // add the times
            if let Some(ref position) = sub.position {
                write!(f, "  {}", position)?;
            }
            write!(f, "{}", self.line_ending)?;
            for line in sub.lines.iter() {
                write!(f, "{}", line)?; // add the text
            }
            f.write_str(self.line_ending.as_str())?; // add a blank line
        }
        Ok(())
    }
}

pub(crate) static NUMBER_REGEX: &str = r"(?x) # allow whitespace/comments
    (-)? # negative?
    (?:
    (?:(?:(\d+):)?(?:(\d+):))? # [[hours:]minutes:]
    (\d+) # seconds
    (?:[,.](\d+))? # the decimal part
    |
    \.(\d+) # only decimal, no prior digit
    )";

/// Parse the digits after "." to a decimal, so for example "050" ("0.05") is 50 ms.
pub(crate) fn parse_decimal_part(n: &str) -> Result<u64> {
    let trimmed = n.trim_matches('0');
    if trimmed.is_empty() {
        return Ok(0);
    }
    let mut result = trimmed.parse::<u64>()? * 1000 / 10_u64.pow(trimmed.len() as u32);

    // Count the leading 0s:
    for digit in n.chars() {
        if digit == '0' {
            result /= 10;
        } else {
            break;
        }
    }
    Ok(result)
}

/// Parse [[hh:]mm:]ss[,ms] into seconds. Or ss.ms. Comma or period is okay.
pub fn parse_ms(input: &str) -> Result<i64> {
    lazy_static! {
        static ref RE: Regex = Regex::new(format!(r"^{}\s*$", NUMBER_REGEX).as_str()).unwrap();
    };

    if let Some(captures) = RE.captures(input) {
        let sign = captures.get(1).map_or(1, |_| -1);

        if let Some(only_ms) = captures.get(6) {
            let ms = parse_decimal_part(only_ms.as_str())?;
            return Ok(sign * ms as i64);
        }

        let hours = captures.get(2).map_or(Ok(0), |n| n.as_str().parse())?;
        let minutes = captures.get(3).map_or(Ok(0), |n| n.as_str().parse())?;
        let seconds = captures.get(4).map_or(Ok(0), |n| n.as_str().parse())?;
        let ms = captures
            .get(5)
            .map_or(Ok(0), |n| parse_decimal_part(n.as_str()))?;

        // this blocks "1:90" but allows "90"
        if minutes > 60 || (seconds > 60 && (minutes > 0 || hours > 0)) {
            bail!("Invalid minutes or seconds value");
        }

        Ok(sign * (ms + 1000 * (seconds + 60 * (minutes + 60 * hours))) as i64)
    } else {
        bail!("Cannot coerce value into timestamp: {}", input)
    }
}

/// Parse intervals like a-b, a-, -b, where a and b are timestamps.
pub(crate) fn parse_timespan(input: &str) -> Result<TimeSpan> {
    lazy_static! {
        static ref RE: Regex =
            Regex::new(format!(r"^({})?-({})?$", NUMBER_REGEX, NUMBER_REGEX).as_str()).unwrap();
    }

    let captures = RE
        .captures(input)
        .ok_or_else(|| anyhow!("Malformed timespan: {:#?}", input))?;

    let start_time = captures
        .get(1)
        .map_or(Ok(i64::MIN), |m| parse_ms(m.as_str()))?;
    let end_time = captures
        .get(8)
        .map_or(Ok(i64::MAX), |m| parse_ms(m.as_str()))?;

    if start_time >= end_time {
        bail!("Timespan end must come after the start: {}", input);
    }
    Ok(TimeSpan::new(start_time, end_time))
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use crate::{
        parse_decimal_part, parse_ms, parse_timespan, Milliseconds, Position, SubData, Subtitle,
        TimeSpan, NUMBER_REGEX,
    };

    #[test]
    fn test_parse_decimal_part() {
        assert_eq!(parse_decimal_part("111").unwrap(), 111);
        assert_eq!(parse_decimal_part("0005").unwrap(), 0);
        assert_eq!(parse_decimal_part("005").unwrap(), 5);
        assert_eq!(parse_decimal_part("050").unwrap(), 50);
        assert_eq!(parse_decimal_part("05").unwrap(), 50);
        assert_eq!(parse_decimal_part("5").unwrap(), 500);
        assert_eq!(parse_decimal_part("50").unwrap(), 500);
        assert_eq!(parse_decimal_part("500").unwrap(), 500);
        assert_eq!(parse_decimal_part("5000").unwrap(), 500);
        assert_eq!(parse_decimal_part("000").unwrap(), 0);
    }

    #[test]
    fn test_parse_ms() {
        let re = Regex::new(format!("^{}$", NUMBER_REGEX).as_str()).unwrap();
        assert!(re.find("-").is_none());
        assert!(re.find(".").is_none());
        assert!(re.find("1-").is_none());
        assert!(re.find("1-2").is_none());

        assert!(parse_ms("-").is_err());
        assert!(parse_ms("1-").is_err());
        assert!(parse_ms("1:61").is_err());
        assert!(parse_ms("61:00").is_err());
        assert!(parse_ms("61:00:00\r\n").is_ok());
        // This kind of data can occur when copying times from grep/ack/rg output (includes a line number):
        assert!(parse_ms("31-00:02:52,965").is_err());
        assert!(parse_ms("31:00:02:52,965").is_err());
        assert!(parse_ms(":00:02:52,965").is_err());

        assert_eq!(parse_ms("90.5").unwrap(), 90500);
        assert_eq!(parse_ms("-9.05").unwrap(), -9050);
        assert_eq!(parse_ms("-0.1").unwrap(), -100);
        assert_eq!(parse_ms("-0.10").unwrap(), -100);
        assert_eq!(parse_ms(".111").unwrap(), 111);
        assert_eq!(parse_ms("1.111").unwrap(), 1111);
        assert_eq!(parse_ms("1.1").unwrap(), 1100);
        assert_eq!(parse_ms(".1").unwrap(), 100);
        assert_eq!(parse_ms("-.3").unwrap(), -300);
        assert_eq!(parse_ms(".01").unwrap(), 10);
        assert_eq!(parse_ms("1.10\r").unwrap(), 1100);
        assert_eq!(parse_ms("90.01\n").unwrap(), 90010);
        assert_eq!(
            parse_ms("1:2:3.200").unwrap(),
            200 + 1000 * (3 + 60 * (2 + 60 * 1))
        );
    }

    #[test]
    pub fn test_parse_timespan() {
        assert_eq!(
            parse_timespan("1-1:00.5").unwrap(),
            TimeSpan::new(1000, 60500)
        );
        assert_eq!(parse_timespan("-1-2").unwrap(), TimeSpan::new(-1000, 2000));
        assert_eq!(
            parse_timespan("-1--0.5").unwrap(),
            TimeSpan::new(-1000, -500)
        );
        assert_eq!(
            parse_timespan("-1--.5").unwrap(),
            TimeSpan::new(-1000, -500)
        );
        assert_eq!(parse_timespan("-2").unwrap(), TimeSpan::new(i64::MIN, 2000));
        assert_eq!(
            parse_timespan("-").unwrap(),
            TimeSpan::new(i64::MIN, i64::MAX)
        );
        assert_eq!(
            parse_timespan("-2-").unwrap(),
            TimeSpan::new(-2000, i64::MAX)
        );
        assert_eq!(
            parse_timespan("--2").unwrap(),
            TimeSpan::new(i64::MIN, -2000)
        );
        assert!(parse_timespan("2-1").is_err());
    }

    #[test]
    fn test_format_ms() {
        assert_eq!(format!("{}", Milliseconds(65565123)), "18:12:45,123");
        assert_eq!(format!("{}", Milliseconds(-65565123)), "-18:12:45,123");

        let ts = TimeSpan {
            start_ms: -65565123,
            end_ms: 65565123,
        };
        assert_eq!(format!("{}", ts), "-18:12:45,123 --> 18:12:45,123");
    }

    #[test]
    fn test_format_subtitle() {
        for line_ending in vec!["\n".to_string(), "\r\n".to_string()] {
            let data = SubData {
                subs: vec![
                    Subtitle {
                        number: 1,
                        time_span: TimeSpan::new(0, 1000),
                        position: None,
                        lines: vec![format!("l1{}", line_ending), format!("l2{}", line_ending)],
                    },
                    Subtitle {
                        number: 2,
                        time_span: TimeSpan::new(2000, 3000),
                        position: Some(Position {
                            x1: 1,
                            x2: 2,
                            y1: 3,
                            y2: 4,
                        }),
                        lines: vec![format!("l3{}", line_ending)],
                    },
                ],
                line_ending: line_ending.to_owned(),
            };
            assert_eq!(
                format!("{}", data),
                format!(
                    "1{line_ending}\
                    00:00:00,000 --> 00:00:01,000{line_ending}\
                    l1{line_ending}\
                    l2{line_ending}\
                    {line_ending}\
                    2{line_ending}\
                    00:00:02,000 --> 00:00:03,000  X1:1 X2:2 Y1:3 Y2:4{line_ending}\
                    l3{line_ending}\
                    {line_ending}",
                    line_ending = line_ending
                )
            );
        }
    }
}
