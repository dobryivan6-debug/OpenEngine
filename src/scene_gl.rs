use crate::{
    particles::{quad, Vertex},
    scene::{Object, Package, Scene},
    win::{client, wide},
};
use glow::HasContext;
use std::{
    ffi::CString,
    io,
    mem::size_of,
    path::Path,
    ptr::{null, null_mut},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{Gdi::*, OpenGL::*},
    System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
        WindowsAndMessaging::*,
    },
};

const CLASS: &str = "OpenEngine.Scene.OpenGL";

fn error(message: impl ToString) -> io::Error {
    io::Error::other(message.to_string())
}

struct Surface {
    window: HWND,
    dc: HDC,
    context: HGLRC,
}

impl Surface {
    unsafe fn create(parent: HWND) -> io::Result<Self> {
        let instance = GetModuleHandleW(null());
        let name = wide(CLASS);
        let class = WNDCLASSW {
            style: CS_OWNDC,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            ..Default::default()
        };
        RegisterClassW(&class);
        let area = client(parent);
        let window = CreateWindowExW(
            WS_EX_NOACTIVATE,
            name.as_ptr(),
            null(),
            WS_CHILD | WS_CLIPSIBLINGS,
            0,
            0,
            area.right,
            area.bottom,
            parent,
            null_mut(),
            instance,
            null(),
        );
        if window.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut surface = Self {
            window,
            dc: GetDC(window),
            context: null_mut(),
        };
        let pfd = PIXELFORMATDESCRIPTOR {
            nSize: size_of::<PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
            iPixelType: PFD_TYPE_RGBA,
            cColorBits: 24,
            ..Default::default()
        };
        let format = ChoosePixelFormat(surface.dc, &pfd);
        if format == 0 || SetPixelFormat(surface.dc, format, &pfd) == 0 {
            return Err(io::Error::last_os_error());
        }
        surface.context = wglCreateContext(surface.dc);
        if surface.context.is_null() || !surface.current() {
            return Err(io::Error::last_os_error());
        }
        Ok(surface)
    }

    unsafe fn current(&self) -> bool {
        wglGetCurrentContext() == self.context || wglMakeCurrent(self.dc, self.context) != 0
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            if !self.context.is_null() {
                if wglGetCurrentContext() == self.context {
                    wglMakeCurrent(null_mut(), null_mut());
                }
                wglDeleteContext(self.context);
            }
            if !self.dc.is_null() {
                ReleaseDC(self.window, self.dc);
            }
            if IsWindow(self.window) != 0 {
                DestroyWindow(self.window);
            }
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, event: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match event {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut paint);
            EndPaint(hwnd, &paint);
            0
        }
        _ => DefWindowProcW(hwnd, event, wp, lp),
    }
}

pub struct Renderer {
    surface: Surface,
    gl: glow::Context,
    program: glow::Program,
    vertices: glow::Buffer,
    uniforms: Uniforms,
    scene: Scene,
    textures: Vec<glow::Texture>,
    white: glow::Texture,
    scratch: Vec<Vertex>,
    last: Instant,
    time: f32,
    mouse: [f32; 2],
    mouse_down: bool,
    ripple: Option<([f32; 2], f32)>,
    parent: HWND,
    low: bool,
    framebuffer: glow::Framebuffer,
    color: glow::Texture,
    dimensions: (i32, i32),
    pixels: Vec<u8>,
}

struct Uniforms {
    canvas: Option<glow::UniformLocation>,
    fit: Option<glow::UniformLocation>,
    mode: Option<glow::UniformLocation>,
    water: Option<glow::UniformLocation>,
    scroll: Option<glow::UniformLocation>,
    shake: Option<glow::UniformLocation>,
    iris: Option<glow::UniformLocation>,
    time: Option<glow::UniformLocation>,
    pointer: Option<glow::UniformLocation>,
    screen: Option<glow::UniformLocation>,
    soft: Option<glow::UniformLocation>,
}

