//! Offline facts review. This executable has no device discovery or USB calls.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use fujimod_workflows::service_capture::Facts;
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: service_capture_plan FACTS.json")?;
    let facts = Facts::from_json(&std::fs::read_to_string(path)?)?;
    let review = facts.review();
    println!("{}", serde_json::to_string_pretty(&review)?);
    if !review.ready {
        std::process::exit(1);
    }
    Ok(())
}
