pub type Vector = [f32; 3];
pub type Matrix = [f32; 16];

pub fn subtract(a: Vector, b: Vector) -> Vector {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add(a: Vector, b: Vector) -> Vector {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn scale(a: Vector, factor: f32) -> Vector {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

pub fn dot(a: Vector, b: Vector) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vector, b: Vector) -> Vector {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn normalize(a: Vector) -> Vector {
    let length = dot(a, a).sqrt();
    if length > 1e-6 {
        scale(a, 1.0 / length)
    } else {
        a
    }
}

pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0f32; 16];
    for column in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for step in 0..4 {
                sum += a[step * 4 + row] * b[column * 4 + step];
            }
            out[column * 4 + row] = sum;
        }
    }
    out
}

pub fn perspective(field_of_view: f32, aspect: f32, near: f32, far: f32) -> Matrix {
    let focal = 1.0 / (field_of_view * 0.5).tan();
    let mut out = [0.0f32; 16];
    out[0] = focal / aspect;
    out[5] = focal;
    out[10] = (far + near) / (near - far);
    out[11] = -1.0;
    out[14] = (2.0 * far * near) / (near - far);
    out
}

pub fn look_at(eye: Vector, center: Vector, up: Vector) -> Matrix {
    let forward = normalize(subtract(center, eye));
    let side = normalize(cross(forward, up));
    let upward = cross(side, forward);
    [
        side[0], upward[0], -forward[0], 0.0, side[1], upward[1], -forward[1], 0.0, side[2],
        upward[2], -forward[2], 0.0, -dot(side, eye), -dot(upward, eye), dot(forward, eye), 1.0,
    ]
}

fn transform(matrix: &Matrix, point: Vector) -> [f32; 4] {
    [
        matrix[0] * point[0] + matrix[4] * point[1] + matrix[8] * point[2] + matrix[12],
        matrix[1] * point[0] + matrix[5] * point[1] + matrix[9] * point[2] + matrix[13],
        matrix[2] * point[0] + matrix[6] * point[1] + matrix[10] * point[2] + matrix[14],
        matrix[3] * point[0] + matrix[7] * point[1] + matrix[11] * point[2] + matrix[15],
    ]
}

pub struct Camera {
    pub target: Vector,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: [0.0; 3],
            yaw: 0.9,
            pitch: 0.5,
            distance: 400.0,
        }
    }
}

impl Camera {
    pub fn eye(&self) -> Vector {
        let cosine = self.pitch.cos();
        add(
            self.target,
            scale(
                [
                    self.yaw.cos() * cosine,
                    self.pitch.sin(),
                    self.yaw.sin() * cosine,
                ],
                self.distance,
            ),
        )
    }

    pub fn view_projection(&self, aspect: f32) -> Matrix {
        let view = look_at(self.eye(), self.target, [0.0, 1.0, 0.0]);
        let projection = perspective(
            60f32.to_radians(),
            aspect.max(0.01),
            (self.distance * 0.005).max(0.05),
            self.distance * 20.0 + 1000.0,
        );
        multiply(&projection, &view)
    }
}

pub struct Triangle {
    pub points: [Vector; 3],
    pub uv: [[f32; 2]; 3],
    pub texture: i32,
    pub color: [u8; 3],
    pub light: [f32; 3],
}

pub struct Texture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub struct Scene {
    pub triangles: Vec<Triangle>,
    pub lines: Vec<(Vector, Vector, [u8; 3])>,
    pub textures: Vec<Texture>,
}

impl Scene {
    pub fn shade(&mut self) {
        let key_direction = normalize([0.35, 0.9, 0.25]);
        for triangle in self.triangles.iter_mut() {
            let normal = normalize(cross(
                subtract(triangle.points[1], triangle.points[0]),
                subtract(triangle.points[2], triangle.points[0]),
            ));
            let hemisphere = 0.5 + 0.5 * normal[1].clamp(-1.0, 1.0);
            let key = dot(normal, key_direction).max(0.0);
            let intensity = 0.40 + 0.32 * hemisphere + 0.30 * key;
            triangle.light = [intensity * 0.93, intensity * 0.97, intensity * 1.07];
        }
    }

    pub fn add_grid(&mut self, extent: f32, step: f32, color: [u8; 3]) {
        let mut position = -extent;
        while position <= extent {
            self.lines
                .push(([position, 0.0, -extent], [position, 0.0, extent], color));
            self.lines
                .push(([-extent, 0.0, position], [extent, 0.0, position], color));
            position += step;
        }
    }
}

