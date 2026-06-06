use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
    pub eye:       [f32; 4],
}

pub struct OrbitCamera {
    pub target:  Vec3,
    pub yaw:     f32,
    pub pitch:   f32,
    pub dist:    f32,
    pub fov_y:   f32,
    pub aspect:  f32,
    pub znear:   f32,
    pub zfar:    f32,
}

impl OrbitCamera {
    pub fn new(aspect: f32) -> Self {
        Self {
            target: Vec3::ZERO,
            yaw:   0.85,
            pitch: 0.38,
            dist:  15.0,
            fov_y: 50.0_f32.to_radians(),
            aspect,
            znear: 0.05,
            zfar:  140.0,
        }
    }

    pub fn eye(&self) -> Vec3 {
        let cp = self.pitch.cos();
        let sp = self.pitch.sin();
        let cy = self.yaw.cos();
        let sy = self.yaw.sin();
        self.target + Vec3::new(self.dist * cp * sy, self.dist * sp, self.dist * cp * cy)
    }

    pub fn uniform(&self) -> CameraUniform {
        let eye = self.eye();
        let view = Mat4::look_at_rh(eye, self.target, Vec3::Y);
        let proj = Mat4::perspective_rh(self.fov_y, self.aspect, self.znear, self.zfar);
        let vp = proj * view;
        CameraUniform {
            view_proj: vp.to_cols_array_2d(),
            eye: [eye.x, eye.y, eye.z, 1.0],
        }
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw   -= dx * 0.005;
        self.pitch  = (self.pitch + dy * 0.005)
            .clamp(-std::f32::consts::FRAC_PI_2 + 0.05, std::f32::consts::FRAC_PI_2 - 0.05);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.dist = (self.dist * factor).clamp(0.5, 120.0);
    }

    /// Reframe the scene to a sensible default 3/4 view that takes in the whole
    /// long flight box (the +x axis is 3× the cross-section).
    pub fn fit(&mut self) {
        self.target = Vec3::ZERO;
        self.yaw    = 0.85;
        self.pitch  = 0.38;
        self.dist   = 15.0;
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        let eye = self.eye();
        let forward = (self.target - eye).normalize();
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward).normalize();
        let k = self.dist * 0.0015;
        self.target += -right * dx * k + up * dy * k;
    }
}
