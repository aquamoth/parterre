use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use eframe::egui::{self, Color32, Pos2, Shape, Stroke, vec2};
use image::RgbaImage;

pub const FPS: u32 = 30;

/// Frames the window may run ahead of the encoder; beyond that it waits for it.
const ENCODER_BACKLOG: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Gif,
    Video,
    PngFrames,
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
            None => Ok(Format::PngFrames),
            Some(other) => Err(format!(
                "cannot record .{other}: name a .gif, .mp4, .webm, .mkv or .mov file, or a folder"
            )),
        }
    }
}

/// An image and how many frame times it lasts.
type Frame = (RgbaImage, u32);

#[derive(Debug)]
pub struct Recorder {
    path: PathBuf,
    sender: Option<SyncSender<Frame>>,
    encoder: Option<JoinHandle<Result<(), String>>>,
    /// Held, with the frame time it shows from, until the next frame shows how long it lasts.
    held: Option<(RgbaImage, u64)>,
    /// Later frames are cut or padded to the first one's size.
    first_size: Option<(u32, u32)>,
    pub last_requested_frame_time: Option<u64>,
}

impl Recorder {
    pub fn new(path: PathBuf, format: Format) -> Recorder {
        let (sender, receiver) = sync_channel(ENCODER_BACKLOG);
        let out = path.clone();
        let encoder = std::thread::spawn(move || encode(&out, format, &receiver));
        Recorder {
            path,
            sender: Some(sender),
            encoder: Some(encoder),
            held: None,
            first_size: None,
            last_requested_frame_time: None,
        }
    }

    pub fn push(&mut self, screenshot: &egui::ColorImage, frame_time: u64) {
        let Some(mut frame) = to_rgba(screenshot) else {
            return;
        };
        let (width, height) = *self.first_size.get_or_insert(frame.dimensions());
        if frame.dimensions() != (width, height) {
            let mut fitted = RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
            image::imageops::replace(&mut fitted, &frame, 0, 0);
            frame = fitted;
        }
        if let Some((held, shown_from)) = self.held.take() {
            if frame_time <= shown_from {
                // Two frames for one frame time: the newer one shows.
                self.held = Some((frame, shown_from));
                return;
            }
            let lasts = u32::try_from(frame_time - shown_from).unwrap_or(u32::MAX);
            self.send(held, lasts);
        }
        self.held = Some((frame, frame_time));
    }

    fn send(&mut self, image: RgbaImage, lasts: u32) {
        if let Some(sender) = &self.sender
            && sender.send((image, lasts)).is_err()
        {
            // The encoder stopped; `finish` says why.
            self.sender = None;
        }
    }

    fn finish(&mut self) {
        if let Some((held, _)) = self.held.take() {
            self.send(held, 1);
        }
        self.sender = None;
        let Some(encoder) = self.encoder.take() else {
            return;
        };
        match encoder.join() {
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

pub fn to_rgba(image: &egui::ColorImage) -> Option<RgbaImage> {
    let [width, height] = image.size.map(|side| u32::try_from(side).ok());
    let bytes = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    RgbaImage::from_raw(width?, height?, bytes)
}

fn encode(path: &Path, format: Format, frames: &Receiver<Frame>) -> Result<(), String> {
    match format {
        Format::Gif => encode_gif(path, frames),
        Format::Video => encode_video(path, frames),
        Format::PngFrames => encode_png_frames(path, frames),
    }
    .map_err(|e| e.to_string())
}

fn encode_gif(path: &Path, frames: &Receiver<Frame>) -> anyhow::Result<()> {
    use image::codecs::gif::{GifEncoder, Repeat};
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    // Quantising takes most of the time; speed 10 of 1 (best) to 30 still looks right for UI.
    let mut encoder = GifEncoder::new_with_speed(file, 10);
    encoder.set_repeat(Repeat::Infinite)?;
    let mut write = |image: RgbaImage, lasts: u32| {
        let delay = image::Delay::from_numer_denom_ms(lasts * 1000, FPS);
        encoder.encode_frame(image::Frame::from_parts(image, 0, 0, delay))
    };
    // A window mostly stands still: a frame like the one before only makes that one last.
    let mut held: Option<Frame> = None;
    for (image, lasts) in frames {
        match &mut held {
            Some((same, held_lasts)) if *same == image => *held_lasts += lasts,
            _ => {
                if let Some((image, lasts)) = held.replace((image, lasts)) {
                    write(image, lasts)?;
                }
            }
        }
    }
    if let Some((image, lasts)) = held {
        write(image, lasts)?;
    }
    Ok(())
}

fn encode_video(path: &Path, frames: &Receiver<Frame>) -> anyhow::Result<()> {
    let mut ffmpeg: Option<Child> = None;
    for (image, lasts) in frames {
        if ffmpeg.is_none() {
            ffmpeg = Some(start_ffmpeg(path, image.width(), image.height())?);
        }
        let stdin = ffmpeg.as_mut().and_then(|f| f.stdin.as_mut());
        let stdin = stdin.ok_or_else(|| anyhow::anyhow!("ffmpeg closed its input"))?;
        for _ in 0..lasts {
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

fn encode_png_frames(dir: &Path, frames: &Receiver<Frame>) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut written = 0;
    for (image, lasts) in frames {
        let first = dir.join(format!("frame-{:05}.png", written + 1));
        image.save(&first)?;
        for i in 1..lasts {
            std::fs::copy(
                &first,
                dir.join(format!("frame-{:05}.png", written + 1 + i)),
            )?;
        }
        written += lasts;
    }
    Ok(())
}

const POINTER_ARROW: [(f32, f32); 7] = [
    (0.0, 0.0),
    (0.0, 17.0),
    (4.5, 13.0),
    (7.5, 19.5),
    (10.5, 18.0),
    (7.5, 11.5),
    (13.0, 11.5),
];

/// Screenshots leave the pointer out: this paints one, ringed while a button is down.
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
    let points: Vec<Pos2> = POINTER_ARROW
        .iter()
        .map(|&(x, y)| at + vec2(x, y))
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
        assert_eq!(Format::of(Path::new("frames")), Ok(Format::PngFrames));
        assert!(Format::of(Path::new("a.png")).is_err());
    }

    #[test]
    fn writes_frames_for_every_frame_time() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("frames");
        let mut recorder = Recorder::new(out.clone(), Format::PngFrames);
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
        for (shade, frame_time) in [(10, 0), (10, 1), (10, 2), (200, 3)] {
            recorder.push(&image(shade), frame_time);
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
