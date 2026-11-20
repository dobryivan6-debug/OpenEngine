use crate::scene::{list, number, vector, Object, Package, Scene};
use serde_json::Value;

const MAX_EMITTERS: usize = 32;
const MAX_PARTICLES: usize = 1024;

pub type Vertex = [f32; 8];

pub struct Emitter {
    pub texture: Option<usize>,
    pub additive: bool,
    pub soft: f32,
    origin: [f32; 2],
    scale: [f32; 2],
    rotation: f32,
    radius: [f32; 2],
    directions: [f32; 2],
    rate: f32,
    capacity: usize,
    life: [f32; 2],
    size: [f32; 2],
    alpha: [f32; 2],
    velocity: [[f32; 2]; 2],
    turbulence: f32,
    colors: [[f32; 3]; 2],
    gravity: [f32; 2],
    drag: f32,
    fade: [f32; 2],
    growth: [f32; 2],
    trail: [f32; 2],
    wiggle: [f32; 2],
    particles: Vec<Particle>,
    elapsed: f32,
    rng: u32,
}

struct Particle {
    pos: [f32; 2],
    velocity: [f32; 2],
    color: [f32; 4],
    radius: f32,
    life: f32,
    age: f32,
    phase: f32,
}

fn named<'a>(values: &'a Value, name: &str) -> &'a Value {
    list(values)
        .iter()
        .find(|item| item["name"].as_str() == Some(name))
        .unwrap_or(&Value::Null)
}

fn range(inputs: &Value, name: &str, defaults: [f32; 2], bounds: [f32; 2]) -> [f32; 2] {
    let item = named(inputs, name);
    [
        number(&item["min"], defaults[0], bounds[0], bounds[1]),
        number(&item["max"], defaults[1], bounds[0], bounds[1]),
    ]
}

fn lerp(a: f32, b: f32, f: f32) -> f32 {
    a + (b - a) * f
}

fn rotate([x, y]: [f32; 2], angle: f32) -> [f32; 2] {
    let (s, c) = angle.sin_cos();
    [x * c - y * s, x * s + y * c]
}

fn rnd(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state >> 8) as f32 / 16_777_216.0
}

impl Emitter {
    pub fn load(scene: &mut Scene, package: &Package<'_>, object: &Value, path: &str) {
        let origin = vector(
            &object["origin"],
            [scene.size[0] / 2.0, scene.size[1] / 2.0],
        );
        Self::load_children(scene, package, object, path, origin, 0);
    }

