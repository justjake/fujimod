//! Inspect a firmware definition without connecting to USB.
use fujimod_catalog_service::Definition;
use fujimod_catalog_firmware_identity::FirmwareIdentity;
use fujimod_usb_app80::operations::Service;
use fujimod_usb_app80_types::catalog::Command;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: service_catalog DEFINITION_JSON FIRMWARE_ID SHA256".into());
    }
    let identity = FirmwareIdentity::new(&args[1], &args[2])?;
    let definition = Definition::from_json(&std::fs::read_to_string(&args[0])?, &identity)?;
    let service = Service::new(definition)?;
    println!(
        "firmware\t{}",
        service.catalog().definition().identity().id()
    );
    for entry in service.catalog().definition().entries() {
        println!(
            "{:02x}\t{:?}\t{:?}\t{}",
            entry.code(),
            entry.codec(),
            entry.enabled_modes(),
            entry.label()
        );
    }
    Ok(())
}