impl Renderer {
    pub unsafe fn start(parent: HWND, file: &Path, low: bool) -> io::Result<Self> {
        let raw = crate::scene::read_package(file)?;
        let package = Package::parse(&raw)?;
        let main = crate::archive::scene_entry(file);
        let (mut scene, _) = Scene::load(&package, &main)?;
        drop(raw);
        let surface = Surface::create(parent)?;
        let module = GetModuleHandleW(wide("opengl32.dll").as_ptr());
        let gl = glow::Context::from_loader_function(|name| {
            let name = CString::new(name).unwrap();
            let address = wglGetProcAddress(name.as_ptr().cast()).map_or(0, |p| p as usize);
            if address > 3 && address != usize::MAX {
                address as *const _
            } else {
                GetProcAddress(module, name.as_ptr().cast()).map_or(null(), |p| p as *const _)
            }
        });
        if gl.version().major < 3 {
            return Err(error("OpenGL 3.0 required"));
        }
        let program = shader(&gl)?;
        gl.use_program(Some(program));
        let vertices = gl.create_buffer().map_err(error)?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertices));
        for (index, count, offset) in [(0, 2, 0), (1, 2, 8), (2, 4, 16)] {
            gl.enable_vertex_attrib_array(index);
            gl.vertex_attrib_pointer_f32(
                index,
                count,
                glow::FLOAT,
                false,
                size_of::<Vertex>() as i32,
                offset,
            );
        }
        for (i, name) in ["image", "waterMask", "normals", "flowMap", "irisMask"]
            .into_iter()
            .enumerate()
        {
            gl.uniform_1_i32(gl.get_uniform_location(program, name).as_ref(), i as i32);
        }
        let uniform = |name| gl.get_uniform_location(program, name);
        let uniforms = Uniforms {
            canvas: uniform("canvas"),
            fit: uniform("fit"),
            mode: uniform("mode"),
            water: uniform("water"),
            scroll: uniform("scroll"),
            shake: uniform("shake"),
            iris: uniform("iris"),
            time: uniform("clockTime"),
            pointer: uniform("pointer"),
            screen: uniform("screen"),
            soft: uniform("softness"),
        };
        gl.enable(glow::BLEND);
        let mut textures = Vec::new();
        let max = (gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE).max(1) as u32).min(if low {
            1024
        } else {
            8192
        });
        for image in scene.images.drain(..) {
            let image = if image.width() > max || image.height() > max {
                let factor = max as f32 / image.width().max(image.height()) as f32;
                image::imageops::resize(
                    &image,
                    (image.width() as f32 * factor).round().max(1.0) as u32,
                    (image.height() as f32 * factor).round().max(1.0) as u32,
                    image::imageops::FilterType::Triangle,
                )
            } else {
                image
            };
            textures.push(texture(
                &gl,
                image.width() as i32,
                image.height() as i32,
                &image,
            )?);
        }
        let white = texture(&gl, 1, 1, &[255; 4])?;
        let framebuffer = gl.create_framebuffer().map_err(error)?;
        let color = gl.create_texture().map_err(error)?;
        Ok(Self {
            surface,
            gl,
            program,
            vertices,
            uniforms,
            scene,
            textures,
            white,
            scratch: Vec::new(),
            last: Instant::now(),
            time: 0.0,
            mouse: [0.0; 2],
            mouse_down: true,
            ripple: None,
            parent,
            low,
            framebuffer,
            color,
            dimensions: (0, 0),
            pixels: Vec::new(),
        })
    }

    pub unsafe fn paint(&self, dc: HDC, area: RECT) {
        let (w, h) = self.dimensions;
        if self.pixels.is_empty() {
            return;
        }
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        SetStretchBltMode(dc, HALFTONE);
        StretchDIBits(
            dc,
            0,
            0,
            area.right,
            area.bottom,
            0,
            0,
            w,
            h,
            self.pixels.as_ptr().cast(),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }

    pub fn reset_clock(&mut self) {
        self.last = Instant::now();
    }

    pub unsafe fn draw(&mut self, interactive: bool) -> bool {
        if !self.surface.current() {
            return false;
        }
        let area = client(self.parent);
        let (aw, ah) = (area.right as f32, area.bottom as f32);
        if aw <= 0.0 || ah <= 0.0 {
            return false;
        }
        let (max_w, max_h) = if self.low {
            (960.0, 540.0)
        } else {
            (3840.0, 2160.0)
        };
        let scale = (max_w / aw).min(max_h / ah).min(1.0);
        let (width, height) = (
            (aw * scale).round().max(1.0) as i32,
            (ah * scale).round().max(1.0) as i32,
        );
        let (w, h) = (width as f32, height as f32);
        let gl = &self.gl;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
        if (width, height) != self.dimensions {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.color));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width,
                height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(self.color),
                0,
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return false;
            }
            self.dimensions = (width, height);
            self.pixels.resize(width as usize * height as usize * 4, 0);
        }
        let dt = self.last.elapsed().as_secs_f32().min(0.10);
        self.last = Instant::now();
        self.time += dt;
        let [cw, ch] = self.scene.size;
        let zoom = (w / cw).max(h / ch);
        let fit = [cw * zoom / w, ch * zoom / h];
        let mut point = POINT::default();
        GetCursorPos(&mut point);
        ScreenToClient(self.parent, &mut point);
        let mouse = [point.x as f32 * w / aw, h - point.y as f32 * h / ah];
        let inside = interactive && (0.0..w).contains(&mouse[0]) && (0.0..h).contains(&mouse[1]);
        let target = if inside {
            [2.0 * mouse[0] / w - 1.0, 2.0 * mouse[1] / h - 1.0]
        } else {
            [0.0; 2]
        };
        for (component, target) in self.mouse.iter_mut().zip(target) {
            *component += (target - *component) * (1.0 - (-8.0 * dt).exp());
        }
        let down = GetAsyncKeyState(VK_LBUTTON as i32) < 0;
        if inside && down && !self.mouse_down {
            self.ripple = Some((mouse, self.time));
        }
        self.mouse_down = down;
        let [x, y, age, amp] = self.ripple.map_or([0.0; 4], |(m, start)| {
            let age = self.time - start;
            [m[0], m[1], age, (1.0 - age / 0.85).max(0.0)]
        });
        let emitter_pointer = (inside && down).then_some([
            cw * 0.5 + (mouse[0] - w * 0.5) / zoom,
            ch * 0.5 + (mouse[1] - h * 0.5) / zoom,
        ]);
        let u = &self.uniforms;
        gl.viewport(0, 0, w as i32, h as i32);
        let [r, g, b] = self.scene.clear;
        gl.clear_color(r, g, b, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.use_program(Some(self.program));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vertices));
        gl.uniform_2_f32(u.canvas.as_ref(), cw, ch);
        gl.uniform_2_f32(u.fit.as_ref(), fit[0], fit[1]);
        gl.uniform_2_f32(u.screen.as_ref(), w, h);
        gl.uniform_4_f32(u.pointer.as_ref(), x, y, age, amp);
        gl.uniform_1_f32(u.time.as_ref(), self.time);
        let [cam_amp, cam_speed, rough] = self.scene.camera;
        let phase = self.time * cam_speed;
        let shake = [
            (phase * 2.3).sin() + 0.3 * (phase * rough * 5.7).sin(),
            (phase * 1.7).cos(),
        ];
        for object in &mut self.scene.objects {
            gl.active_texture(glow::TEXTURE0);
            match object {
                Object::Image(layer) => {
                    gl.blend_func(
                        glow::SRC_ALPHA,
                        if layer.additive {
                            glow::ONE
                        } else {
                            glow::ONE_MINUS_SRC_ALPHA
                        },
                    );
                    gl.uniform_1_i32(u.mode.as_ref(), 0);
                    let effect = &layer.effect;
                    for (i, index) in [
                        Some(layer.texture),
                        effect.water_mask,
                        effect.normals,
                        effect.flow,
                        effect.iris_mask,
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        gl.active_texture(glow::TEXTURE0 + i as u32);
                        gl.bind_texture(
                            glow::TEXTURE_2D,
                            Some(index.map_or(self.white, |i| self.textures[i])),
                        );
                        if i == 2 && index.is_some() {
                            gl.tex_parameter_i32(
                                glow::TEXTURE_2D,
                                glow::TEXTURE_WRAP_S,
                                glow::REPEAT as i32,
                            );
                            gl.tex_parameter_i32(
                                glow::TEXTURE_2D,
                                glow::TEXTURE_WRAP_T,
                                glow::REPEAT as i32,
                            );
                        }
                    }
                    let mut water = effect.water;
                    if effect.normals.is_none() {
                        water[0] = 0.0;
                    }
                    gl.uniform_4_f32_slice(u.water.as_ref(), &water);
                    gl.uniform_2_f32_slice(u.scroll.as_ref(), &effect.scroll);
                    gl.uniform_4_f32_slice(u.shake.as_ref(), &effect.shake);
                    gl.uniform_2_f32_slice(u.iris.as_ref(), &effect.iris);
                    let center = std::array::from_fn(|i| {
                        layer.origin[i]
                            - self.mouse[i] * layer.depth[i] * self.scene.parallax * ch * 0.025
                            + shake[i] * cam_amp * ch * 0.003
                    });
                    self.scratch.clear();
                    quad(
                        &mut self.scratch,
                        center,
                        layer.size.map(|n| n * 0.5),
                        layer.angle,
                        layer.tint,
                    );
                }
                Object::Particles(emitter) => {
                    gl.blend_func(
                        glow::SRC_ALPHA,
                        if emitter.additive {
                            glow::ONE
                        } else {
                            glow::ONE_MINUS_SRC_ALPHA
                        },
                    );
                    if let Some(index) = emitter.texture {
                        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[index]));
                    }
                    gl.uniform_1_i32(
                        u.mode.as_ref(),
                        if emitter.texture.is_some() { 1 } else { 2 },
                    );
                    gl.uniform_1_f32(u.soft.as_ref(), emitter.soft);
                    emitter.tick(dt, emitter_pointer, ch * 0.14);
                    emitter.vertices(&mut self.scratch);
                }
            }
            if !self.scratch.is_empty() {
                let bytes = std::slice::from_raw_parts(
                    self.scratch.as_ptr().cast(),
                    self.scratch.len() * size_of::<Vertex>(),
                );
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
                gl.draw_arrays(glow::TRIANGLES, 0, self.scratch.len() as i32);
            }
        }
        gl.read_pixels(
            0,
            0,
            width,
            height,
            glow::BGRA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut self.pixels)),
        );
        true
    }
}

