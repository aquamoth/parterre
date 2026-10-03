//! Recording the window (`--record`): as a GIF, as a video through `ffmpeg`, or as a folder of
//! PNG frames, at [`FPS`] frames a second. Frames are encoded on a thread of their own.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use eframe::egui::{self, Color32, Pos2, Shape, Stroke, pos2};
use image::RgbaImage;

/// Frames a second of a recording.
pub const FPS: u32 = 30;

/// What a recording is written as, by the file's extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Gif,
    /// A video `ffmpeg` writes.
    Video,
    /// A folder of `frame-00001.png`, …
    Frames,
}

impl Format {
    pub fn of(path: &Path) -> Result<Format, String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("gif") => Ok(Format::Gif),
            Some("mp4" | "webm" | "mkv" | "mov") => {
                let found = Command::new("ffmpeg")
                    .arg("-version")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok();
                if found {
                    Ok(Format::Video)
                } else {
                    Err(format!(
                        "{} needs ffmpeg on the PATH; record a .gif or a folder of frames instead",
                        path.display()
                    ))
                }
            }
            None => Ok(Format::Frames),
            Some(other) => Err(format!(
                "cannot record .{other}: name a .gif, .mp4, .webm, .mkv or .mov file, or a folder"
            )),
        }
    }
}

/// A frame, and how many frame times it lasts.
type Frame = (RgbaImage, u32);

#[derive(Debug)]
pub struct Recorder {
    path: PathBuf,
    sender: Option<SyncSender<Frame>>,
    thread: Option<JoinHandle<Result<(), String>>>,
    /// The last frame, held until the next shows how long it lasted.
    last: Option<(RgbaImage, u64)>,
    /// The size of the first frame; later ones are cut or padded to it.
    size: Option<(u32, u32)>,
    /// The frame time of the last frame asked for.
    pub requested: Option<u64>,
}

impl Recorder {
    pub fn new(path: PathBuf, format: Format) -> Recorder {
        // A few frames ahead of the encoder; beyond that the window waits for it.
        let (sender, receiver) = sync_channel(8);
        let out = path.clone();
        let thread = std::thread::spawn(move || encode(&out, format, &receiver));
        Recorder {
            path,
            sender: Some(sender),
            thread: Some(thread),
            last: None,
            size: None,
            requested: None,
        }
    }

    /// Adds the frame shown from frame time `slot` on.
    pub fn push(&mut self, image: &egui::ColorImage, slot: u64) {
        let [w, h] = image.size;
        let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        let Some(mut frame) = RgbaImage::from_raw(w as u32, h as u32, bytes) else {
            return;
        };
        let (width, height) = *self.size.get_or_insert((frame.width(), frame.height()));
        if (frame.width(), frame.height()) != (width, height) {
            let mut fitted = RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
            image::imageops::replace(&mut fitted, &frame, 0, 0);
            frame = fitted;
        }
        if let Some((last, from)) = self.last.take() {
            if slot <= from {
                // Two frames for one frame time: the newer one shows.
                self.last = Some((frame, from));
                return;
            }
            self.send(last, (slot - from) as u32);
        }
        self.last = Some((frame, slot));
    }

    fn send(&mut self, frame: RgbaImage, frames: u32) {
        if let Some(sender) = &self.sender
            && sender.send((frame, frames)).is_err()
        {
            // The encoder stopped; `finish` says why.
            self.sender = None;
        }
    }

    /// Writes the last frame and waits for the encoder.
    pub fn finish(&mut self) {
        if let Some((last, _)) = self.last.take() {
            self.send(last, 1);
        }
        self.sender = None;
        let Some(thread) = self.thread.take() else {
            return;
        };
        match thread.join() {
            Ok(Ok(())) => eprintln!("saved recording to {}", self.path.display()),
            Ok(Err(e)) => {
                eprintln!("could not record {}: {e}", self.path.display());
                crate::automation::fail();
            }
            Err(_) => crate::automation::fail(),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.finish();
    }
}

fn encode(path: &Path, format: Format, frames: &Receiver<Frame>) -> Result<(), String> {
    match format {
        Format::Gif => encode_gif(path, frames),
        Format::Video => encode_video(path, frames),
        Format::Frames => encode_frames(path, frames),
    }
    .map_err(|e| e.to_string())
}

fn encode_gif(path: &Path, frames: &Receiver<Frame>) -> anyhow::Result<()> {
    use image::codecs::gif::{GifEncoder, Repeat};
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    // Quantising is most of the time; 10 of 1 (best) to 30 (fastest) still looks right for UI.
    let mut encoder = GifEncoder::new_with_speed(file, 10);
    encoder.set_repeat(Repeat::Infinite)?;
    let mut write = |image: RgbaImage, frames: u32| {
        let delay = image::Delay::from_numer_denom_ms(frames * 1000, FPS);
        encoder.encode_frame(image::Frame::from_parts(image, 0, 0, delay))
    };
    // A window mostly stands still: a frame like the one before only makes that one last.
    let mut held: Option<Frame> = None;
    for (image, n) in frames {
        match &mut held {
            Some((same, count)) if *same == image => *count += n,
            _ => {
                if let Some((image, n)) = held.replace((image, n)) {
                    write(image, n)?;
                }
            }
        }
    }
    if let Some((image, n)) = held {
        write(image, n)?;
    }
    Ok(())
}

fn encode_video(path: &Path, frames: &Receiver<Frame>) -> anyhow::Result<()> {
    let mut ffmpeg: Option<Child> = None;
    for (image, n) in frames {
        if ffmpeg.is_none() {
            ffmpeg = Some(start_ffmpeg(path, image.width(), image.height())?);
        }
        let stdin = ffmpeg.as_mut().and_then(|f| f.stdin.as_mut());
        let stdin = stdin.ok_or_else(|| anyhow::anyhow!("ffmpeg closed its input"))?;
        for _ in 0..n {
            stdin.write_all(image.as_raw())?;
        }
    }
    let Some(mut ffmpeg) = ffmpeg else {
        anyhow::bail!("nothing was recorded");
    };
    drop(ffmpeg.stdin.take());
    let status = ffmpeg.wait()?;
    anyhow::ensure!(status.success(), "ffmpeg failed ({status})");
    Ok(())
}

fn start_ffmpeg(path: &Path, width: u32, height: u32) -> std::io::Result<Child> {
    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
        ])
        .args(["-s", &format!("{width}x{height}"), "-r", &FPS.to_string()])
        .args(["-i", "-"])
        // The usual 4:2:0 video plays everywhere (browsers, GitHub) but needs even sizes.
        .args([
            "-vf",
            "pad=ceil(iw/2)*2:ceil(ih/2)*2",
            "-pix_fmt",
            "yuv420p",
        ]);
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("mov"))
    {
        // Starts playing before it has all loaded.
        command.args(["-movflags", "+faststart"]);
    }
    command.arg(path).stdin(Stdio::piped()).spawn()
}

