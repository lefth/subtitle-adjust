# subtitle-adjust

This program adjusts subtitle timings or positions.

Use subtitle-adjust to fix the time offset or time scale of subtitles that were meant for a different cut or a different
playback speed.

This program knows about offset, scale, and an offset start time. The offset is in seconds,
and can be negative to move the subtitles sooner. Scale is good for compensating for different
playback speeds. `--subs-are-fast` and `--subs-are-slow` fix the most common speed errors
(related to the differing PAL and NTSC frame rates).

Subtitles can also be moved to the top or bottom of the frame without applying any timing changes.

Times are input as [[hh:]mm:]ss[,ms], a decimal number of seconds, or a mix like 1:30.4.

#### USAGE:
    subtitle-adjust [FLAGS] [OPTIONS] <input>

#### FLAGS:
    -e, --extract          If ffmpeg or ffmpeg.exe is found, use it to extract .srt subtitles from a video or other
                           subtitle file format
    -h, --help             Prints help information
    -r, --renumber         Should the number of the subtitles be recounted/rewritten?
        --subs-are-fast    If the subtitles are continually jumping further and further ahead, use this option. It will
                           guess the values for the most common scenario
        --subs-are-slow    If the subtitles are continually lagging more and more behind, use this option. It will guess
                           the values for the most common scenario
    -V, --version          Prints version information

#### OPTIONS:
    -f, --from <from>                    `--from` and `--to` can be used together to create an offset, instead of
                                         `--offset`
    -o, --offset <offset>                How much should the subtitle be shifted forward? Negative values will shift the
                                         subtitles backward
    -s, --offset-start <offset-start>    At what timestamp should subtitles start to be adjusted? Adjustment will occur
                                         from this point to the end
        --scale <scale>                  Scale the subtitle speed slower (<1) or faster (>1)
        --scale-pivot <scale-pivot>      This is the time that's assumed to be perfectly matched already when scaling
                                         subtitles faster or slower
    -t, --to <to>                        `--from` and `--to` can be used together to create an offset, instead of
                                         `--offset`
        --to-bottom <to-bottom>...       Move subtitles in this time range to the bottom of the screen. This operation
                                         has no effect on subtitles that don't currently have an overridden position;
                                         the only effect is to remove position tags. The time given is before any timing
                                         adjustments. The start or end time may be omitted, for example: 10-20, -1:00.5,
                                         300-, -. Negative times are allowed. This may not be supported by all players
        --to-top <to-top>...             Move subtitles in this time range to the top of the screen. This operation
                                         can't be used with subtitles that have pixel-based positions. The time given is
                                         before any timing adjustments. The start or end time may be omitted, for
                                         example: 10-20, -1:00.5, 300-, -. Negative times are allowed. This may not be
                                         supported by all players

#### ARGS:
    <input>    Input file in the SubRip (.srt) format


## Installation

If [cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html) is installed, run:
```
cargo install --git https://github.com/lefth/subtitle-adjust
```

## Examples

If the subtitles have become delayed after a scene and change (t=30 seconds) and should be moved forward
a second after that point:
```
    subtitle-adjust movie.srt --offset -1 --offset-start 30
```

If subtitles begin appearing at 10 seconds but should start at 45 seconds:
```
    subtitle-adjust movie.srt --from 10 --to 45
```

If credits are shown for 2 minutes and subtitles should be shown at the top of the screen for that duration:
```
    subtitle-adjust movie.srt --to-top -2:00
```
Or if the credits don't start right away:
```
    subtitle-adjust movie.srt --to-top 30-2:00
```

If subtitles are getting progressively slower due to a mistake in converting between PAL and NTSC:
```
    subtitle-adjust movie.srt --subs-are-slow
```
Or if the speed doesn't match, but the subtitles have already been synced to match at t=10:
```
    subtitle-adjust movie.srt --subs-are-slow --scale-pivot 10
```
