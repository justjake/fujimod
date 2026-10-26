//! Immutable physical exposure and film inputs. Frame edges publish this
//! descriptor; armed consumers request their own readout, never a fixture-sized
//! intermediate. Scene binning precedes PSF, sensor response and film.
use super::exposure::ExposedFrame;
use fujimod_emulator_devices::parts::optics::ApertureHandle;
use fujimod_emulator_imaging::{
    analysis::ImageLayout,
    exposure::{Calibration, ExposureHandle},
    exr2_isp::parameters::ParameterHandle,
    film::{Film, FilmPixels},
    scene::{SensorFramesHandle, frames::SensorImage},
};
use std::sync::{Arc, Mutex};
pub struct Renderer {
    frames: SensorFramesHandle,
    exposure: ExposureHandle,
    aperture: ApertureHandle,
    calibration: Calibration,
    parameters: ParameterHandle,
    projection: Option<Arc<super::optics::Projection>>,
    previous: Option<Arc<Image>>,
}
/// The live front end's fixed gain between the sensor readout the statistics
/// measure and the image it hands the film stage (Q10; 3.78x, within 0.1 EV of
/// the 12-bit live readout's factor of four). An X70 in REC shows its preview
/// brighter, relative to its own color statistics, than this renderer without
/// the gain did, by 3.78x x the 0x12c digital gain at every analog gain measured:
/// idle runs at analog 2.86x, 2.86x and 2.60x, and S1 (analog 7.56x, 0x12c 0.34x)
/// in proportion. research/live-view-exposure.md cites the runs and the method.
const LIVE_FRONT_END_GAIN_Q10: u64 = 3871;
/// A latched hardware input. All memoized readouts derive from these immutable
/// values; no later register write or lens step changes an exposed image.
pub struct Image {
    pub exposure: ExposedFrame,
    film: Film,
    gain: [u32; 3],
    projection: Option<Arc<super::optics::Projection>>,
    outputs: Mutex<Vec<(ImageLayout, Arc<[u8]>)>>,
    measurements: Mutex<Vec<([usize; 2], Vec<[usize; 4]>, ExposedFrame)>>,
    histograms: Mutex<Vec<([usize; 2], [usize; 4], [u8; 1024])>>,
}
/// Owned input to a raster job. The referenced image contains only values
/// captured at one exposure edge; later scene, focus, exposure, or film writes
/// cannot change this readout. Layout is fixed before the job leaves its owner.
#[derive(Clone)]
pub struct FrozenReadout {
    image: Arc<Image>,
    layout: ImageLayout,
}
impl FrozenReadout {
    pub fn render(&self, workers: usize) -> Result<Arc<[u8]>, String> {
        self.image.render(self.layout, workers)
    }
}
impl Renderer {
    pub fn new(
        calibration: Calibration,
        frames: SensorFramesHandle,
        exposure: ExposureHandle,
        aperture: ApertureHandle,
        parameters: ParameterHandle,
        projection: Option<super::optics::Projection>,
    ) -> Result<Self, String> {
        Ok(Self {
            frames,
            exposure,
            aperture,
            calibration: calibration.validate()?,
            parameters,
            projection: projection.map(Arc::new),
            previous: None,
        })
    }
    pub fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            "radiometric-renderer",
            &(self.calibration, self.projection.as_ref().map(|p| p.spec())),
        )
    }
    pub fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let (calibration, projection): (Calibration, Option<fujimod_emulator_imaging::optics::Spec>) =
            saved.decode("radiometric-renderer")?;
        if calibration != self.calibration
            || projection.as_ref() != self.projection.as_ref().map(|p| p.spec())
            || !saved.blobs().is_empty()
            || !saved.children().is_empty()
        {
            return Err("radiometric renderer checkpoint calibration differs".into());
        }
        Ok(())
    }
    pub fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.previous = None;
        Ok(())
    }
    pub fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({"calibration":self.calibration,"projection":self.projection.as_ref().map(|p|p.spec())})
    }
    pub fn capture(&mut self) -> Result<Option<Arc<Image>>, String> {
        let Some(mut exposure) = ExposedFrame::capture(
            &self.frames,
            &self.exposure,
            &self.aperture,
            self.calibration,
        )?
        else {
            return Ok(None);
        };
        exposure.focus_shift_mm = self.projection.as_ref().map(|p| p.focus()).transpose()?;
        exposure.view = self
            .frames
            .lock()
            .map_err(|_| "sensor frame poisoned")?
            .view;
        let p = self.parameters.lock().map_err(|_| "film bank poisoned")?;
        // 0x12c is the live digital gain (Q10, 1024 = 1x): the firmware raises
        // it to preview exposure compensation and lowers it while S1 raises
        // the analog gain. An unprogrammed bank (0) leaves the gains unchanged.
        let digital = u64::from(match p.words[0x12c / 4] & 0x7fff {
            0 => 1024,
            g => g,
        });
        let gain = [
            p.words[0x200 / 4] & 32767,
            p.words[0x200 / 4] >> 16 & 32767,
            p.words[0x204 / 4] & 32767,
        ]
        .map(|wb| {
            let gained = u64::from(wb) * digital * LIVE_FRONT_END_GAIN_Q10;
            ((gained + (1 << 19)) >> 20).min(32767) as u32
        });
        if let Some(old) = &self.previous {
            if Arc::ptr_eq(&old.exposure.frame, &exposure.frame)
                && old.exposure.exposure == exposure.exposure
                && old.exposure.aperture_squared == exposure.aperture_squared
                && old.exposure.focus_shift_mm == exposure.focus_shift_mm
                && old.exposure.view == exposure.view
                && old.gain == gain
                && old.film.matches(|o| p.words[o / 4])
            {
                return Ok(Some(old.clone()));
            }
        }
        let mut film = self
            .previous
            .as_ref()
            .map(|p| p.film.clone())
            .unwrap_or_default();
        film.update(|o| p.words[o / 4])?;
        let image = Arc::new(Image {
            exposure,
            film,
            gain,
            projection: self.projection.clone(),
            outputs: Mutex::new(Vec::new()),
            measurements: Mutex::new(Vec::new()),
            histograms: Mutex::new(Vec::new()),
        });
        self.previous = Some(image.clone());
        Ok(Some(image))
    }
}
/// Store one packed pixel pair; four-byte copies are not inlined here.
#[inline(always)]
fn store(pair: &mut [u8], value: [u8; 4]) {
    let pair: &mut [u8; 4] = pair.try_into().unwrap();
    *pair = value;
}
impl Image {
    pub fn freeze(self: &Arc<Self>, layout: ImageLayout) -> Result<FrozenReadout, String> {
        layout.bytes()?;
        Ok(FrozenReadout {
            image: self.clone(),
            layout,
        })
    }
    pub fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            "radiometric-image",
            &(
                &self.film,
                self.gain,
                self.projection.as_ref().map(|p| p.spec()),
            ),
        )?
        .with_children(vec![self.exposure.checkpoint()?]))
    }
    pub fn from_checkpoint(saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<Self, String> {
        let (film, gain, projection): (Film, [u32; 3], Option<fujimod_emulator_imaging::optics::Spec>) =
            saved.decode("radiometric-image")?;
        if saved.children().len() != 1
            || !saved.blobs().is_empty()
            || gain.iter().any(|v| *v > 32767)
        {
            return Err("invalid radiometric image checkpoint".into());
        }
        let exposure = ExposedFrame::from_checkpoint(&saved.children()[0])?;
        let projection = projection
            .map(|spec| {
                super::optics::Projection::new(
                    spec,
                    Arc::new(Mutex::new(exposure.focus_shift_mm.unwrap_or(0.))),
                )
            })
            .transpose()?
            .map(Arc::new);
        Ok(Self {
            exposure,
            film,
            gain,
            projection,
            outputs: Mutex::new(Vec::new()),
            measurements: Mutex::new(Vec::new()),
            histograms: Mutex::new(Vec::new()),
        })
    }
    /// Observational cache inventory; inspection never prepares a readout.
    pub fn readouts(&self) -> serde_json::Value {
        let outputs = self.outputs.lock().unwrap();
        let measurements = self.measurements.lock().unwrap();
        serde_json::json!({"source":[self.exposure.frame.width,self.exposure.frame.height],
            "packed_outputs":outputs.iter().map(|(l,p)|serde_json::json!({"dimensions":[l.width,l.height],"bytes":p.len()})).collect::<Vec<_>>(),
            "raw_readouts":measurements.iter().map(|(g,w,f)|serde_json::json!({"grid":g,"dimensions":f.sample_dimensions(),"window_count":w.len(),"window_samples":w.iter().map(|w|w[2]*w[3]).sum::<usize>(),"projected_pixels":f.prepared_pixels()})).collect::<Vec<_>>()})
    }
    pub fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({"exposure":self.exposure.fingerprint(),"film":self.film.words(),"gain":self.gain,"projection":self.projection.as_ref().map(|p|p.spec())})
    }
    pub(crate) fn measurement(&self, grid: [usize; 2]) -> Result<ExposedFrame, String> {
        self.measurement_windows(grid, &[[0, 0, grid[0], grid[1]]])
    }
    pub fn measurement_windows(
        &self,
        grid: [usize; 2],
        windows: &[[usize; 4]],
    ) -> Result<ExposedFrame, String> {
        if grid.contains(&0) || grid.iter().any(|&v| v > 16384) {
            return Err("invalid physical readout grid".into());
        }
        let dimensions = [
            grid[0].min(self.exposure.frame.width),
            grid[1].min(self.exposure.frame.height),
        ];
        let cache = self
            .measurements
            .lock()
            .map_err(|_| "readout cache poisoned")?;
        if let Some((_, _, frame)) = cache
            .iter()
            .find(|(g, w, f)| *g == grid && (*w == windows || f.covers(grid, windows)))
        {
            return Ok(frame.clone());
        }
        drop(cache);
        let mut frame = self.exposure.clone();
        let view = frame.view.take();
        let full = [[0, 0, grid[0], grid[1]]];
        let prepared_windows = if view.is_some() { &full[..] } else { windows };
        if let Some(projection) = &self.projection {
            projection.prepare_binned_windows(&mut frame, dimensions, grid, prepared_windows)?
        } else {
            super::optics::bin_exposure(&mut frame, dimensions)?
        }
        frame.view = view;
        // Bound retained derived pixels; the immutable source is shared.
        let cost = dimensions[0] * dimensions[1];
        let mut cache = self
            .measurements
            .lock()
            .map_err(|_| "readout cache poisoned")?;
        if let Some((_, _, cached)) = cache
            .iter()
            .find(|(g, w, f)| *g == grid && (*w == windows || f.covers(grid, windows)))
        {
            return Ok(cached.clone());
        }
        while !cache.is_empty()
            && (cache.len() >= 4
                || cache
                    .iter()
                    .map(|(_, _, f)| f.sample_dimensions().iter().product::<usize>())
                    .sum::<usize>()
                    + cost
                    > 4 * 1024 * 1024)
        {
            cache.remove(0);
        }
        if cost <= 4 * 1024 * 1024 {
            cache.push((grid, windows.to_vec(), frame.clone()));
        }
        Ok(frame)
    }
    #[inline(always)]
    fn color(&self, raw: [u16; 3], pixels: &mut FilmPixels<'_>) -> [u8; 3] {
        pixels.develop(self.white_balance(raw))
    }
    #[inline(always)]
    fn white_balance(&self, raw: [u16; 3]) -> [u16; 3] {
        let balance =
            |c: usize| ((u64::from(raw[c]) * u64::from(self.gain[c])) >> 8).min(16383) as u16;
        [balance(0), balance(1), balance(2)]
    }
    pub fn render(&self, layout: ImageLayout, workers: usize) -> Result<Arc<[u8]>, String> {
        layout.bytes()?;
        {
            let cache = self
                .outputs
                .lock()
                .map_err(|_| "film readout cache poisoned")?;
            if let Some((_, pixels)) = cache.iter().find(|(l, _)| *l == layout) {
                return Ok(pixels.clone());
            }
        }
        let frame = self.measurement([layout.width, layout.height])?;
        let [width, height] = frame.sample_dimensions();
        let mut out = vec![0; layout.bytes()?];
        if [width, height] == [layout.width, layout.height] {
            if let Some(readout) = frame.sample_offsets([width, height], [0, 0, width, height], 1) {
                super::parallel::rows(
                    &mut out,
                    layout.stride,
                    layout.height,
                    workers,
                    |range, rows| {
                        let mut memo = FilmPixels::new(&self.film);
                        let columns = &readout.columns[..layout.width];
                        for (local_y, row) in rows.chunks_exact_mut(layout.stride).enumerate() {
                            let source_row = readout.rows[range.start + local_y];
                            // Index the pairs: chunked iterators are not
                            // inlined in this crate's optimization profile.
                            let row = &mut row[..layout.width * 2];
                            for x in 0..layout.width / 2 {
                                let left = self
                                    .color(readout.sample(source_row, columns[x * 2]), &mut memo);
                                let right = self.color(
                                    readout.sample(source_row, columns[x * 2 + 1]),
                                    &mut memo,
                                );
                                store(&mut row[x * 4..x * 4 + 4], Film::pack(left, right));
                            }
                        }
                    },
                )?;
            } else {
                super::parallel::rows(
                    &mut out,
                    layout.stride,
                    layout.height,
                    workers,
                    |range, rows| {
                        let mut memo = FilmPixels::new(&self.film);
                        for (local_y, row) in rows.chunks_exact_mut(layout.stride).enumerate() {
                            let y = range.start + local_y;
                            for (x, pair) in row[..layout.width * 2].chunks_exact_mut(4).enumerate()
                            {
                                let left =
                                    self.color(frame.sample(x * 2, y, [width, height]), &mut memo);
                                let right = self
                                    .color(frame.sample(x * 2 + 1, y, [width, height]), &mut memo);
                                store(pair, Film::pack(left, right));
                            }
                        }
                    },
                )?;
            }
        } else {
            let mut colors = vec![[0; 3]; width * height];
            if let Some(readout) = frame.sample_offsets([width, height], [0, 0, width, height], 1) {
                super::parallel::rows(&mut colors, width, height, workers, |range, rows| {
                    let mut memo = FilmPixels::new(&self.film);
                    for (local_y, row) in rows.chunks_exact_mut(width).enumerate() {
                        let source_row = readout.rows[range.start + local_y];
                        for (x, color) in row.iter_mut().enumerate() {
                            *color = self
                                .color(readout.sample(source_row, readout.columns[x]), &mut memo);
                        }
                    }
                })?;
            } else {
                super::parallel::rows(&mut colors, width, height, workers, |range, rows| {
                    let mut memo = FilmPixels::new(&self.film);
                    for (local_y, row) in rows.chunks_exact_mut(width).enumerate() {
                        let y = range.start + local_y;
                        for (x, color) in row.iter_mut().enumerate() {
                            *color = self.color(frame.sample(x, y, [width, height]), &mut memo);
                        }
                    }
                })?;
            }
            for (y, row) in out.chunks_exact_mut(layout.stride).enumerate() {
                let sy = ((2 * y + 1) * height / (2 * layout.height)).min(height - 1);
                for (x, pair) in row[..layout.width * 2].chunks_exact_mut(4).enumerate() {
                    let sample = |x: usize| {
                        colors
                            [sy * width + ((2 * x + 1) * width / (2 * layout.width)).min(width - 1)]
                    };
                    store(pair, Film::pack(sample(x * 2), sample(x * 2 + 1)));
                }
            }
        }
        let out: Arc<[u8]> = out.into();
        let mut cache = self
            .outputs
            .lock()
            .map_err(|_| "film readout cache poisoned")?;
        if let Some((_, pixels)) = cache.iter().find(|(l, _)| *l == layout) {
            return Ok(pixels.clone());
        }
        while cache.iter().map(|(_, p)| p.len()).sum::<usize>() + out.len() > 8 * 1024 * 1024
            && !cache.is_empty()
        {
            cache.remove(0);
        }
        if out.len() <= 8 * 1024 * 1024 {
            cache.push((layout, out.clone()));
        }
        Ok(out)
    }
    pub fn histogram(
        &self,
        grid: [usize; 2],
        window: [usize; 4],
        out: &mut [u8; 1024],
    ) -> Result<(), String> {
        if grid.contains(&0)
            || grid.iter().any(|&v| v > 16384)
            || window.iter().any(|&v| v > 8192)
            || window[2] == 0
            || window[3] == 0
        {
            return Err("invalid histogram readout window".into());
        }
        let mut cache = self
            .histograms
            .lock()
            .map_err(|_| "histogram cache poisoned")?;
        if let Some((_, _, bins)) = cache.iter().find(|(g, w, _)| *g == grid && *w == window) {
            *out = *bins;
            return Ok(());
        }
        let clipped = [
            window[0].min(grid[0]),
            window[1].min(grid[1]),
            (window[0] + window[2])
                .min(grid[0])
                .saturating_sub(window[0]),
            (window[1] + window[3])
                .min(grid[1])
                .saturating_sub(window[1]),
        ];
        let frame = self.measurement_windows(grid, &[clipped])?;
        let [left, top, width, height] = window;
        let mut bins = [0u32; 256];
        let mut pixels = FilmPixels::new(&self.film);
        if let Some(readout) = frame.sample_offsets(grid, window, 2) {
            for &row in &readout.rows {
                for &column in &readout.columns {
                    let y = pixels.luma(self.white_balance(readout.sample(row, column)));
                    bins[y as usize] += 1;
                }
            }
        } else {
            for y in top..(top + height).min(grid[1]) {
                for x in (left..(left + width).min(grid[0])).step_by(2) {
                    let y = pixels.luma(self.white_balance(frame.sample(x, y, grid)));
                    bins[y as usize] += 1;
                }
            }
        }
        for (word, count) in out.chunks_exact_mut(4).zip(bins) {
            word.copy_from_slice(&count.to_le_bytes());
        }
        if cache.len() == 8 {
            cache.remove(0);
        }
        cache.push((grid, window, *out));
        Ok(())
    }
}

