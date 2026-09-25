//! D6: farm3d's own 3D convex hull (quickhull), for the lay-flat faces.
//!
//! [`hull_faces`] returns the hull's planar faces with their outward unit
//! normals and areas. Coplanar hull triangles are merged, so a cube gives
//! six faces rather than twelve triangles.
//!
//! Robustness:
//! - Points are deduplicated by bit pattern first. A welded mesh's vertices
//!   are already distinct, but a flattened 3MF may repeat them.
//! - Distances use the usual quickhull tolerance: `3 · ε · (max|x| + max|y|
//!   + max|z|)`. Points within it of a face count as on the face.
//! - Fewer than three distinct points, or collinear points, have no faces.
//!   Coplanar points give two faces (both sides) whose area is the area of
//!   their 2D hull.
//! - A step whose horizon isn't a simple loop (a numerical corner case)
//!   skips that point instead of corrupting the hull.

use std::collections::{HashMap, HashSet};

/// One planar face of a convex hull.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HullFace {
    /// The outward unit normal.
    pub normal: [f64; 3],
    /// The face's area, in the square of the input unit.
    pub area: f64,
}

/// Hull triangles whose normals are within this angle of a face's first
/// triangle are merged into that face.
const MERGE_ANGLE_RAD: f64 = 1e-4;

/// The convex hull of `points` as merged planar faces, largest first. Ties
/// are ordered by normal so the result is deterministic.
pub fn hull_faces(points: impl IntoIterator<Item = [f64; 3]>) -> Vec<HullFace> {
    let points = distinct(points);
    if points.len() < 3 {
        return Vec::new();
    }
    let eps = tolerance(&points);
    let mut faces = match initial_simplex(&points, eps) {
        Simplex::Tetrahedron(corners) => Hull::build(&points, corners, eps).merged_faces(),
        Simplex::Planar(normal, origin, along) => planar_faces(&points, normal, origin, along),
        Simplex::Degenerate => Vec::new(),
    };
    faces.sort_by(|a, b| {
        b.area
            .total_cmp(&a.area)
            .then_with(|| compare_vectors(&a.normal, &b.normal))
    });
    faces
}

fn compare_vectors(a: &[f64; 3], b: &[f64; 3]) -> std::cmp::Ordering {
    a[0].total_cmp(&b[0])
        .then_with(|| a[1].total_cmp(&b[1]))
        .then_with(|| a[2].total_cmp(&b[2]))
}

/// The distinct points, by bit pattern once `-0.0` is folded into `0.0`,
/// in first-seen order. Non-finite points are dropped.
fn distinct(points: impl IntoIterator<Item = [f64; 3]>) -> Vec<[f64; 3]> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for point in points {
        if !point.iter().all(|value| value.is_finite()) {
            continue;
        }
        let point = point.map(|value| value + 0.0);
        if seen.insert(point.map(f64::to_bits)) {
            out.push(point);
        }
    }
    out
}

fn tolerance(points: &[[f64; 3]]) -> f64 {
    let mut max_abs = [0.0f64; 3];
    for point in points {
        for axis in 0..3 {
            max_abs[axis] = max_abs[axis].max(point[axis].abs());
        }
    }
    3.0 * f64::EPSILON * (max_abs[0] + max_abs[1] + max_abs[2])
}