fn linear_to_srgb(value: f32) -> u8 {
    let tonemapped = (value / (1.0 + value)).clamp(0.0, 1.0);
    let encoded = if tonemapped <= 0.0031308 {
        tonemapped * 12.92
    } else {
        1.055 * tonemapped.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

pub struct Raster {
    pub width: usize,
    pub height: usize,
    pub color: Vec<u8>,
    pub depth: Vec<f32>,
    lookup: [f32; 256],
    pub sky_top: [u8; 3],
    pub sky_bottom: [u8; 3],
}

impl Raster {
    pub fn new(width: usize, height: usize) -> Self {
        let mut lookup = [0.0f32; 256];
        for (index, entry) in lookup.iter_mut().enumerate() {
            let value = index as f32 / 255.0;
            *entry = if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            };
        }
        Self {
            width,
            height,
            color: vec![0; width * height * 4],
            depth: vec![f32::INFINITY; width * height],
            lookup,
            sky_top: [54, 58, 74],
            sky_bottom: [22, 22, 30],
        }
    }

    fn clear(&mut self) {
        let top = self.sky_top;
        let bottom = self.sky_bottom;
        for y in 0..self.height {
            let ratio = y as f32 / (self.height.max(2) - 1) as f32;
            let blended = [
                (top[0] as f32 * (1.0 - ratio) + bottom[0] as f32 * ratio) as u8,
                (top[1] as f32 * (1.0 - ratio) + bottom[1] as f32 * ratio) as u8,
                (top[2] as f32 * (1.0 - ratio) + bottom[2] as f32 * ratio) as u8,
            ];
            for x in 0..self.width {
                let offset = (y * self.width + x) * 4;
                self.color[offset] = blended[0];
                self.color[offset + 1] = blended[1];
                self.color[offset + 2] = blended[2];
                self.color[offset + 3] = 255;
            }
        }
        for value in self.depth.iter_mut() {
            *value = f32::INFINITY;
        }
    }

    pub fn render(&mut self, scene: &Scene, view_projection: &Matrix) {
        self.clear();
        let width = self.width as f32;
        let height = self.height as f32;
        let project = |point: Vector| -> Option<(f32, f32, f32, f32)> {
            let clip = transform(view_projection, point);
            if clip[3] <= 0.01 {
                return None;
            }
            let inverse = 1.0 / clip[3];
            Some((
                (clip[0] * inverse * 0.5 + 0.5) * width,
                (1.0 - (clip[1] * inverse * 0.5 + 0.5)) * height,
                clip[2] * inverse,
                inverse,
            ))
        };
        for triangle in &scene.triangles {
            let projected = (
                project(triangle.points[0]),
                project(triangle.points[1]),
                project(triangle.points[2]),
            );
            let (a, b, c) = match projected {
                (Some(a), Some(b), Some(c)) => (a, b, c),
                _ => continue,
            };
            if triangle.texture >= 0 && (triangle.texture as usize) < scene.textures.len() {
                self.textured(
                    a,
                    b,
                    c,
                    &triangle.uv,
                    &scene.textures[triangle.texture as usize],
                    triangle.light,
                );
            } else {
                let lookup = self.lookup;
                let color = [
                    linear_to_srgb(lookup[triangle.color[0] as usize] * triangle.light[0]),
                    linear_to_srgb(lookup[triangle.color[1] as usize] * triangle.light[1]),
                    linear_to_srgb(lookup[triangle.color[2] as usize] * triangle.light[2]),
                ];
                self.solid(a, b, c, color);
            }
        }
        for (from, to, color) in &scene.lines {
            if let (Some(a), Some(b)) = (project(*from), project(*to)) {
                self.line(a, b, *color);
            }
        }
    }

    fn bounds(
        &self,
        a: (f32, f32, f32, f32),
        b: (f32, f32, f32, f32),
        c: (f32, f32, f32, f32),
    ) -> Option<(usize, usize, usize, usize)> {
        let min_x = a.0.min(b.0).min(c.0).floor().max(0.0) as usize;
        let max_x = a.0.max(b.0).max(c.0).ceil().min(self.width as f32 - 1.0) as usize;
        let min_y = a.1.min(b.1).min(c.1).floor().max(0.0) as usize;
        let max_y = a.1.max(b.1).max(c.1).ceil().min(self.height as f32 - 1.0) as usize;
        (min_x <= max_x && min_y <= max_y).then_some((min_x, max_x, min_y, max_y))
    }

    fn solid(
        &mut self,
        a: (f32, f32, f32, f32),
        b: (f32, f32, f32, f32),
        c: (f32, f32, f32, f32),
        color: [u8; 3],
    ) {
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-4 {
            return;
        }
        let inverse_area = 1.0 / area;
        let Some((min_x, max_x, min_y, max_y)) = self.bounds(a, b, c) else {
            return;
        };
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) * inverse_area;
                let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) * inverse_area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let depth = w0 * a.2 + w1 * b.2 + w2 * c.2;
                let index = y * self.width + x;
                if depth < self.depth[index] {
                    self.depth[index] = depth;
                    let offset = index * 4;
                    self.color[offset] = color[0];
                    self.color[offset + 1] = color[1];
                    self.color[offset + 2] = color[2];
                    self.color[offset + 3] = 255;
                }
            }
        }
    }

    fn textured(
        &mut self,
        a: (f32, f32, f32, f32),
        b: (f32, f32, f32, f32),
        c: (f32, f32, f32, f32),
        uv: &[[f32; 2]; 3],
        texture: &Texture,
        light: [f32; 3],
    ) {
        if texture.width == 0 || texture.height == 0 {
            return;
        }
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-4 {
            return;
        }
        let inverse_area = 1.0 / area;
        let Some((min_x, max_x, min_y, max_y)) = self.bounds(a, b, c) else {
            return;
        };
        let projected_uv = [
            (uv[0][0] * a.3, uv[0][1] * a.3),
            (uv[1][0] * b.3, uv[1][1] * b.3),
            (uv[2][0] * c.3, uv[2][1] * c.3),
        ];
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) * inverse_area;
                let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) * inverse_area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let depth = w0 * a.2 + w1 * b.2 + w2 * c.2;
                let index = y * self.width + x;
                if depth >= self.depth[index] {
                    continue;
                }
                let inverse_w = w0 * a.3 + w1 * b.3 + w2 * c.3;
                if inverse_w.abs() < 1e-12 {
                    continue;
                }
                let u = (w0 * projected_uv[0].0 + w1 * projected_uv[1].0 + w2 * projected_uv[2].0)
                    / inverse_w;
                let v = (w0 * projected_uv[0].1 + w1 * projected_uv[1].1 + w2 * projected_uv[2].1)
                    / inverse_w;
                let sample = self.sample(texture, u, v);
                if sample[3] < 110.0 {
                    continue;
                }
                self.depth[index] = depth;
                let offset = index * 4;
                self.color[offset] = linear_to_srgb(sample[0] * light[0]);
                self.color[offset + 1] = linear_to_srgb(sample[1] * light[1]);
                self.color[offset + 2] = linear_to_srgb(sample[2] * light[2]);
                self.color[offset + 3] = 255;
            }
        }
    }

    fn sample(&self, texture: &Texture, u: f32, v: f32) -> [f32; 4] {
        let fu = (u - u.floor()) * texture.width as f32 - 0.5;
        let fv = (v - v.floor()) * texture.height as f32 - 0.5;
        let x0 = fu.floor();
        let y0 = fv.floor();
        let du = fu - x0;
        let dv = fv - y0;
        let wrap = |value: i32, size: usize| ((value % size as i32 + size as i32) % size as i32) as usize;
        let ix0 = wrap(x0 as i32, texture.width);
        let ix1 = wrap(x0 as i32 + 1, texture.width);
        let iy0 = wrap(y0 as i32, texture.height);
        let iy1 = wrap(y0 as i32 + 1, texture.height);
        let texel = |tx: usize, ty: usize| -> [f32; 4] {
            let offset = (ty * texture.width + tx) * 4;
            [
                self.lookup[texture.rgba[offset] as usize],
                self.lookup[texture.rgba[offset + 1] as usize],
                self.lookup[texture.rgba[offset + 2] as usize],
                texture.rgba[offset + 3] as f32,
            ]
        };
        let t00 = texel(ix0, iy0);
        let t10 = texel(ix1, iy0);
        let t01 = texel(ix0, iy1);
        let t11 = texel(ix1, iy1);
        let mut out = [0.0f32; 4];
        for channel in 0..4 {
            out[channel] = (t00[channel] * (1.0 - du) + t10[channel] * du) * (1.0 - dv)
                + (t01[channel] * (1.0 - du) + t11[channel] * du) * dv;
        }
        out
    }

    fn line(&mut self, from: (f32, f32, f32, f32), to: (f32, f32, f32, f32), color: [u8; 3]) {
        let steps = (to.0 - from.0)
            .abs()
            .max((to.1 - from.1).abs())
            .ceil()
            .max(1.0);
        let count = (steps as usize).min(4096);
        for step in 0..=count {
            let ratio = step as f32 / steps;
            let x = (from.0 + (to.0 - from.0) * ratio).round();
            let y = (from.1 + (to.1 - from.1) * ratio).round();
            if x < 0.0 || y < 0.0 || x >= self.width as f32 || y >= self.height as f32 {
                continue;
            }
            let index = y as usize * self.width + x as usize;
            let depth = from.2 + (to.2 - from.2) * ratio - 0.0008;
            if depth <= self.depth[index] {
                let offset = index * 4;
                self.color[offset] = color[0];
                self.color[offset + 1] = color[1];
                self.color[offset + 2] = color[2];
                self.color[offset + 3] = 255;
            }
        }
    }
}
