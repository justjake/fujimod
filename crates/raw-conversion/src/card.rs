//! RAFs on the camera's SD card, read in USB CARD READER mode, where the SD
//! card is a PTP store: list them with their ISO and capture date, then
//! download the chosen ones. Reads only.
use crate::ptp::{self, Pipe, Reader, Session};

const GET_OBJECT_INFO: u16 = 0x1008;
const GET_PARTIAL_OBJECT: u16 = 0x101b;
/// RAF header: "FUJIFILMCCD-RAW ", then at 84 the big-endian offset and
/// length of the embedded JPEG preview, whose Exif carries the ISO.
const RAF_HEADER: u32 = 92;
const PREVIEW_EXIF: u32 = 64 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct CardRaf {
    pub handle: u32,
    pub name: String,
    pub size: u32,
    /// PTP DateTime, `YYYYMMDDThhmmss`.
    pub captured: String,
    pub iso: Option<u32>,
}

async fn call<P: Pipe>(s: &mut Session<P>, code: u16, params: &[u32]) -> Result<Vec<u8>, String> {
    Ok(s.call(code, params, None)
        .await
        .map_err(|e| e.to_string())?
        .data)
}

pub async fn list<P: Pipe>(session: &mut Session<P>) -> Result<Vec<CardRaf>, String> {
    let data = call(session, ptp::GET_OBJECT_HANDLES, &[0xffff_ffff, 0, 0]).await?;
    let handles = Reader::new(&data).u32_array().map_err(|e| e.to_string())?;
    let mut rafs = Vec::new();
    for handle in handles {
        let info = call(session, GET_OBJECT_INFO, &[handle]).await?;
        let mut r = Reader::new(&info);
        let parse = |r: &mut Reader| -> ptp::Result<(u32, String, String)> {
            r.u32()?; // StorageID
            r.u16()?; // ObjectFormat
            r.u16()?; // ProtectionStatus
            let size = r.u32()?;
            r.u16()?; // ThumbFormat
            for _ in 0..7 {
                r.u32()?; // thumb size/width/height, image width/height/depth, parent
            }
            r.u16()?; // AssociationType
            r.u32()?; // AssociationDesc
            r.u32()?; // SequenceNumber
            Ok((size, r.string()?, r.string()?))
        };
        let (size, name, captured) = parse(&mut r).map_err(|e| e.to_string())?;
        if !name.to_ascii_uppercase().ends_with(".RAF") {
            continue;
        }
        let iso = raf_iso(session, handle).await;
        rafs.push(CardRaf {
            handle,
            name,
            size,
            captured,
            iso,
        });
    }
    Ok(rafs)
}

/// ISO from the RAF's preview Exif, read with two partial reads; None when
/// the camera or the file does not allow it.
async fn raf_iso<P: Pipe>(session: &mut Session<P>, handle: u32) -> Option<u32> {
    let header = call(session, GET_PARTIAL_OBJECT, &[handle, 0, RAF_HEADER])
        .await
        .ok()?;
    if !header.starts_with(b"FUJIFILMCCD-RAW ") || header.len() < 92 {
        return None;
    }
    let offset = u32::from_be_bytes(header[84..88].try_into().ok()?);
    let length = u32::from_be_bytes(header[88..92].try_into().ok()?);
    let preview = call(
        session,
        GET_PARTIAL_OBJECT,
        &[handle, offset, length.min(PREVIEW_EXIF)],
    )
    .await
    .ok()?;
    crate::jpeg::exif_iso(&preview)
}

pub async fn fetch<P: Pipe>(session: &mut Session<P>, handle: u32) -> Result<Vec<u8>, String> {
    call(session, ptp::GET_OBJECT, &[handle]).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_camera::FakeCamera;
    use futures_lite::future::block_on;

    #[test]
    fn lists_card_rafs_with_iso_and_fetches_them() {
        let mut camera = FakeCamera::new(512);
        let raf = camera.add_card_raf("DSCF0005.RAF", 3200, "20261110T120000");
        camera.add_card_file("DSCF0005.JPG", vec![0xff, 0xd8]);
        block_on(async {
            let mut session = Session::open(camera).await.unwrap();
            let rafs = list(&mut session).await.unwrap();
            assert_eq!(rafs.len(), 1);
            assert_eq!(rafs[0].name, "DSCF0005.RAF");
            assert_eq!(rafs[0].iso, Some(3200));
            assert_eq!(rafs[0].captured, "20261110T120000");
            assert_eq!(rafs[0].size as usize, raf.len());
            assert_eq!(fetch(&mut session, rafs[0].handle).await.unwrap(), raf);
            let camera = session.close().await.unwrap_or_else(|_| panic!("close"));
            assert_eq!(camera.objects.len(), 2, "nothing deleted");
        });
    }
}