    fn load_children(
        scene: &mut Scene,
        package: &Package<'_>,
        object: &Value,
        path: &str,
        origin: [f32; 2],
        depth: usize,
    ) {
        if depth > 3
            || scene
                .objects
                .iter()
                .filter(|o| matches!(o, Object::Particles(_)))
                .count()
                >= MAX_EMITTERS
        {
            return;
        }
        let Some(def) = package.json(path) else {
            return;
        };
        let overrides = &object["instanceoverride"];
        let [_, _, rotation] = vector(&object["angles"], [0.0; 3]);
        let scale = vector(&object["scale"], [1.0; 2]);
        let speed = number(&overrides["speed"], 1.0, 0.0, 10.0);
        let size = number(&overrides["size"], 1.0, 0.0, 10.0);
        let mut alpha = number(&overrides["alpha"], 1.0, 0.0, 1.0);
        let count = number(&overrides["count"], 1.0, 0.0, 8.0);
        let capacity =
            (number(&def["maxcount"], 128.0, 0.0, MAX_PARTICLES as f32) * count) as usize;
        let init = &def["initializer"];
        let operators = &def["operator"];
        let movement = named(operators, "movement");
        let fade = named(operators, "alphafade");
        let growth = named(operators, "sizechange");
        let wiggle = named(operators, "oscillateposition");
        let velocity = named(init, "velocityrandom");
        let turbulence = named(init, "turbulentvelocityrandom");
        let color = named(init, "colorrandom");
        let override_color = vector(&overrides["colorn"], [1.0; 3]);
        let renderer = &def["renderer"][0];
        let material = def["material"]
            .as_str()
            .and_then(|name| package.material(name));
        let pass = material.as_ref().and_then(|m| list(&m["passes"]).first());
        let name = pass.and_then(|p| p["textures"][0].as_str()).unwrap_or("");
        let texture = (!name.is_empty())
            .then(|| scene.texture(package, name))
            .flatten();
        let soft = if name.contains("smoke") || name.contains("fog") {
            1.0
        } else {
            0.0
        };
        if soft > 0.0 && texture.is_none() {
            // The built-in smoke and fog atlases are often external to scene.pkg.
            // A procedural sprite lacks their sparse alpha coverage.
            alpha *= 0.16;
        }
        let trail = matches!(renderer["name"].as_str(), Some("spritetrail" | "ropetrail"));
        for e in list(&def["emitter"]).iter().take(4) {
            if !matches!(e["name"].as_str(), Some("sphererandom" | "boxrandom")) || capacity == 0 {
                continue;
            }
            if scene
                .objects
                .iter()
                .filter(|o| matches!(o, Object::Particles(_)))
                .count()
                >= MAX_EMITTERS
            {
                break;
            }
            let offset = rotate(vector(&e["origin"], [0.0; 2]), rotation);
            let cap = capacity.min(MAX_PARTICLES);
            let mut emitter = Self {
                texture,
                soft,
                rotation,
                scale,
                additive: pass.and_then(|p| p["blending"].as_str()) == Some("additive"),
                origin: [
                    origin[0] + offset[0] * scale[0],
                    origin[1] + offset[1] * scale[1],
                ],
                radius: [
                    number(&e["distancemin"], 0.0, 0.0, 16384.0),
                    number(&e["distancemax"], 0.0, 0.0, 16384.0),
                ],
                directions: vector(&e["directions"], [1.0; 2]),
                rate: (number(&e["rate"], 10.0, 0.0, 2048.0)
                    * number(&overrides["rate"], 1.0, 0.0, 16.0))
                .min(2048.0),
                capacity: cap,
                life: range(init, "lifetimerandom", [1.0, 3.0], [0.05, 60.0]),
                size: range(init, "sizerandom", [4.0, 8.0], [0.1, 4096.0])
                    .map(|v| (v * size).min(scene.size[1] * 0.9)),
                alpha: range(init, "alpharandom", [1.0, 1.0], [0.0, 1.0]).map(|v| v * alpha),
                velocity: [
                    vector(&velocity["min"], [0.0; 2]),
                    vector(&velocity["max"], [0.0; 2]),
                ]
                .map(|v| v.map(|n| n * speed)),
                turbulence: number(&turbulence["speedmax"], 0.0, 0.0, 1000.0) * speed,
                colors: ["min", "max"].map(|key| {
                    let rgb = vector(&color[key], [255.0; 3]);
                    std::array::from_fn(|i| (rgb[i] / 255.0 * override_color[i]).clamp(0.0, 1.0))
                }),
                gravity: vector(&movement["gravity"], [0.0; 2]),
                drag: number(&movement["drag"], 0.0, 0.0, 10.0),
                fade: [
                    number(&fade["fadeintime"], 0.1, 0.0, 30.0),
                    number(&fade["fadeouttime"], 0.3, 0.0, 30.0),
                ],
                growth: [
                    number(&growth["startvalue"], 1.0, 0.0, 16.0),
                    number(&growth["endvalue"], 1.0, 0.0, 16.0),
                ],
                trail: if trail {
                    [
                        number(&renderer["length"], 0.05, 0.0, 1.0),
                        number(&renderer["maxlength"], 260.0, 0.0, 1024.0),
                    ]
                } else {
                    [0.0; 2]
                },
                wiggle: [
                    number(&wiggle["scalemax"], 0.0, 0.0, 250.0),
                    number(&wiggle["frequencymax"], 1.0, 0.0, 20.0),
                ],
                particles: Vec::with_capacity(cap),
                elapsed: 0.0,
                rng: (number(&object["id"], 1.0, 0.0, 1_000_000.0) as u32
                    + scene.objects.len() as u32
                    + 1)
                .wrapping_mul(0x9E3779B9),
            };
            let warmup = number(&def["starttime"], 0.0, 0.0, 5.0);
            for _ in 0..(warmup * 20.0) as usize {
                emitter.tick(0.05, None, 0.0);
            }
            scene.objects.push(Object::Particles(Box::new(emitter)));
        }
        for child in list(&def["children"]).iter().take(8) {
            let Some(name) = child["name"].as_str() else {
                continue;
            };
            let offset = rotate(vector(&child["origin"], [0.0; 2]), rotation);
            Self::load_children(
                scene,
                package,
                object,
                name,
                [
                    origin[0] + offset[0] * scale[0],
                    origin[1] + offset[1] * scale[1],
                ],
                depth + 1,
            );
        }
    }