// --- Vector helpers ---------------------------------------------------------

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f64; 3], factor: f64) -> [f64; 3] {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// `a` scaled to unit length, or zero when it has no length.
fn normalize(a: [f64; 3]) -> [f64; 3] {
    let len = length(a);
    if len > 0.0 {
        scale(a, 1.0 / len)
    } else {
        [0.0; 3]
    }
}

// --- The starting simplex ---------------------------------------------------

enum Simplex {
    Tetrahedron([usize; 4]),
    /// Every point lies in the plane through `origin` with unit `normal`;
    /// `along` is a unit direction in that plane.
    Planar([f64; 3], [f64; 3], [f64; 3]),
    Degenerate,
}

fn initial_simplex(points: &[[f64; 3]], eps: f64) -> Simplex {
    // The two farthest-apart of the six axis extremes.
    let mut extremes = [0usize; 6];
    for (index, point) in points.iter().enumerate() {
        for axis in 0..3 {
            if point[axis] < points[extremes[axis * 2]][axis] {
                extremes[axis * 2] = index;
            }
            if point[axis] > points[extremes[axis * 2 + 1]][axis] {
                extremes[axis * 2 + 1] = index;
            }
        }
    }
    let mut best = (0.0, 0, 0);
    for &i in &extremes {
        for &j in &extremes {
            let distance = length(sub(points[i], points[j]));
            if distance > best.0 {
                best = (distance, i, j);
            }
        }
    }
    let (spread, a, b) = best;
    if spread <= eps {
        return Simplex::Degenerate;
    }

    // The point farthest from the line ab.
    let direction = normalize(sub(points[b], points[a]));
    let (line_distance, c) = farthest(points, |point| {
        length(cross(sub(point, points[a]), direction))
    });
    if line_distance <= eps {
        return Simplex::Degenerate;
    }

    // The point farthest from the plane abc.
    let normal = normalize(cross(sub(points[b], points[a]), sub(points[c], points[a])));
    let (plane_distance, d) = farthest(points, |point| dot(normal, sub(point, points[a])).abs());
    if plane_distance <= eps {
        return Simplex::Planar(normal, points[a], direction);
    }
    Simplex::Tetrahedron([a, b, c, d])
}

fn farthest(points: &[[f64; 3]], distance: impl Fn([f64; 3]) -> f64) -> (f64, usize) {
    points
        .iter()
        .enumerate()
        .fold((0.0, 0), |best, (index, point)| {
            let value = distance(*point);
            if value > best.0 {
                (value, index)
            } else {
                best
            }
        })
}

// --- Planar input -----------------------------------------------------------

/// Both sides of a flat point set: the area of its 2D hull, facing up and
/// down along `normal`.
fn planar_faces(
    points: &[[f64; 3]],
    normal: [f64; 3],
    origin: [f64; 3],
    along: [f64; 3],
) -> Vec<HullFace> {
    let across = cross(normal, along);
    let mut flat: Vec<[f64; 2]> = points
        .iter()
        .map(|point| {
            let offset = sub(*point, origin);
            [dot(offset, along), dot(offset, across)]
        })
        .collect();
    let area = hull_area_2d(&mut flat);
    if area <= 0.0 {
        return Vec::new();
    }
    vec![
        HullFace { normal, area },
        HullFace {
            normal: scale(normal, -1.0),
            area,
        },
    ]
}

/// The area of the 2D convex hull (Andrew's monotone chain).
fn hull_area_2d(points: &mut [[f64; 2]]) -> f64 {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then_with(|| a[1].total_cmp(&b[1])));
    let turn = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut hull: Vec<[f64; 2]> = Vec::with_capacity(points.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let ordered: Box<dyn Iterator<Item = &[f64; 2]>> = if pass == 0 {
            Box::new(points.iter())
        } else {
            Box::new(points.iter().rev())
        };
        for &point in ordered {
            while hull.len() >= start + 2
                && turn(hull[hull.len() - 2], hull[hull.len() - 1], point) <= 0.0
            {
                hull.pop();
            }
            hull.push(point);
        }
        hull.pop();
    }
    let mut twice_area = 0.0;
    for (index, a) in hull.iter().enumerate() {
        let b = hull[(index + 1) % hull.len()];
        twice_area += a[0] * b[1] - b[0] * a[1];
    }
    twice_area.abs() / 2.0
}

// --- Quickhull --------------------------------------------------------------

struct Face {
    /// Counter-clockwise seen from outside.
    vertices: [usize; 3],
    /// `neighbors[i]` shares the edge `vertices[i] → vertices[i + 1]`.
    neighbors: [usize; 3],
    normal: [f64; 3],
    offset: f64,
    outside: Vec<usize>,
    alive: bool,
}

impl Face {
    fn distance(&self, point: [f64; 3]) -> f64 {
        dot(self.normal, point) - self.offset
    }

    /// The index of the edge `from → to`, if this face has it.
    fn edge(&self, from: usize, to: usize) -> Option<usize> {
        (0..3).find(|&i| self.vertices[i] == from && self.vertices[(i + 1) % 3] == to)
    }
}

struct Hull<'p> {
    points: &'p [[f64; 3]],
    eps: f64,
    faces: Vec<Face>,
}

