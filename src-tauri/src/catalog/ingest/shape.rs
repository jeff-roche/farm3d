use crate::catalog::{BedShape, PointMm};

#[derive(Debug, PartialEq)]
pub struct ShapeError(pub String);

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Parses OrcaSlicer's `printable_area` list of "Xx Y" strings (e.g. "0x0",
/// "256x0", "256x256", "0x256") into a BedShape. Exactly four points that form
/// an axis-aligned bounding box become Rectangular; anything else (deltas,
/// circular beds, cut corners) becomes Polygon.
pub fn parse_printable_area(points: &[String]) -> Result<BedShape, ShapeError> {
    let parsed: Vec<PointMm> = points
        .iter()
        .map(|p| {
            let (x, y) = p
                .split_once('x')
                .ok_or_else(|| ShapeError(format!("malformed point: {p:?}")))?;
            Ok(PointMm {
                x_mm: x
                    .parse()
                    .map_err(|_| ShapeError(format!("bad x in {p:?}")))?,
                y_mm: y
                    .parse()
                    .map_err(|_| ShapeError(format!("bad y in {p:?}")))?,
            })
        })
        .collect::<Result<_, ShapeError>>()?;

    if parsed.is_empty() {
        return Err(ShapeError("printable_area has no points".to_string()));
    }
    if let Some(rect) = as_axis_aligned_rectangle(&parsed) {
        return Ok(rect);
    }
    Ok(BedShape::Polygon { points: parsed })
}

fn as_axis_aligned_rectangle(points: &[PointMm]) -> Option<BedShape> {
    if points.len() != 4 {
        return None;
    }
    let min_x = points.iter().map(|p| p.x_mm).fold(f64::INFINITY, f64::min);
    let max_x = points
        .iter()
        .map(|p| p.x_mm)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = points.iter().map(|p| p.y_mm).fold(f64::INFINITY, f64::min);
    let max_y = points
        .iter()
        .map(|p| p.y_mm)
        .fold(f64::NEG_INFINITY, f64::max);

    let is_corner =
        |p: &PointMm| (p.x_mm == min_x || p.x_mm == max_x) && (p.y_mm == min_y || p.y_mm == max_y);
    if !points.iter().all(is_corner) {
        return None;
    }
    let mut corners: Vec<(i64, i64)> = points
        .iter()
        .map(|p| {
            (
                (p.x_mm * 1000.0).round() as i64,
                (p.y_mm * 1000.0).round() as i64,
            )
        })
        .collect();
    corners.sort_unstable();
    corners.dedup();
    if corners.len() != 4 {
        return None;
    }

    Some(BedShape::Rectangular {
        width_mm: max_x - min_x,
        depth_mm: max_y - min_y,
        origin_x_mm: min_x,
        origin_y_mm: min_y,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_point_box_becomes_rectangular() {
        let points = ["0x0", "256x0", "256x256", "0x256"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let shape = parse_printable_area(&points).unwrap();
        assert_eq!(
            shape,
            BedShape::Rectangular {
                width_mm: 256.0,
                depth_mm: 256.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            }
        );
    }

    #[test]
    fn offset_rectangle_captures_origin() {
        let points = ["10x0", "266x0", "266x256", "10x256"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let shape = parse_printable_area(&points).unwrap();
        assert_eq!(
            shape,
            BedShape::Rectangular {
                width_mm: 256.0,
                depth_mm: 256.0,
                origin_x_mm: 10.0,
                origin_y_mm: 0.0,
            }
        );
    }

    #[test]
    fn non_rectangular_shape_becomes_polygon() {
        // A coarse octagon approximation, as delta/round beds use.
        let raw = ["150x0", "106x44", "44x106", "0x150", "0x0"];
        let points = raw.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let shape = parse_printable_area(&points).unwrap();
        match shape {
            BedShape::Polygon { points: parsed } => assert_eq!(parsed.len(), 5),
            other => panic!("expected Polygon, got {other:?}"),
        }
    }

    #[test]
    fn malformed_point_is_an_error() {
        let points = vec!["not-a-point".to_string()];
        assert!(parse_printable_area(&points).is_err());
    }

    #[test]
    fn empty_list_is_an_error() {
        assert!(parse_printable_area(&[]).is_err());
    }

    #[test]
    fn rectangular_serializes_fields_as_camel_case() {
        let shape = BedShape::Rectangular {
            width_mm: 256.0,
            depth_mm: 256.0,
            origin_x_mm: 10.0,
            origin_y_mm: 20.0,
        };
        let json = serde_json::to_string(&shape).unwrap();
        // Fields should be in camelCase (widthMm, depthMm, originXMm, originYMm)
        // not snake_case (width_mm, depth_mm, origin_x_mm, origin_y_mm)
        assert!(
            json.contains("\"widthMm\""),
            "missing camelCase widthMm in: {json}"
        );
        assert!(
            json.contains("\"depthMm\""),
            "missing camelCase depthMm in: {json}"
        );
        assert!(
            json.contains("\"originXMm\""),
            "missing camelCase originXMm in: {json}"
        );
        assert!(
            json.contains("\"originYMm\""),
            "missing camelCase originYMm in: {json}"
        );
        assert!(
            !json.contains("\"width_mm\""),
            "found snake_case width_mm in: {json}"
        );
        assert!(
            !json.contains("\"depth_mm\""),
            "found snake_case depth_mm in: {json}"
        );
        assert!(
            !json.contains("\"origin_x_mm\""),
            "found snake_case origin_x_mm in: {json}"
        );
        assert!(
            !json.contains("\"origin_y_mm\""),
            "found snake_case origin_y_mm in: {json}"
        );
    }
}