    fn spawn(&mut self) -> Particle {
        let r = &mut self.rng;
        let theta = rnd(r) * std::f32::consts::TAU;
        let radius = lerp(self.radius[0], self.radius[1], rnd(r).sqrt());
        let pos = rotate(
            [
                theta.cos() * radius * self.directions[0],
                theta.sin() * radius * self.directions[1],
            ],
            self.rotation,
        );
        let a = rnd(r) * std::f32::consts::TAU;
        let jitter = self.turbulence * rnd(r);
        let velocity = rotate(
            [
                lerp(self.velocity[0][0], self.velocity[1][0], rnd(r)) + a.cos() * jitter,
                lerp(self.velocity[0][1], self.velocity[1][1], rnd(r)) + a.sin() * jitter,
            ],
            self.rotation,
        );
        let f = rnd(r);
        Particle {
            pos: std::array::from_fn(|i| self.origin[i] + pos[i] * self.scale[i]),
            velocity: std::array::from_fn(|i| velocity[i] * self.scale[i]),
            color: std::array::from_fn(|i| {
                if i < 3 {
                    lerp(self.colors[0][i], self.colors[1][i], f)
                } else {
                    lerp(self.alpha[0], self.alpha[1], f)
                }
            }),
            radius: lerp(self.size[0], self.size[1], rnd(r)),
            life: lerp(self.life[0], self.life[1], rnd(r)),
            age: 0.0,
            phase: theta,
        }
    }

    pub fn tick(&mut self, dt: f32, pointer: Option<[f32; 2]>, reach: f32) {
        let drag = (-self.drag * dt).exp();
        let gravity = rotate(self.gravity, self.rotation);
        self.particles.retain_mut(|p| {
            p.age += dt;
            if p.age >= p.life {
                return false;
            }
            if let Some(mouse) = pointer {
                let dx = p.pos[0] - mouse[0];
                let dy = p.pos[1] - mouse[1];
                let distance = dx.hypot(dy);
                if distance < reach {
                    let push = (1.0 - distance / reach) * 16.0 * dt;
                    p.velocity[0] += dx * push;
                    p.velocity[1] += dy * push;
                }
            }
            for (i, g) in gravity.into_iter().enumerate() {
                p.velocity[i] = p.velocity[i] * drag + g * dt;
                p.pos[i] += p.velocity[i] * dt;
            }
            true
        });
        self.elapsed += self.rate * dt;
        let count = self.elapsed.floor() as usize;
        self.elapsed -= count as f32;
        for _ in 0..count.min(self.capacity - self.particles.len()) {
            let p = self.spawn();
            self.particles.push(p);
        }
    }

    pub fn vertices(&self, out: &mut Vec<Vertex>) {
        out.clear();
        for p in &self.particles {
            let radius = p.radius * lerp(self.growth[0], self.growth[1], p.age / p.life) * 0.5;
            let mut color = p.color;
            color[3] *= (p.age / self.fade[0].max(0.001)).min(1.0)
                * ((p.life - p.age) / self.fade[1].max(0.001)).min(1.0);
            let wiggle =
                (p.phase + p.age * self.wiggle[1] * std::f32::consts::TAU).sin() * self.wiggle[0];
            let speed = p.velocity[0].hypot(p.velocity[1]);
            let length = (speed * self.trail[0]).min(self.trail[1]);
            let angle = if length > 0.0 {
                p.velocity[1].atan2(p.velocity[0])
            } else {
                0.0
            };
            quad(
                out,
                [p.pos[0] + wiggle, p.pos[1]],
                [radius + length * 0.5, radius],
                angle,
                color,
            );
        }
    }
}

pub fn quad(out: &mut Vec<Vertex>, pos: [f32; 2], half: [f32; 2], angle: f32, color: [f32; 4]) {
    for [u, v] in [
        [0.0, 1.0],
        [1.0, 1.0],
        [0.0, 0.0],
        [0.0, 0.0],
        [1.0, 1.0],
        [1.0, 0.0],
    ] {
        let [x, y] = rotate(
            [(2.0 * u - 1.0) * half[0], (1.0 - 2.0 * v) * half[1]],
            angle,
        );
        out.push([
            pos[0] + x,
            pos[1] + y,
            u,
            v,
            color[0],
            color[1],
            color[2],
            color[3],
        ]);
    }
}