impl<'p> Hull<'p> {
    fn build(points: &'p [[f64; 3]], [a, b, c, d]: [usize; 4], eps: f64) -> Self {
        let mut hull = Hull {
            points,
            eps,
            faces: Vec::new(),
        };
        let centroid = scale(
            add(add(points[a], points[b]), add(points[c], points[d])),
            0.25,
        );
        for triangle in [[a, b, c], [a, b, d], [a, c, d], [b, c, d]] {
            let mut face = hull.new_face(triangle);
            if face.distance(centroid) > 0.0 {
                face = hull.new_face([triangle[0], triangle[2], triangle[1]]);
            }
            hull.faces.push(face);
        }
        // Each directed edge's twin is the reversed edge of another face.
        let mut edges = HashMap::new();
        for (index, face) in hull.faces.iter().enumerate() {
            for i in 0..3 {
                edges.insert((face.vertices[i], face.vertices[(i + 1) % 3]), index);
            }
        }
        for index in 0..4 {
            for i in 0..3 {
                let face = &hull.faces[index];
                let twin = edges[&(face.vertices[(i + 1) % 3], face.vertices[i])];
                hull.faces[index].neighbors[i] = twin;
            }
        }

        let simplex = [a, b, c, d];
        for point in 0..points.len() {
            if !simplex.contains(&point) {
                hull.assign(point, 0..4);
            }
        }
        hull.expand();
        hull
    }

    fn new_face(&self, vertices: [usize; 3]) -> Face {
        let [a, b, c] = vertices.map(|index| self.points[index]);
        let normal = normalize(cross(sub(b, a), sub(c, a)));
        Face {
            vertices,
            neighbors: [usize::MAX; 3],
            normal,
            offset: dot(normal, a),
            outside: Vec::new(),
            alive: true,
        }
    }

    /// Adds `point` to the outside set of the first face in `candidates`
    /// that it is above. A point above none of them is inside the hull.
    fn assign(&mut self, point: usize, candidates: impl IntoIterator<Item = usize>) {
        for face in candidates {
            if self.faces[face].distance(self.points[point]) > self.eps {
                self.faces[face].outside.push(point);
                return;
            }
        }
    }

    fn expand(&mut self) {
        let mut pending: Vec<usize> = (0..self.faces.len()).collect();
        // `visited[face] == step`: the face is visible from this step's eye.
        let mut visited: Vec<usize> = vec![usize::MAX; self.faces.len()];
        // `vertex_stamp[point] == step`: the point starts a horizon edge.
        let mut vertex_stamp: Vec<usize> = vec![usize::MAX; self.points.len()];
        let mut visible: Vec<usize> = Vec::new();
        // The horizon as a counter-clockwise loop of edges (from, to, the
        // face beyond it), each edge's `to` the next edge's `from`.
        let mut horizon: Vec<(usize, usize, usize)> = Vec::new();
        // The DFS: (face, the edge index it was entered by, edges done).
        let mut stack: Vec<(usize, Option<usize>, usize)> = Vec::new();
        let mut step = 0usize;
        while let Some(start) = pending.pop() {
            if !self.faces[start].alive || self.faces[start].outside.is_empty() {
                continue;
            }
            step += 1;
            let start_face = &self.faces[start];
            let eye = *start_face
                .outside
                .iter()
                .max_by(|&&i, &&j| {
                    start_face
                        .distance(self.points[i])
                        .total_cmp(&start_face.distance(self.points[j]))
                })
                .expect("the outside set is not empty");
            let eye_point = self.points[eye];

            // The standard quickhull horizon walk: a DFS over the faces the
            // eye sees, crossing each face's edges in counter-clockwise order
            // from the one it was entered by, so the edges to faces the eye
            // doesn't see come out as an ordered loop.
            visible.clear();
            horizon.clear();
            visited[start] = step;
            visible.push(start);
            stack.push((start, None, 0));
            while let Some((face, entered_by, done)) = stack.pop() {
                let edge_count = if entered_by.is_some() { 2 } else { 3 };
                if done == edge_count {
                    continue;
                }
                stack.push((face, entered_by, done + 1));
                let edge = match entered_by {
                    Some(entry) => (entry + 1 + done) % 3,
                    None => done,
                };
                let neighbor = self.faces[face].neighbors[edge];
                if visited[neighbor] == step {
                    continue;
                }
                let vertices = self.faces[face].vertices;
                let (from, to) = (vertices[edge], vertices[(edge + 1) % 3]);
                if self.faces[neighbor].distance(eye_point) > self.eps {
                    visited[neighbor] = step;
                    visible.push(neighbor);
                    let back = self.faces[neighbor]
                        .edge(to, from)
                        .expect("neighbours share the edge");
                    stack.push((neighbor, Some(back), 0));
                } else {
                    horizon.push((from, to, neighbor));
                }
            }

            // A simple loop, checked in O(h): consecutive edges join, and no
            // vertex starts two edges.
            let joined = horizon
                .iter()
                .zip(horizon.iter().cycle().skip(1))
                .all(|(edge, next)| edge.1 == next.0);
            let distinct = horizon.iter().all(|&(from, _, _)| {
                let fresh = vertex_stamp[from] != step;
                vertex_stamp[from] = step;
                fresh
            });
            if horizon.len() < 3 || !joined || !distinct {
                // A numerical corner case: treat the eye as on the hull.
                self.faces[start].outside.retain(|&point| point != eye);
                pending.push(start);
                continue;
            }

            // Cone the horizon to the eye. New face k borders the horizon
            // face on edge 0, face k + 1 on edge 1, and face k - 1 on edge 2.
            let first_new = self.faces.len();
            let count = horizon.len();
            for (offset, &(from, to, outer)) in horizon.iter().enumerate() {
                let mut face = self.new_face([from, to, eye]);
                face.neighbors = [
                    outer,
                    first_new + (offset + 1) % count,
                    first_new + (offset + count - 1) % count,
                ];
                let back = self.faces[outer]
                    .edge(to, from)
                    .expect("the horizon face shares the edge");
                self.faces[outer].neighbors[back] = first_new + offset;
                self.faces.push(face);
            }
            visited.resize(self.faces.len(), usize::MAX);

            // Hand the visible faces' points to the new faces.
            let new_faces = first_new..self.faces.len();
            for &face in &visible {
                self.faces[face].alive = false;
                let orphans = std::mem::take(&mut self.faces[face].outside);
                for point in orphans {
                    if point != eye {
                        self.assign(point, new_faces.clone());
                    }
                }
            }
            pending.extend(new_faces);
        }
    }