impl SensorImage for Image {
    fn render(&self, layout: ImageLayout, workers: usize) -> Result<Arc<[u8]>, String> {
        Image::render(self, layout, workers)
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Image::checkpoint(self)
    }
    fn fingerprint(&self) -> serde_json::Value {
        Image::fingerprint(self)
    }
}
impl Image {
    /// The radiometric image behind an image frame's sensor pixels.
    pub fn of(image: &Arc<dyn SensorImage>) -> Result<Arc<Self>, String> {
        (image.clone() as Arc<dyn std::any::Any + Send + Sync>)
            .downcast()
            .map_err(|_| "sensor pixels are not a radiometric image".into())
    }
    /// Decode a checkpointed sensor image.
    pub fn decode(saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<Arc<dyn SensorImage>, String> {
        Ok(Arc::new(Self::from_checkpoint(saved)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_imaging::{
        exposure::Exposure,
        exr2_isp::parameters::Parameters,
        scene::{Frame, FramePixels, SensorFrames},
    };
    fn renderer() -> (Renderer, ParameterHandle, ExposureHandle) {
        renderer_with_projection(None)
    }
    fn renderer_with_projection(
        projection: Option<super::super::optics::Projection>,
    ) -> (Renderer, ParameterHandle, ExposureHandle) {
        let frames = Arc::new(Mutex::new(SensorFrames::new(
            Frame::new(
                64,
                48,
                FramePixels::Rgb24(
                    (0..64 * 48)
                        .flat_map(|i| [if i % 2 == 0 { 0 } else { 255 }; 3])
                        .collect(),
                ),
            )
            .unwrap(),
        )));
        let exposure = Arc::new(Mutex::new(Exposure {
            seconds: 0.0001,
            analog_gain: 1.,
        }));
        let parameters = Arc::new(Mutex::new(Parameters {
            words: fujimod_emulator_devices::test_host::film_words(),
            film: Film::default(),
        }));
        let r = Renderer::new(
            Calibration {
                full_scale: 16380,
                reference_white: [256; 3],
                saturation_lux_seconds: 1.,
                color_sample_weights: [2, 5, 2],
            },
            frames,
            exposure.clone(),
            Arc::new(Mutex::new(4.)),
            parameters.clone(),
            projection,
        )
        .unwrap();
        (r, parameters, exposure)
    }
    fn scalar_render(image: &Image, layout: ImageLayout) -> Vec<u8> {
        let frame = image.measurement([layout.width, layout.height]).unwrap();
        let [width, height] = frame.sample_dimensions();
        let mut pixels = FilmPixels::new(&image.film);
        let mut colors = Vec::with_capacity(width * height);
        if let Some(readout) = frame.sample_offsets([width, height], [0, 0, width, height], 1) {
            for &row in &readout.rows {
                for &column in &readout.columns {
                    colors.push(image.color(readout.sample(row, column), &mut pixels));
                }
            }
        } else {
            for y in 0..height {
                for x in 0..width {
                    colors.push(image.color(frame.sample(x, y, [width, height]), &mut pixels));
                }
            }
        }
        let mut output = vec![0; layout.bytes().unwrap()];
        for (y, row) in output.chunks_exact_mut(layout.stride).enumerate() {
            let source_y = ((2 * y + 1) * height / (2 * layout.height)).min(height - 1);
            for (x, pair) in row[..layout.width * 2].chunks_exact_mut(4).enumerate() {
                let sample = |x: usize| {
                    colors[source_y * width
                        + ((2 * x + 1) * width / (2 * layout.width)).min(width - 1)]
                };
                pair.copy_from_slice(&Film::pack(sample(x * 2), sample(x * 2 + 1)));
            }
        }
        output
    }
    #[test]
    fn sensor_window_changes_pixels_and_statistics_and_survives_later_input_and_checkpoint() {
        use fujimod_emulator_imaging::scene::view::View;
        let (mut renderer, _, _) = renderer();
        renderer.frames.lock().unwrap().set_frame(
            Frame::new(
                64,
                48,
                FramePixels::Rgb24(
                    (0..64 * 48)
                        .flat_map(|i| [(i % 64 * 3) as u8, (i / 64 * 4) as u8, 80])
                        .collect(),
                ),
            )
            .unwrap(),
        );
        let full = renderer.capture().unwrap().unwrap();
        renderer.frames.lock().unwrap().view = Some(View::new([64, 48], [16, 8, 32, 24]).unwrap());
        let cropped = renderer.capture().unwrap().unwrap();
        assert!(!Arc::ptr_eq(&full, &cropped));
        let a = full.measurement([64, 48]).unwrap();
        let b = cropped.measurement([64, 48]).unwrap();
        for y in 0..48 {
            for x in 0..64 {
                assert_eq!(
                    b.sample(x, y, [64, 48]),
                    a.sample(16 + (2 * x + 1) / 4, 8 + (2 * y + 1) / 4, [64, 48])
                );
            }
        }
        let layout = ImageLayout {
            width: 64,
            height: 48,
            stride: 128,
        };
        let pixels = cropped.render(layout, 1).unwrap();
        assert_ne!(pixels, full.render(layout, 1).unwrap());
        assert_eq!(&*pixels, &scalar_render(&cropped, layout));
        renderer.frames.lock().unwrap().view = None;
        assert_eq!(cropped.render(layout, 1).unwrap(), pixels);
        let restored = Image::from_checkpoint(&cropped.checkpoint().unwrap()).unwrap();
        assert_eq!(restored.render(layout, 1).unwrap(), pixels);
        assert_eq!(restored.exposure.view, cropped.exposure.view);
    }

    #[test]
    fn live_gains_are_white_balance_times_digital_gain_times_the_front_end() {
        // 0x12c (Q10) times the fixed 3.78x front end: an X70 lowers 0x12c to
        // 348 while S1 raises the analog gain 2.9x, keeping its preview level.
        let (mut renderer, parameters, _) = renderer();
        let words = parameters.lock().unwrap().words.clone();
        let wb = [
            words[0x200 / 4] & 32767,
            words[0x200 / 4] >> 16 & 32767,
            words[0x204 / 4] & 32767,
        ];
        let expect = |digital: u64| {
            wb.map(|w| {
                ((u64::from(w) * digital * LIVE_FRONT_END_GAIN_Q10 + (1 << 19)) >> 20).min(32767)
                    as u32
            })
        };
        assert_eq!(renderer.capture().unwrap().unwrap().gain, expect(1024));
        parameters.lock().unwrap().words[0x12c / 4] = 1024;
        assert_eq!(renderer.capture().unwrap().unwrap().gain, expect(1024));
        parameters.lock().unwrap().words[0x12c / 4] = 348;
        assert_eq!(renderer.capture().unwrap().unwrap().gain, expect(348));
    }
    #[test]
    fn exposure_publication_is_lazy_and_reuses_only_consumed_parameters() {
        let (mut renderer, parameters, exposure) = renderer();
        let first = renderer.capture().unwrap().unwrap();
        assert!(first.outputs.lock().unwrap().is_empty());
        assert!(first.measurements.lock().unwrap().is_empty());
        let layout = ImageLayout {
            width: 16,
            height: 12,
            stride: 36,
        };
        let pixels = first.render(layout, 0).unwrap();
        assert_eq!(first.measurements.lock().unwrap()[0].0, [16, 12]);
        assert_eq!(pixels.len(), 432);
        assert!(
            pixels
                .chunks_exact(36)
                .all(|r| r[32..].iter().all(|v| *v == 0))
        );
        parameters.lock().unwrap().words[0] = 1;
        let unchanged = renderer.capture().unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &unchanged));
        assert!(Arc::ptr_eq(&pixels, &unchanged.render(layout, 0).unwrap()));
        exposure.lock().unwrap().seconds *= 2.;
        let changed = renderer.capture().unwrap().unwrap();
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(changed.outputs.lock().unwrap().is_empty());
        assert_eq!(first.render(layout, 0).unwrap(), pixels);
    }
    #[test]
    fn frozen_readout_keeps_capture_inputs_across_focus_exposure_and_film_updates() {
        use crate::optics::Projection;
        use fujimod_emulator_imaging::optics::{Illumination, Spec};

        let focus = Arc::new(Mutex::new(0.2));
        let projection = Projection::new(
            Spec {
                focus: "focus".into(),
                focal_length_mm: 18.5,
                subject_distance_mm: 2000.,
                sensor_width_mm: 2.,
                relative_illumination: Illumination::Uniform,
            },
            focus.clone(),
        )
        .unwrap();
        let (mut renderer, parameters, exposure) = renderer_with_projection(Some(projection));
        let first = renderer.capture().unwrap().unwrap();
        let layout = ImageLayout {
            width: 32,
            height: 24,
            stride: 72,
        };
        let frozen = first.freeze(layout).unwrap();
        let before_fingerprint = first.fingerprint();
        let expected = scalar_render(&first, layout);
        assert_eq!(frozen.render(2).unwrap().as_ref(), expected);

        exposure.lock().unwrap().seconds *= 2.;
        *focus.lock().unwrap() = 1.2;
        {
            let mut parameters = parameters.lock().unwrap();
            parameters.words[0x200 / 4] = 257 | 193 << 16;
            parameters.words[0x400 / 4] = 1536;
        }
        let updated = renderer.capture().unwrap().unwrap();
        assert!(!Arc::ptr_eq(&first, &updated));
        assert_ne!(updated.fingerprint(), before_fingerprint);
        assert_eq!(first.fingerprint(), before_fingerprint);
        assert_eq!(frozen.render(0).unwrap().as_ref(), expected);
        assert_eq!(frozen.render(2).unwrap().as_ref(), expected);
    }
    #[test]
    fn binned_readout_integrates_light_before_film_and_roundtrips_without_rendering() {
        let (mut renderer, _, _) = renderer();
        let frame = renderer.capture().unwrap().unwrap();
        let before = frame.fingerprint();
        let saved = frame.checkpoint().unwrap();
        assert!(frame.outputs.lock().unwrap().is_empty());
        let restored = Image::from_checkpoint(&saved).unwrap();
        assert_eq!(before, restored.fingerprint());
        let layout = ImageLayout {
            width: 16,
            height: 12,
            stride: 32,
        };
        let output = frame.render(layout, 0).unwrap();
        assert_eq!(output, restored.render(layout, 0).unwrap());
        // The two-pixel black/white pattern averages in linear irradiance;
        // point sampling before film would select alternating endpoints.
        assert!(output.chunks_exact(4).all(|p| p[0] == p[1]));
        assert!(output.chunks_exact(4).all(|p| p == &output[..4]));
        assert_eq!(before, frame.fingerprint());
        let mut bins = [0; 1024];
        frame
            .histogram([64, 48], [0, 0, 64, 48], &mut bins)
            .unwrap();
        let total: u32 = bins
            .chunks_exact(4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .sum();
        assert_eq!(total, 64 * 48 / 2);
        assert_eq!(frame.histograms.lock().unwrap().len(), 1);
    }
    #[test]
    fn exact_and_upscaled_outputs_match_the_scalar_renderer() {
        let (mut renderer, _, _) = renderer();
        let image = renderer.capture().unwrap().unwrap();
        for (width, height) in [(16, 12), (64, 48), (128, 96)] {
            let layout = ImageLayout {
                width,
                height,
                stride: width * 2 + 8,
            };
            let expected = scalar_render(&image, layout);
            assert_eq!(image.render(layout, 2).unwrap().as_ref(), expected);
            image.outputs.lock().unwrap().clear();
            assert_eq!(image.render(layout, 0).unwrap().as_ref(), expected);
        }
    }
    #[test]
    fn readouts_match_scalar_film_across_binning_projection_padding_and_restore() {
        use crate::optics::Projection;
        use fujimod_emulator_imaging::optics::{Illumination, Spec};
        for [width, height] in [[1, 1], [5, 3], [17, 11]] {
            for [out_width, out_height] in [[2, 1], [14, 7], [4, 19], [40, 12]] {
                for projection_mode in 0..3 {
                    let rgb: Vec<u8> = (0..width * height * 3)
                        .map(|i| ((i * 79 + i / 3 * 17) % 256) as u8)
                        .collect();
                    let frames = Arc::new(Mutex::new(SensorFrames::new(
                        Frame::new(width, height, FramePixels::Rgb24(rgb.into())).unwrap(),
                    )));
                    let exposure = Arc::new(Mutex::new(Exposure {
                        seconds: 0.0001,
                        analog_gain: 1.3,
                    }));
                    let aperture = Arc::new(Mutex::new(4.));
                    let calibration = Calibration {
                        full_scale: 16380,
                        reference_white: [256, 192, 320],
                        saturation_lux_seconds: 1.,
                        color_sample_weights: [8, 20, 8],
                    };
                    let mut words = fujimod_emulator_devices::test_host::film_words();
                    words[0x200 / 4] = 257 | 193 << 16;
                    words[0x204 / 4] = 311;
                    let parameters = Arc::new(Mutex::new(Parameters {
                        words: words.clone(),
                        film: Film::default(),
                    }));
                    let spec = Spec {
                        focus: "focus".into(),
                        focal_length_mm: 18.5,
                        subject_distance_mm: 2000.,
                        sensor_width_mm: 2.,
                        relative_illumination: if projection_mode == 2 {
                            Illumination::CosineFourth
                        } else {
                            Illumination::Uniform
                        },
                    };
                    let projection = || {
                        (projection_mode != 0).then(|| {
                            Projection::new(spec.clone(), Arc::new(Mutex::new(0.2))).unwrap()
                        })
                    };
                    let layout = ImageLayout {
                        width: out_width,
                        height: out_height,
                        stride: out_width * 2 + 8,
                    };
                    let mut expected = vec![0; layout.bytes().unwrap()];
                    let mut sampled =
                        ExposedFrame::capture(&frames, &exposure, &aperture, calibration)
                            .unwrap()
                            .unwrap();
                    if let Some(p) = projection() {
                        sampled.focus_shift_mm = Some(p.focus().unwrap());
                        p.prepare_binned_windows(
                            &mut sampled,
                            [width.min(out_width), height.min(out_height)],
                            [out_width, out_height],
                            &[[0, 0, out_width, out_height]],
                        )
                        .unwrap();
                    } else {
                        crate::optics::bin_exposure(
                            &mut sampled,
                            [width.min(out_width), height.min(out_height)],
                        )
                        .unwrap();
                    }
                    let mut film = Film::default();
                    film.update(|o| words[o / 4]).unwrap();
                    for y in 0..out_height {
                        for x in (0..out_width).step_by(2) {
                            let color = |x| {
                                let raw = sampled.sample(x, y, [out_width, out_height]);
                                film.develop(std::array::from_fn(|c| {
                                    let gain = ([257u64, 193, 311][c] * 1024 * LIVE_FRONT_END_GAIN_Q10
                                        + (1 << 19))
                                        >> 20;
                                    ((u64::from(raw[c]) * gain) >> 8).min(16383) as u16
                                }))
                            };
                            let at = y * layout.stride + x * 2;
                            expected[at..at + 4]
                                .copy_from_slice(&Film::pack(color(x), color(x + 1)));
                        }
                    }
                    let mut renderer = Renderer::new(
                        calibration,
                        frames,
                        exposure,
                        aperture,
                        parameters,
                        projection(),
                    )
                    .unwrap();
                    let image = renderer.capture().unwrap().unwrap();
                    let saved = image.checkpoint().unwrap();
                    let actual = image.render(layout, 0).unwrap();
                    let restored = Image::from_checkpoint(&saved).unwrap();
                    assert_eq!(actual, restored.render(layout, 0).unwrap());
                    assert_eq!(
                        actual.as_ref(),
                        expected,
                        "source {width}x{height}, output {out_width}x{out_height}, projection {projection_mode}"
                    );
                }
            }
        }
    }
}
