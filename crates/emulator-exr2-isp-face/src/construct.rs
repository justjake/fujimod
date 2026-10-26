//! EXR II board constructors for this crate's models.
use fujimod_emulator_exr2_plan::{Model, construction::{Builder, Built, Constructors, decode}};
use fujimod_emulator_exr2_resources::wire;

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("face-detector", face_detector);
    constructors.add("face-results", face_results);
}

fn face_detector(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::FaceDetector { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    let options = decode(options)?;
    Ok(Built::Device(Box::new(
        crate::face_detector::FaceDetector::new(options),
    )))
}

fn face_results(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::FaceResults { profile, routes } = model else {
        return Err("constructor called for another model kind".into());
    };
    let profile = decode(profile)?;
    Ok(Built::Device({
        Box::new(crate::face_results::FaceResults::new(
            profile,
            wire!(resources, id, routes, IrqRoutes),
        ))
    }))
}