    /// The live triangles grouped into planar faces: a face grows across
    /// neighbours whose normals are within [`MERGE_ANGLE_RAD`] of its first
    /// triangle's.
    fn merged_faces(&self) -> Vec<HullFace> {
        let limit = MERGE_ANGLE_RAD.cos();
        let mut grouped = vec![false; self.faces.len()];
        let mut out = Vec::new();
        for seed in 0..self.faces.len() {
            if grouped[seed] || !self.faces[seed].alive {
                continue;
            }
            grouped[seed] = true;
            let seed_normal = self.faces[seed].normal;
            let mut members = vec![seed];
            let mut area_vector = [0.0; 3];
            let mut area = 0.0;
            while let Some(face) = members.pop() {
                let [a, b, c] = self.faces[face].vertices.map(|index| self.points[index]);
                let doubled = cross(sub(b, a), sub(c, a));
                area_vector = add(area_vector, doubled);
                area += length(doubled) / 2.0;
                for neighbor in self.faces[face].neighbors {
                    if !grouped[neighbor] && dot(self.faces[neighbor].normal, seed_normal) >= limit
                    {
                        grouped[neighbor] = true;
                        members.push(neighbor);
                    }
                }
            }
            if area > 0.0 {
                out.push(HullFace {
                    normal: normalize(area_vector),
                    area,
                });
            }
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The corners of an axis-aligned box.
    pub(crate) fn box_corners(min: [f64; 3], max: [f64; 3]) -> Vec<[f64; 3]> {
        (0..8)
            .map(|corner| {
                [
                    if corner & 1 == 0 { min[0] } else { max[0] },
                    if corner & 2 == 0 { min[1] } else { max[1] },
                    if corner & 4 == 0 { min[2] } else { max[2] },
                ]
            })
            .collect()
    }

    /// The 10 mm cube as 12 triangles, three vertices each (unwelded).
    fn cube_triangle_soup() -> Vec<[f64; 3]> {
        let v = box_corners([0.0; 3], [10.0; 3]);
        let triangles = [
            [0, 2, 3],
            [0, 3, 1],
            [4, 5, 7],
            [4, 7, 6],
            [0, 1, 5],
            [0, 5, 4],
            [2, 6, 7],
            [2, 7, 3],
            [0, 4, 6],
            [0, 6, 2],
            [1, 3, 7],
            [1, 7, 5],
        ];
        triangles
            .iter()
            .flat_map(|triangle| triangle.map(|index| v[index]))
            .collect()
    }

    fn assert_close(actual: f64, expected: f64, tolerance: f64, what: &str) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{what}: {actual} != {expected}"
        );
    }

