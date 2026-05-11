use ros_z_msgs::geometry_msgs::{Quaternion, Transform, TransformStamped, Vector3};
use ros_z_msgs::std_msgs::Header;

pub fn identity_transform() -> Transform {
    Transform {
        translation: Vector3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        rotation: Quaternion {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
    }
}

pub fn quaternion_dot(a: &Quaternion, b: &Quaternion) -> f64 {
    a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w
}

pub fn quaternion_negate(q: &Quaternion) -> Quaternion {
    Quaternion {
        x: -q.x,
        y: -q.y,
        z: -q.z,
        w: -q.w,
    }
}

pub fn quaternion_normalize(q: &Quaternion) -> Quaternion {
    let norm = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    if norm < f64::EPSILON {
        return Quaternion {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
    }
    Quaternion {
        x: q.x / norm,
        y: q.y / norm,
        z: q.z / norm,
        w: q.w / norm,
    }
}

pub fn quaternion_conjugate(q: &Quaternion) -> Quaternion {
    Quaternion {
        x: -q.x,
        y: -q.y,
        z: -q.z,
        w: q.w,
    }
}

/// Quaternion product: lhs * rhs.
/// Applying rhs rotation first, then lhs.
pub fn quaternion_multiply(lhs: &Quaternion, rhs: &Quaternion) -> Quaternion {
    Quaternion {
        x: lhs.w * rhs.x + lhs.x * rhs.w + lhs.y * rhs.z - lhs.z * rhs.y,
        y: lhs.w * rhs.y - lhs.x * rhs.z + lhs.y * rhs.w + lhs.z * rhs.x,
        z: lhs.w * rhs.z + lhs.x * rhs.y - lhs.y * rhs.x + lhs.z * rhs.w,
        w: lhs.w * rhs.w - lhs.x * rhs.x - lhs.y * rhs.y - lhs.z * rhs.z,
    }
}

/// Rotate vector v by quaternion q: q * v * q_conjugate.
pub fn rotate_vector(q: &Quaternion, v: &Vector3) -> Vector3 {
    let tx = 2.0 * (q.y * v.z - q.z * v.y);
    let ty = 2.0 * (q.z * v.x - q.x * v.z);
    let tz = 2.0 * (q.x * v.y - q.y * v.x);
    Vector3 {
        x: v.x + q.w * tx + q.y * tz - q.z * ty,
        y: v.y + q.w * ty + q.z * tx - q.x * tz,
        z: v.z + q.w * tz + q.x * ty - q.y * tx,
    }
}

/// Spherical linear interpolation between two quaternions.
/// t=0 returns q0, t=1 returns q1.
pub fn slerp(q0: &Quaternion, q1: &Quaternion, t: f64) -> Quaternion {
    let mut dot = quaternion_dot(q0, q1);
    // Ensure shortest path
    let q1_eff = if dot < 0.0 {
        dot = -dot;
        quaternion_negate(q1)
    } else {
        *q1
    };
    // For very close quaternions use normalised linear interpolation
    if dot > 0.9995 {
        let interp = Quaternion {
            x: q0.x + t * (q1_eff.x - q0.x),
            y: q0.y + t * (q1_eff.y - q0.y),
            z: q0.z + t * (q1_eff.z - q0.z),
            w: q0.w + t * (q1_eff.w - q0.w),
        };
        return quaternion_normalize(&interp);
    }
    let theta0 = dot.acos();
    let sin_theta0 = theta0.sin();
    let s0 = ((1.0 - t) * theta0).sin() / sin_theta0;
    let s1 = (t * theta0).sin() / sin_theta0;
    Quaternion {
        x: s0 * q0.x + s1 * q1_eff.x,
        y: s0 * q0.y + s1 * q1_eff.y,
        z: s0 * q0.z + s1 * q1_eff.z,
        w: s0 * q0.w + s1 * q1_eff.w,
    }
}

/// Linear interpolation of translations.
pub fn lerp_vector3(v0: &Vector3, v1: &Vector3, t: f64) -> Vector3 {
    Vector3 {
        x: v0.x + t * (v1.x - v0.x),
        y: v0.y + t * (v1.y - v0.y),
        z: v0.z + t * (v1.z - v0.z),
    }
}

/// Interpolate between two transforms at factor t (0=a, 1=b).
pub fn interpolate_transforms(a: &Transform, b: &Transform, t: f64) -> Transform {
    Transform {
        translation: lerp_vector3(&a.translation, &b.translation, t),
        rotation: slerp(&a.rotation, &b.rotation, t),
    }
}

/// compose_transforms(a, b): apply a first, then b.
/// Equivalent to: point_out = b.rotation * (a.rotation * point + a.translation) + b.translation
pub fn compose_transforms(a: &Transform, b: &Transform) -> Transform {
    Transform {
        translation: Vector3 {
            x: rotate_vector(&b.rotation, &a.translation).x + b.translation.x,
            y: rotate_vector(&b.rotation, &a.translation).y + b.translation.y,
            z: rotate_vector(&b.rotation, &a.translation).z + b.translation.z,
        },
        rotation: quaternion_multiply(&b.rotation, &a.rotation),
    }
}

/// Compose two `TransformStamped` values and wrap the result with new frame labels.
///
/// The result represents `T(target_frame ← source_frame)`:
/// `T(target ← fixed) ∘ T(fixed ← source)`.
/// The result's stamp is taken from `t2` (the target side).
pub fn compose_stamped(
    t2: TransformStamped,
    t1: TransformStamped,
    target_frame: &str,
    source_frame: &str,
) -> TransformStamped {
    TransformStamped {
        header: Header {
            frame_id: target_frame.to_string(),
            stamp: t2.header.stamp,
        },
        child_frame_id: source_frame.to_string(),
        transform: compose_transforms(&t1.transform, &t2.transform),
    }
}

/// Invert a transform: T^-1 such that compose(T, T^-1) = identity.
pub fn invert_transform(t: &Transform) -> Transform {
    let rot_inv = quaternion_conjugate(&t.rotation);
    let trans_inv = rotate_vector(
        &rot_inv,
        &Vector3 {
            x: -t.translation.x,
            y: -t.translation.y,
            z: -t.translation.z,
        },
    );
    Transform {
        translation: trans_inv,
        rotation: rot_inv,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-10
    }

    fn quat_approx_eq(a: &Quaternion, b: &Quaternion) -> bool {
        // q and -q represent the same rotation
        let same = approx_eq(a.x, b.x)
            && approx_eq(a.y, b.y)
            && approx_eq(a.z, b.z)
            && approx_eq(a.w, b.w);
        let neg = approx_eq(a.x, -b.x)
            && approx_eq(a.y, -b.y)
            && approx_eq(a.z, -b.z)
            && approx_eq(a.w, -b.w);
        same || neg
    }

    fn vec_approx_eq(a: &Vector3, b: &Vector3) -> bool {
        approx_eq(a.x, b.x) && approx_eq(a.y, b.y) && approx_eq(a.z, b.z)
    }

    fn identity_quat() -> Quaternion {
        Quaternion {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }

    // Unit quaternion for 90° rotation around Z: (0, 0, sin(45°), cos(45°))
    fn q_90z() -> Quaternion {
        let h = std::f64::consts::FRAC_PI_4; // π/4 = half-angle for 90° rotation
        Quaternion {
            x: 0.0,
            y: 0.0,
            z: h.sin(),
            w: h.cos(),
        }
    }

    #[test]
    fn slerp_at_t0_returns_q0() {
        let q0 = Quaternion {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
        let r = slerp(&q0, &q_90z(), 0.0);
        assert!(quat_approx_eq(&r, &q0));
    }

    #[test]
    fn slerp_at_t1_returns_q1() {
        let q1 = q_90z();
        let q0 = Quaternion {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
        let r = slerp(&q0, &q1, 1.0);
        assert!(quat_approx_eq(&r, &q1));
    }

    #[test]
    fn slerp_at_midpoint_is_normalized() {
        let q0 = Quaternion {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
        let r = slerp(&q0, &q_90z(), 0.5);
        let norm = (r.x * r.x + r.y * r.y + r.z * r.z + r.w * r.w).sqrt();
        assert!(approx_eq(norm, 1.0));
    }

    #[test]
    fn compose_with_identity_is_noop() {
        let id = identity_transform();
        let t = Transform {
            translation: Vector3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            rotation: Quaternion {
                x: 0.0,
                y: 0.0,
                z: 0.5_f64.sqrt(),
                w: 0.5_f64.sqrt(),
            },
        };
        let r = compose_transforms(&t, &id);
        assert!(vec_approx_eq(&r.translation, &t.translation));
        assert!(quat_approx_eq(&r.rotation, &t.rotation));
    }

    #[test]
    fn compose_identity_with_t_is_t() {
        let id = identity_transform();
        let t = Transform {
            translation: Vector3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            rotation: Quaternion {
                x: 0.0,
                y: 0.0,
                z: 0.5_f64.sqrt(),
                w: 0.5_f64.sqrt(),
            },
        };
        let r = compose_transforms(&id, &t);
        assert!(vec_approx_eq(&r.translation, &t.translation));
        assert!(quat_approx_eq(&r.rotation, &t.rotation));
    }

    #[test]
    fn compose_then_invert_is_identity() {
        let t = Transform {
            translation: Vector3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            rotation: Quaternion {
                x: 0.0,
                y: 0.0,
                z: 0.5_f64.sqrt(),
                w: 0.5_f64.sqrt(),
            },
        };
        let result = compose_transforms(&t, &invert_transform(&t));
        let id = identity_transform();
        assert!(vec_approx_eq(&result.translation, &id.translation));
        assert!(quat_approx_eq(&result.rotation, &id.rotation));
    }

    #[test]
    fn rotate_vector_with_identity_is_noop() {
        let q = identity_quat();
        let v = Vector3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };
        let r = rotate_vector(&q, &v);
        assert!(vec_approx_eq(&r, &v));
    }

    #[test]
    fn rotate_vector_90_degrees_around_z() {
        // 90° rotation around Z: x→y, y→-x
        let angle = std::f64::consts::PI / 2.0;
        let q = Quaternion {
            x: 0.0,
            y: 0.0,
            z: (angle / 2.0).sin(),
            w: (angle / 2.0).cos(),
        };
        let v = Vector3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        };
        let r = rotate_vector(&q, &v);
        assert!(vec_approx_eq(
            &r,
            &Vector3 {
                x: 0.0,
                y: 1.0,
                z: 0.0
            }
        ));
    }

    #[test]
    fn compose_translations_add() {
        let a = Transform {
            translation: Vector3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
            rotation: Quaternion {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
        };
        let b = Transform {
            translation: Vector3 {
                x: 2.0,
                y: 0.0,
                z: 0.0,
            },
            rotation: Quaternion {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
        };
        let r = compose_transforms(&a, &b);
        assert!(vec_approx_eq(
            &r.translation,
            &Vector3 {
                x: 3.0,
                y: 0.0,
                z: 0.0
            }
        ));
    }
}