fn encode_frames(dir: &Path, frames: &Receiver<Frame>) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut number = 0;
    for (image, n) in frames {
        let first = dir.join(format!("frame-{:05}.png", number + 1));
        image.save(&first)?;
        for i in 1..n {
            std::fs::copy(&first, dir.join(format!("frame-{:05}.png", number + 1 + i)))?;
        }
        number += n;
    }
    Ok(())
}

/// Paints the pointer, which screenshots leave out: an arrow, ringed while a button is down.
pub fn paint_pointer(ctx: &egui::Context) {
    let Some((at, down)) = ctx.input(|i| {
        let down = i.pointer.primary_down() || i.pointer.secondary_down();
        i.pointer.latest_pos().map(|p| (p, down))
    }) else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Debug,
        egui::Id::new("recorded-pointer"),
    ));
    if down {
        painter.circle(
            at,
            14.0,
            Color32::from_rgba_unmultiplied(255, 200, 0, 70),
            Stroke::new(2.0, Color32::from_rgb(255, 170, 0)),
        );
    }
    // The usual arrow, its tip on the point.
    let arrow = [
        (0.0, 0.0),
        (0.0, 17.0),
        (4.5, 13.0),
        (7.5, 19.5),
        (10.5, 18.0),
        (7.5, 11.5),
        (13.0, 11.5),
    ];
    let points: Vec<Pos2> = arrow
        .iter()
        .map(|&(x, y)| at + pos2(x, y).to_vec2())
        .collect();
    painter.add(Shape::convex_polygon(
        points.clone(),
        Color32::WHITE,
        Stroke::NONE,
    ));
    painter.add(Shape::closed_line(points, Stroke::new(1.2, Color32::BLACK)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(shade: u8) -> egui::ColorImage {
        egui::ColorImage::new([4, 2], vec![Color32::from_gray(shade); 8])
    }

    #[test]
    fn knows_formats_by_extension() {
        assert_eq!(Format::of(Path::new("a.GIF")), Ok(Format::Gif));
        assert_eq!(Format::of(Path::new("frames")), Ok(Format::Frames));
        assert!(Format::of(Path::new("a.png")).is_err());
    }

    #[test]
    fn writes_frames_for_every_frame_time() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("frames");
        let mut recorder = Recorder::new(out.clone(), Format::Frames);
        recorder.push(&image(10), 0);
        // Frame time 1 was missed: frame 0 lasts two.
        recorder.push(&image(20), 2);
        recorder.push(&image(30), 3);
        recorder.finish();
        let names: Vec<_> = (1..=4)
            .map(|i| out.join(format!("frame-{i:05}.png")))
            .collect();
        let shade = |p: &PathBuf| image::open(p).unwrap().to_rgba8().get_pixel(0, 0)[0];
        assert_eq!(
            names.iter().map(shade).collect::<Vec<_>>(),
            [10, 10, 20, 30]
        );
        assert!(!out.join("frame-00005.png").exists());
    }

    #[test]
    fn writes_a_gif_with_still_frames_joined() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("a.gif");
        let mut recorder = Recorder::new(out.clone(), Format::Gif);
        for (shade, slot) in [(10, 0), (10, 1), (10, 2), (200, 3)] {
            recorder.push(&image(shade), slot);
        }
        recorder.finish();
        use image::AnimationDecoder;
        let file = std::io::BufReader::new(std::fs::File::open(&out).unwrap());
        let frames = image::codecs::gif::GifDecoder::new(file)
            .unwrap()
            .into_frames()
            .collect_frames()
            .unwrap();
        assert_eq!(frames.len(), 2);
        let (numer, denom) = frames[0].delay().numer_denom_ms();
        assert_eq!((numer / denom).abs_diff(100), 0);
    }
}