    fn rotate(point: [f64; 3], [ax, ay, az]: [f64; 3]) -> [f64; 3] {
        let (sx, cx) = ax.to_radians().sin_cos();
        let (sy, cy) = ay.to_radians().sin_cos();
        let (sz, cz) = az.to_radians().sin_cos();
        let [x, y, z] = point;
        let (y, z) = (y * cx - z * sx, y * sx + z * cx);
        let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
        [x * cz - y * sz, x * sz + y * cz, z]
    }

    /// A tiny deterministic generator (xorshift64*), so tests need no crate.
    pub(crate) struct Random(u64);

    impl Random {
        pub(crate) fn new(seed: u64) -> Self {
            Self(seed.max(1))
        }

        /// Uniform in [0, 1).
        pub(crate) fn next(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            let value = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d);
            (value >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    #[test]
    fn a_cube_has_six_faces_merged_from_its_twelve_triangles() {
        let faces = hull_faces(cube_triangle_soup());
        assert_eq!(faces.len(), 6, "{faces:?}");
        let mut normals: Vec<[i64; 3]> = faces
            .iter()
            .map(|face| face.normal.map(|value| value.round() as i64))
            .collect();
        normals.sort();
        assert_eq!(
            normals,
            vec![
                [-1, 0, 0],
                [0, -1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [0, 1, 0],
                [1, 0, 0]
            ]
        );
        for face in &faces {
            assert_close(face.area, 100.0, 1e-9, "cube face area");
            assert_close(length(face.normal), 1.0, 1e-12, "unit normal");
        }
    }

    #[test]
    fn a_tilted_box_keeps_its_faces_and_areas() {
        let angles = [30.0, 45.0, 60.0];
        let corners: Vec<[f64; 3]> = box_corners([0.0; 3], [20.0, 10.0, 5.0])
            .into_iter()
            // f32 storage, as a mesh buffer would hold it.
            .map(|point| rotate(point, angles).map(|value| f64::from(value as f32)))
            .collect();
        let faces = hull_faces(corners);
        assert_eq!(faces.len(), 6, "{faces:?}");
        let expected = [
            (200.0, [0.0, 0.0, 1.0]),
            (200.0, [0.0, 0.0, -1.0]),
            (100.0, [0.0, 1.0, 0.0]),
            (100.0, [0.0, -1.0, 0.0]),
            (50.0, [1.0, 0.0, 0.0]),
            (50.0, [-1.0, 0.0, 0.0]),
        ];
        for (area, normal) in expected {
            let normal = rotate(normal, angles);
            let face = faces
                .iter()
                .find(|face| dot(face.normal, normal) > 0.999_999)
                .unwrap_or_else(|| panic!("no face with normal {normal:?}: {faces:?}"));
            assert_close(face.area, area, 1e-3, "tilted face area");
        }
        // Largest first.
        assert!(faces.windows(2).all(|pair| pair[0].area >= pair[1].area));
    }

    #[test]
    fn interior_and_repeated_points_do_not_change_the_hull() {
        let mut points = cube_triangle_soup();
        let mut random = Random::new(7);
        for _ in 0..500 {
            points.push([
                0.5 + 9.0 * random.next(),
                0.5 + 9.0 * random.next(),
                0.5 + 9.0 * random.next(),
            ]);
        }
        // Points on the faces and edges, coplanar with the hull.
        points.push([5.0, 5.0, 0.0]);
        points.push([5.0, 0.0, 10.0]);
        points.push([10.0, 3.0, 7.0]);
        let faces = hull_faces(points);
        assert_eq!(faces.len(), 6, "{faces:?}");
        for face in &faces {
            assert_close(face.area, 100.0, 1e-9, "face area");
        }
    }

    #[test]
    fn degenerate_input_has_no_faces() {
        assert!(hull_faces([]).is_empty());
        assert!(hull_faces([[1.0, 2.0, 3.0]]).is_empty());
        assert!(hull_faces([[1.0, 2.0, 3.0]; 20]).is_empty());
        assert!(hull_faces([[0.0; 3], [1.0, 1.0, 1.0]]).is_empty());
        let collinear = (0..10).map(|i| [f64::from(i), 2.0 * f64::from(i), 3.0]);
        assert!(hull_faces(collinear).is_empty());
        assert!(hull_faces([[f64::NAN, 0.0, 0.0], [0.0, f64::INFINITY, 0.0]]).is_empty());
    }

    #[test]
    fn coplanar_input_has_two_faces_with_the_flat_area() {
        // A 10 × 4 rectangle with interior points, in the plane z = 2.
        let mut points = vec![
            [0.0, 0.0, 2.0],
            [10.0, 0.0, 2.0],
            [10.0, 4.0, 2.0],
            [0.0, 4.0, 2.0],
        ];
        points.extend((1..9).map(|i| [f64::from(i), 2.0, 2.0]));
        let faces = hull_faces(points);
        assert_eq!(faces.len(), 2, "{faces:?}");
        assert_close(faces[0].area, 40.0, 1e-9, "flat area");
        assert_close(faces[1].area, 40.0, 1e-9, "flat area");
        assert_close(faces[0].normal[2].abs(), 1.0, 1e-12, "flat normal");
        assert_close(
            faces[0].normal[2] + faces[1].normal[2],
            0.0,
            1e-12,
            "opposite normals",
        );
    }

    #[test]
    fn a_sampled_sphere_is_closed_and_near_its_surface_area() {
        let mut random = Random::new(42);
        let points: Vec<[f64; 3]> = (0..4000)
            .map(|_| {
                let z = 2.0 * random.next() - 1.0;
                let angle = std::f64::consts::TAU * random.next();
                let ring = (1.0 - z * z).sqrt();
                [
                    10.0 * ring * angle.cos(),
                    10.0 * ring * angle.sin(),
                    10.0 * z,
                ]
            })
            .collect();
        let faces = hull_faces(points);
        let total: f64 = faces.iter().map(|face| face.area).sum();
        let sphere = 4.0 * std::f64::consts::PI * 100.0;
        assert!(
            total < sphere && total > sphere * 0.98,
            "{total} vs {sphere}"
        );
        // Faces are outward: the area-weighted normals sum to zero.
        let net = faces.iter().fold([0.0; 3], |sum, face| {
            add(sum, scale(face.normal, face.area))
        });
        assert!(length(net) < 1e-6 * total, "{net:?}");
    }

    /// `cargo test --release large_hulls_are_fast -- --ignored --nocapture`
    #[test]
    #[ignore = "timing check for large meshes; run in release"]
    fn large_hulls_are_fast() {
        let mut random = Random::new(3);
        let points: Vec<[f64; 3]> = (0..1_000_000)
            .map(|_| {
                let z = 2.0 * random.next() - 1.0;
                let angle = std::f64::consts::TAU * random.next();
                let ring = (1.0 - z * z).sqrt();
                [
                    50.0 * ring * angle.cos(),
                    50.0 * ring * angle.sin(),
                    50.0 * z,
                ]
            })
            .collect();
        let started = std::time::Instant::now();
        let faces = hull_faces(points);
        let elapsed = started.elapsed();
        eprintln!(
            "1M points on a sphere: {} faces in {elapsed:?}",
            faces.len()
        );
        assert!(elapsed < std::time::Duration::from_secs(10), "{elapsed:?}");

        // A cylinder's rims make long horizons.
        let segments = 20_000;
        let cylinder: Vec<[f64; 3]> = (0..segments)
            .flat_map(|index| {
                let angle = std::f64::consts::TAU * f64::from(index) / f64::from(segments);
                let (y, x) = angle.sin_cos();
                [[50.0 * x, 50.0 * y, 0.0], [50.0 * x, 50.0 * y, 80.0]]
            })
            .collect();
        let started = std::time::Instant::now();
        let faces = hull_faces(cylinder);
        let elapsed = started.elapsed();
        eprintln!(
            "{segments}-segment cylinder: {} faces in {elapsed:?}",
            faces.len()
        );
        let cap = faces
            .iter()
            .find(|face| face.normal[2] > 0.999_999)
            .expect("the top cap is one face");
        let area = std::f64::consts::PI * 2500.0;
        assert!((cap.area - area).abs() < area * 1e-6, "{}", cap.area);
        assert!(elapsed < std::time::Duration::from_secs(10), "{elapsed:?}");
    }
}
