//! Opt-in, one-shot capture of the launcher's real native viewport.
use anyhow::{Context, Result, anyhow};
use eframe::egui;
use std::{fs::File, io::BufWriter, path::PathBuf, sync::Arc};

pub struct WindowCapture {
    path: PathBuf,
    requested: bool,
    result: Option<Result<()>>,
}

impl WindowCapture {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            requested: false,
            result: None,
        }
    }

    /// Called after drawing the normal UI; only the native painter supplies the image.
    pub fn update(&mut self, ctx: &egui::Context) {
        if self.result.is_some() {
            return;
        }
        if !self.requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.requested = true;
            return;
        }
        let image = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot {
                    viewport_id, image, ..
                } if *viewport_id == egui::ViewportId::ROOT => Some(Arc::clone(image)),
                _ => None,
            })
        });
        if let Some(image) = image {
            self.result = Some(self.write_png(&image).with_context(|| {
                format!("writing launcher window capture {}", self.path.display())
            }));
            // A write failure also exits through run_native so main can report it.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub fn finish(&mut self) -> Result<()> {
        self.result.take().unwrap_or_else(|| {
            Err(anyhow!(
                "window closed before launcher window capture completed: {}",
                self.path.display()
            ))
        })
    }

    fn write_png(&self, image: &egui::ColorImage) -> Result<()> {
        let file = File::create(&self.path)?;
        let mut encoder = png::Encoder::new(
            BufWriter::new(file),
            image.size[0].try_into()?,
            image.size[1].try_into()?,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(image.as_raw())?;
        writer.finish()?;
        Ok(())
    }
}
