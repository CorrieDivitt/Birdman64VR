//! Head pose and field of view → matrices. Column-major 4×4 (`m[col][row]`,
//! what a WGSL `mat4x4<f32>` uniform holds), column vectors, right-handed
//! with -z forward and +y up (OpenXR's convention), clip depth 0..1 (wgpu).

pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// `a * b`: applies `b` first, then `a`.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut m = [[0.0; 4]; 4];
    for (c, col) in m.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    m
}

/// `m * v`.
pub fn transform(m: &Mat4, v: [f32; 4]) -> [f32; 4] {
    [0, 1, 2, 3].map(|r| (0..4).map(|k| m[k][r] * v[k]).sum())
}

/// Scale by `s`, then move by `t`.
pub fn translate_scale(t: [f32; 3], s: [f32; 3]) -> Mat4 {
    [
        [s[0], 0.0, 0.0, 0.0],
        [0.0, s[1], 0.0, 0.0],
        [0.0, 0.0, s[2], 0.0],
        [t[0], t[1], t[2], 1.0],
    ]
}

/// World → eye: the inverse of the eye's pose (`orientation` = unit
/// quaternion x, y, z, w; `position` in metres).
pub fn view(orientation: [f32; 4], position: [f32; 3]) -> Mat4 {
    let [x, y, z, w] = orientation;
    // Rotation matrix columns of the pose (eye → world).
    let r = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + z * w),
            2.0 * (x * z - y * w),
        ],
        [
            2.0 * (x * y - z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + x * w),
        ],
        [
            2.0 * (x * z + y * w),
            2.0 * (y * z - x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    // Inverse: transposed rotation, and -Rᵀ·p as the translation.
    let p = position;
    let t = [0, 1, 2].map(|i| -(r[i][0] * p[0] + r[i][1] * p[1] + r[i][2] * p[2]));
    [
        [r[0][0], r[1][0], r[2][0], 0.0],
        [r[0][1], r[1][1], r[2][1], 0.0],
        [r[0][2], r[1][2], r[2][2], 0.0],
        [t[0], t[1], t[2], 1.0],
    ]
}

/// Eye → clip for an asymmetric field of view (angles in radians as OpenXR
/// gives them: left and down are negative), depth 0 at `near`, 1 at `far`.
pub fn projection(left: f32, right: f32, up: f32, down: f32, near: f32, far: f32) -> Mat4 {
    let (l, r, u, d) = (left.tan(), right.tan(), up.tan(), down.tan());
    [
        [2.0 / (r - l), 0.0, 0.0, 0.0],
        [0.0, 2.0 / (u - d), 0.0, 0.0],
        [
            (r + l) / (r - l),
            (u + d) / (u - d),
            far / (near - far),
            -1.0,
        ],
        [0.0, 0.0, near * far / (near - far), 0.0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ndc(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
        let c = transform(m, [p[0], p[1], p[2], 1.0]);
        [c[0] / c[3], c[1] / c[3], c[2] / c[3]]
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    const Q_IDENT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    #[test]
    fn symmetric_projection_maps_the_frustum_to_the_clip_box() {
        let a = 45f32.to_radians();
        let p = projection(-a, a, a, -a, 0.1, 100.0);
        let centre = ndc(&p, [0.0, 0.0, -2.0]);
        assert!(close(centre[0], 0.0) && close(centre[1], 0.0));
        assert!(centre[2] > 0.0 && centre[2] < 1.0);
        // 45 degrees to the right and up is the top-right corner.
        let corner = ndc(&p, [2.0, 2.0, -2.0]);
        assert!(close(corner[0], 1.0) && close(corner[1], 1.0));
        assert!(close(ndc(&p, [0.0, 0.0, -0.1])[2], 0.0));
        assert!(close(ndc(&p, [0.0, 0.0, -100.0])[2], 1.0));
    }

    #[test]
    fn asymmetric_projection_keeps_straight_ahead_off_centre() {
        // A left eye sees further left than right: straight ahead lands
        // right of the image centre.
        let p = projection(-0.9, 0.7, 0.8, -0.8, 0.1, 100.0);
        assert!(ndc(&p, [0.0, 0.0, -1.0])[0] > 0.0);
        let edge = ndc(&p, [0.7f32.tan(), 0.0, -1.0]);
        assert!(close(edge[0], 1.0));
    }

    #[test]
    fn view_undoes_the_pose() {
        // Eye 1 m to the right: a point straight ahead of the origin is to
        // its left.
        let v = view(Q_IDENT, [1.0, 0.0, 0.0]);
        let p = transform(&v, [0.0, 0.0, -2.0, 1.0]);
        assert!(close(p[0], -1.0) && close(p[2], -2.0));
        // Head turned 90 degrees left (about +y): what was on the left
        // (-x) is now straight ahead (-z).
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let v = view([0.0, s, 0.0, s], [0.0, 0.0, 0.0]);
        let p = transform(&v, [-3.0, 0.0, 0.0, 1.0]);
        assert!(close(p[0], 0.0) && close(p[2], -3.0));
    }

    #[test]
    fn mul_applies_the_right_factor_first() {
        let t = translate_scale([1.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let s = translate_scale([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        // Scale, then translate: (1,0,0) → (2,0,0) → (3,0,0).
        let p = transform(&mul(&t, &s), [1.0, 0.0, 0.0, 1.0]);
        assert!(close(p[0], 3.0));
        assert_eq!(mul(&IDENTITY, &t), t);
    }
}
