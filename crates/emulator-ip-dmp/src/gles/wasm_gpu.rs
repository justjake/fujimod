use super::{Engine, HostGpuExecution, Reader, draw_state, gpu_draw, raster};

impl Engine {
    /// Render one command list with WebGPU and map its readback asynchronously.
    pub async fn execute_host_gpu_async(
        &mut self,
        words: &[u32],
        read: &mut Reader<'_>,
    ) -> Result<HostGpuExecution, String> {
        let mut next = self.clone();
        next.gpu_primitives = Some(Vec::new());
        next.gpu_immediate = None;
        if !next.vertices.is_empty() {
            if next.vertices.len() != next.pending_inputs.len() {
                return Err("DMP checkpoint lost pending immediate vertex inputs".into());
            }
            let mut inputs = next.pending_inputs.clone();
            if next.draw_state().raster.primitive_mode == draw_state::PrimitiveMode::TriangleStrip
                && next.primitive_index & 1 != 0
                && inputs.len() >= 2
            {
                inputs.swap(0, 1);
            }
            next.gpu_immediate = Some((next.draw_state(), inputs));
            next.vertices.clear();
            next.pending_inputs.clear();
        }

        let mut unused_output = raster::Targets::default();
        let mut recorded_reads = Vec::new();
        {
            let mut recording_reader = |address, length| {
                let bytes = read(address, length)?;
                recorded_reads.push((address, bytes.clone()));
                Ok(bytes)
            };
            next.packets(words, &mut recording_reader, &mut unused_output, 0)?;
        }
        next.flush_gpu_immediate()?;
        let draws = next.gpu_primitives.take().unwrap_or_default();
        let rendered = gpu_draw::render_async(&draws, &recorded_reads, read).await?;
        if rendered.queue_submissions > 3 {
            return Err(format!(
                "DMP host GPU submitted {} command buffers for one list (limit 3)",
                rendered.queue_submissions
            ));
        }
        next.gpu_primitives = None;
        next.gpu_immediate = None;
        next.written_ranges = rendered.written_ranges;
        *self = next;
        Ok(HostGpuExecution {
            snapshots: rendered.snapshots,
            draws: rendered.draws,
            queue_submissions: rendered.queue_submissions,
        })
    }
}
