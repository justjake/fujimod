use super::*;

#[pymethods]
impl Board {
    fn set_loop_mode(&self, mode: &str) -> PyResult<()> {
        self.board()?
            .set_loop_mode(fujimod_emulator_cpu::loops::LoopMode::parse(mode).map_err(error)?)
            .map_err(error)
    }
    fn loop_statistics(&self) -> PyResult<String> {
        self.board()?
            .loop_statistics()
            .map(|value| value.to_string())
            .map_err(error)
    }
}