unsafe fn texture(gl: &glow::Context, w: i32, h: i32, pixels: &[u8]) -> io::Result<glow::Texture> {
    let id = gl.create_texture().map_err(error)?;
    gl.bind_texture(glow::TEXTURE_2D, Some(id));
    for name in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
        gl.tex_parameter_i32(glow::TEXTURE_2D, name, glow::LINEAR as i32);
    }
    for name in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
        gl.tex_parameter_i32(glow::TEXTURE_2D, name, glow::CLAMP_TO_EDGE as i32);
    }
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA8 as i32,
        w,
        h,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(Some(pixels)),
    );
    if gl.get_error() != glow::NO_ERROR {
        return Err(error("OpenGL texture upload failed"));
    }
    Ok(id)
}

unsafe fn shader(gl: &glow::Context) -> io::Result<glow::Program> {
    let program = gl.create_program().map_err(error)?;
    for (kind, source) in [
        (glow::VERTEX_SHADER, include_str!("scene.vert")),
        (glow::FRAGMENT_SHADER, include_str!("scene.frag")),
    ] {
        let id = gl.create_shader(kind).map_err(error)?;
        gl.shader_source(id, source);
        gl.compile_shader(id);
        if !gl.get_shader_compile_status(id) {
            return Err(error(gl.get_shader_info_log(id)));
        }
        gl.attach_shader(program, id);
        gl.delete_shader(id);
    }
    for (i, name) in ["position", "texcoord", "color"].into_iter().enumerate() {
        gl.bind_attrib_location(program, i as u32, name);
    }
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        return Err(error(gl.get_program_info_log(program)));
    }
    Ok(program)
}
