//! Build and stage flash-reader artifacts from explicit caller-selected inputs.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = std::env::args()
        .nth(1)
        .ok_or("stage recipe JSON required")?;
    let recipe = serde_json::from_slice(&std::fs::read(input)?)?;
    fujimod_workflows::flash::build::stage(&recipe)
}
