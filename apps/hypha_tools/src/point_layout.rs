//! Point cloud layouts that a conversion rewrites.
//!
//! Older Autoware recordings hold `sensor_msgs/PointCloud2` in the XYZI layout: `x`, `y`,
//! `z` and `intensity`, four `float32`, 16 bytes a point. Autoware 2.1.0's nodes read only
//! XYZIRC: `x`, `y`, `z` as `float32`, `intensity` and `return_type` as `uint8`,
//! `channel` as `uint16`, also 16 bytes a point
//! (`autoware_point_types::PointXYZIRC`). [`xyzi_to_xyzirc`] rewrites one into the other.

use hypha_node::sdk::msgs::sensor_msgs::{PointCloud2, PointField};

/// The bytes of one point in both layouts.
const POINT_STEP: u32 = 16;
/// The same, as an index step.
const POINT_BYTES: usize = 16;

fn field(name: &str, offset: u32, datatype: u8) -> PointField {
    PointField { name: name.to_owned(), offset, datatype, count: 1 }
}

/// True when `cloud` is little-endian XYZI with 16 bytes a point.
fn is_xyzi(cloud: &PointCloud2) -> bool {
    let expected = [("x", 0), ("y", 4), ("z", 8), ("intensity", 12)];
    let fields_match = cloud.fields.len() == expected.len()
        && cloud.fields.iter().zip(expected).all(|(field, (name, offset))| {
            field.name == name
                && field.offset == offset
                && field.datatype == PointField::FLOAT32
                && field.count == 1
        });
    fields_match && cloud.point_step == POINT_STEP && !cloud.is_bigendian
}

/// Rewrite `cloud` from XYZI into XYZIRC, in place, and return the highest intensity it
/// held (0 for a cloud with none).
///
/// Each intensity is rounded and held to 0 to 255, as a `uint8`; a NaN intensity becomes
/// 0. Every return type and channel is 0, which the recording does not hold. `row_step`
/// becomes `width` times the point size: an XYZI recording may hold a wrong one. The
/// highest intensity tells a caller when the recorded values are not on the 0 to 255
/// scale, so that rounding turns them all into 0.
///
/// # Errors
/// A message when `cloud` is not little-endian XYZI, or its data is not `width` times
/// `height` points.
pub fn xyzi_to_xyzirc(cloud: &mut PointCloud2) -> Result<f32, String> {
    if !is_xyzi(cloud) {
        let names: Vec<&str> = cloud.fields.iter().map(|field| field.name.as_str()).collect();
        return Err(format!(
            "not a little-endian XYZI cloud of 16 bytes a point: fields {names:?}, point_step {}",
            cloud.point_step
        ));
    }
    let expected = u64::from(cloud.width)
        .checked_mul(u64::from(cloud.height))
        .and_then(|points| points.checked_mul(u64::from(POINT_STEP)));
    let length = u64::try_from(cloud.data.len()).ok();
    if expected != length {
        return Err(format!(
            "{} bytes of data, not {} x {} points of {POINT_STEP} bytes",
            cloud.data.len(),
            cloud.width,
            cloud.height
        ));
    }
    let mut highest = 0.0_f32;
    // The length check above leaves no remainder.
    for point in cloud.data.as_chunks_mut::<POINT_BYTES>().0 {
        // The last four bytes of a point: the intensity in XYZI; intensity, return type
        // and channel in XYZIRC.
        let Some(tail) = point.last_chunk_mut::<4>() else {
            continue;
        };
        let intensity = f32::from_le_bytes(*tail);
        highest = highest.max(intensity);
        // NaN becomes 0; the cast saturates, so 0..=255 is all it can give.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "rounded and held to 0..=255 first"
        )]
        let level = intensity.round().clamp(0.0, 255.0) as u8;
        *tail = [level, 0, 0, 0];
    }
    cloud.fields = vec![
        field("x", 0, PointField::FLOAT32),
        field("y", 4, PointField::FLOAT32),
        field("z", 8, PointField::FLOAT32),
        field("intensity", 12, PointField::UINT8),
        field("return_type", 13, PointField::UINT8),
        field("channel", 14, PointField::UINT16),
    ];
    cloud.row_step = cloud.width.checked_mul(POINT_STEP).ok_or("a row of more than 4 GiB")?;
    Ok(highest)
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a test whose setup fails is a failed test")]
mod tests {
    use hypha_node::sdk::msgs::sensor_msgs::{PointCloud2, PointField};

    use super::xyzi_to_xyzirc;

    fn xyzi(points: &[[f32; 4]]) -> PointCloud2 {
        let field = |name: &str, offset| PointField {
            name: name.to_owned(),
            offset,
            datatype: PointField::FLOAT32,
            count: 1,
        };
        PointCloud2 {
            height: 1,
            width: u32::try_from(points.len()).expect("a few points"),
            fields: vec![field("x", 0), field("y", 4), field("z", 8), field("intensity", 12)],
            point_step: 16,
            // A wrong row step, as the Isuzu recordings hold one.
            row_step: 7,
            data: points.iter().flatten().flat_map(|value| value.to_le_bytes()).collect(),
            ..PointCloud2::default()
        }
    }

    #[test]
    fn an_xyzi_cloud_becomes_xyzirc_with_its_intensity_rounded_and_held() {
        let mut cloud =
            xyzi(&[[1.0, 2.0, 3.0, 12.4], [4.0, 5.0, 6.0, 300.0], [7.0, 8.0, 9.0, f32::NAN]]);
        assert_eq!(xyzi_to_xyzirc(&mut cloud), Ok(300.0));
        let layout: Vec<(&str, u32, u8)> = cloud
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.offset, field.datatype))
            .collect();
        assert_eq!(
            layout,
            [
                ("x", 0, PointField::FLOAT32),
                ("y", 4, PointField::FLOAT32),
                ("z", 8, PointField::FLOAT32),
                ("intensity", 12, PointField::UINT8),
                ("return_type", 13, PointField::UINT8),
                ("channel", 14, PointField::UINT16),
            ]
        );
        assert_eq!((cloud.point_step, cloud.row_step), (16, 48));
        let tails: Vec<&[u8]> =
            cloud.data.chunks(16).map(|point| point.get(12..).expect("a tail")).collect();
        assert_eq!(tails, [&[12, 0, 0, 0][..], &[255, 0, 0, 0][..], &[0, 0, 0, 0][..]]);
        let first_x = cloud.data.get(..4).expect("x");
        assert_eq!(first_x, 1.0_f32.to_le_bytes(), "x, y and z are kept");
    }

    #[test]
    fn a_cloud_that_is_not_xyzi_or_whose_data_is_short_is_refused() {
        let mut converted = xyzi(&[[1.0, 2.0, 3.0, 4.0]]);
        assert!(xyzi_to_xyzirc(&mut converted).is_ok());
        assert!(xyzi_to_xyzirc(&mut converted).is_err(), "already XYZIRC");
        let mut short = xyzi(&[[1.0, 2.0, 3.0, 4.0]]);
        short.width = 2;
        assert!(xyzi_to_xyzirc(&mut short).is_err(), "one point of data for two");
    }
}
