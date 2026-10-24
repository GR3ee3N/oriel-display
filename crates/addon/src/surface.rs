//! Draw the selected producer buffer or test pattern as a premultiplied
//! full-screen triangle in Kodi's GUI frame. Both windows share the surface,
//! retaining imports across window transitions. Access GL resources only on
//! Kodi's GUI thread with Kodi's GL context current.

use crate::gl::*;
use crate::kodi::{Renderer, debug, error, info};
use crate::pacing;
use crate::producer::{self, Incoming};
use crate::stats::Stats;
use crate::windows::{self, Which};
use oriel_display_protocol::kind;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Force per-frame redraws for diagnostics; otherwise redraw on page updates.
pub static ALWAYS_DIRTY: AtomicBool = AtomicBool::new(false);
/// Select the diagnostic test pattern as the drawing source.
pub static PATTERN_MODE: AtomicBool = AtomicBool::new(false);
/// Enable GPU timing through EXT_disjoint_timer_query.
pub static TIMING: AtomicBool = AtomicBool::new(false);

const VERTEX: &std::ffi::CStr = c"#version 300 es
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  // nearest: Kodi's back-to-front pass tests GEQUAL against what its
  // front-to-back pass wrote
  gl_Position = vec4(p * 2.0 - 1.0, 1.0, 1.0);
}
";

const PATTERN: &std::ffi::CStr = c"#version 300 es
precision highp float;
uniform float t;
uniform vec4 viewport;
out vec4 color;
void main() {
  vec2 p = (gl_FragCoord.xy - viewport.xy) / viewport.zw;
  float band = 0.5 + 0.5 * sin((p.x + p.y) * 12.0 - t * 3.0);
  vec3 c = mix(vec3(0.05, 0.15, 0.45), vec3(0.95, 0.45, 0.1), band);
  float a = 0.45;
  float x = fract(t * 0.25);
  if (abs(p.x - x) < 0.06 && abs(p.y - 0.5) < 0.12) { c = vec3(1.0); a = 1.0; }
  if (abs(p.y - fract(t * 0.5)) < 0.01) { c = vec3(0.0, 1.0, 0.3); a = 1.0; }
  color = vec4(c * a, a);
}
";

const PAGE: &std::ffi::CStr = c"#version 300 es
precision highp float;
uniform sampler2D page;
uniform vec4 viewport;
out vec4 color;
void main() {
  vec2 p = (gl_FragCoord.xy - viewport.xy) / viewport.zw;
  color = texture(page, vec2(p.x, 1.0 - p.y));
}
";

const QUERIES: usize = 8;

struct Texture {
    image: EGLImage,
    texture: GLuint,
    fence: EGLSync,
}

struct Gl {
    ext: Ext,
    display: EGLDisplay,
    pattern: GLuint,
    pattern_t: GLint,
    pattern_viewport: GLint,
    page: GLuint,
    page_viewport: GLint,
    vao: GLuint,
    queries: [GLuint; QUERIES],
}

pub struct Surface {
    clock: Instant,
    start: f64,
    gl: Option<Gl>,
    /// Own imported GL resources; producer bookkeeping tracks their outstanding replies.
    textures: HashMap<u32, Texture>,
    query_next: usize,
    query_read: usize,
    stats: Stats,
    logged_dirty: bool,
    logged_fence: bool,
    /// Force a draw after events such as window opening, even without a page update.
    redraw: bool,
}

static SURFACE: Mutex<Option<Surface>> = Mutex::new(None);

// Send is safe because all EGL/GL handle access remains on Kodi's GUI thread.
unsafe impl Send for Surface {}

fn with<R>(f: impl FnOnce(&mut Surface) -> R) -> R {
    let mut surface = SURFACE.lock().unwrap_or_else(|e| e.into_inner());
    f(surface.get_or_insert_with(Surface::new))
}

/// Request a page draw in the next GUI frame.
pub fn redraw() {
    with(|s| s.redraw = true);
}

/// Release imports on the GUI thread after all windows close. The producer
/// must resend buffers before showing them again.
pub fn drop_buffers() {
    with(Surface::drop_buffers);
}

/// Render control for one browse or player window.
pub struct Page {
    which: Which,
}

impl Page {
    pub fn new(which: Which) -> Page {
        Page { which }
    }
}

impl Renderer for Page {
    fn create(&mut self, x: i32, y: i32, width: i32, height: i32) -> bool {
        debug!("oriel-display: {} control {width}x{height} at {x},{y}", self.which.name());
        true
    }

    fn dirty(&mut self) -> bool {
        windows::on_gui_thread();
        // Pace only browsing; during video playback this thread also submits
        // decoded frames and must not wait for GUI pacing.
        if windows::drawing() && self.which == Which::Browse {
            pacing::wait();
        }
        with(|s| {
            s.retire();
            // Redraw every player frame because HDR GUI composition retains only
            // content drawn in that frame.
            windows::drawing() && (s.dirty() || self.which == Which::Player)
        })
    }

    fn render(&mut self) {
        if windows::drawing() && with(Surface::render) && self.which == Which::Browse {
            pacing::drew();
        }
    }

    fn stop(&mut self) {
        windows::closed(self.which);
        if windows::unloading() {
            with(Surface::free);
        }
    }
}

impl Surface {
    fn new() -> Surface {
        Surface {
            clock: Instant::now(),
            start: 0.0,
            gl: None,
            textures: HashMap::new(),
            query_next: 0,
            query_read: 0,
            stats: Stats::default(),
            logged_dirty: false,
            logged_fence: false,
            redraw: true,
        }
    }

    fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }
}

impl Surface {
    fn dirty(&mut self) -> bool {
        if !self.logged_dirty {
            self.logged_dirty = true;
            debug!("oriel-display: first dirty check");
        }
        if ALWAYS_DIRTY.load(Ordering::Relaxed) || self.redraw {
            self.redraw = false;
            return true;
        }
        let queue = producer::queue();
        queue.book.pending() || !queue.buffers.is_empty() || !queue.forget.is_empty() || queue.reset
    }

    /// Return true only when a page or test-pattern draw was submitted.
    fn render(&mut self) -> bool {
        let t0 = self.now();
        // Depth writes identify Kodi's opaque front-to-back pass. Draw the
        // translucent page only in the back-to-front pass.
        let mut depth_writes = 0u8;
        unsafe { glGetBooleanv(GL_DEPTH_WRITEMASK, &mut depth_writes) };
        if depth_writes != 0 {
            self.stats.opaque_pass();
            return false;
        }
        if self.gl.is_none() {
            self.init_gl();
        }
        let Some(gl) = self.gl.take() else { return false };
        let current = self.take_from_link(&gl);
        self.read_queries(&gl);
        let drew = unsafe { self.draw(&gl, current, t0) };
        self.gl = Some(gl);
        let t1 = self.now();
        self.stats.render(t0, t1);
        drew
    }

    fn drop_buffers(&mut self) {
        let Some(gl) = self.gl.as_ref() else { return };
        // Finish pending GPU reads before freeing imported buffers.
        unsafe { glFinish() };
        for (_, texture) in self.textures.drain() {
            free(gl, texture);
        }
        let replies = {
            let mut queue = producer::queue();
            queue.buffers.clear();
            queue.forget.clear();
            queue.book.drop_buffers()
        };
        producer::reply(replies);
        producer::send(kind::RESET, 0);
        info!("oriel-display: buffers dropped");
    }

    /// Free surface GL resources before add-on unload.
    fn free(&mut self) {
        info!("oriel-display: freeing the surface");
        if let Some(gl) = self.gl.take() {
            for (_, texture) in self.textures.drain() {
                free(&gl, texture);
            }
            unsafe {
                glDeleteProgram(gl.pattern);
                glDeleteProgram(gl.page);
                glDeleteVertexArrays(1, &gl.vao);
                if gl.ext.timer.is_some() {
                    glDeleteQueries(QUERIES as GLint, gl.queries.as_ptr());
                }
            }
        }
        producer::queue().book.clear();
        pacing::stop();
    }
}

fn free(gl: &Gl, texture: Texture) {
    unsafe {
        if !texture.fence.is_null() {
            (gl.ext.destroy_sync)(gl.display, texture.fence);
        }
        if texture.texture != 0 {
            glDeleteTextures(1, &texture.texture);
        }
        if !texture.image.is_null() {
            (gl.ext.destroy_image)(gl.display, texture.image);
        }
    }
}

impl Surface {
    fn init_gl(&mut self) {
        let ext = match Ext::load() {
            Ok(ext) => ext,
            Err(missing) => {
                error!("oriel-display: {missing} is missing");
                return;
            }
        };
        let programs = unsafe { (program(VERTEX, PATTERN), program(VERTEX, PAGE)) };
        let (pattern, page) = match programs {
            (Ok(pattern), Ok(page)) => (pattern, page),
            (Err(e), _) | (_, Err(e)) => {
                error!("oriel-display: {e}");
                return;
            }
        };
        let gl = unsafe {
            let mut bound = 0;
            glGetIntegerv(GL_CURRENT_PROGRAM, &mut bound);
            glUseProgram(page);
            glUniform1i(glGetUniformLocation(page, c"page".as_ptr()), 0);
            glUseProgram(bound as GLuint);
            let mut vao = 0;
            glGenVertexArrays(1, &mut vao);
            let mut queries = [0; QUERIES];
            if let Some(timer) = &ext.timer {
                (timer.generate)(QUERIES as GLint, queries.as_mut_ptr());
            }
            Gl {
                display: eglGetCurrentDisplay(),
                pattern,
                pattern_t: glGetUniformLocation(pattern, c"t".as_ptr()),
                pattern_viewport: glGetUniformLocation(pattern, c"viewport".as_ptr()),
                page,
                page_viewport: glGetUniformLocation(page, c"viewport".as_ptr()),
                vao,
                queries,
                ext,
            }
        };
        self.start = self.now();
        self.stats.start(self.start);
        info!(
            "oriel-display: GL ready, display {:?} context {:?}, GL {} / {}, timer queries {}",
            gl.display,
            unsafe { eglGetCurrentContext() },
            unsafe { string(GL_RENDERER) },
            unsafe { string(GL_VERSION) },
            if gl.ext.timer.is_some() { "yes" } else { "no" }
        );
        self.gl = Some(gl);
    }

    /// Save and restore Kodi GL state around one pass; return whether a draw was submitted.
    unsafe fn draw(&mut self, gl: &Gl, current: Option<u32>, t0: f64) -> bool {
        unsafe {
            let get = |name| {
                let mut value = 0;
                glGetIntegerv(name, &mut value);
                value
            };
            let program = get(GL_CURRENT_PROGRAM);
            let vao = get(GL_VERTEX_ARRAY_BINDING);
            let active_texture = get(GL_ACTIVE_TEXTURE);
            glActiveTexture(GL_TEXTURE0);
            let texture = get(GL_TEXTURE_BINDING_2D);
            let blend_func =
                [get(GL_BLEND_SRC_RGB), get(GL_BLEND_DST_RGB), get(GL_BLEND_SRC_ALPHA), get(GL_BLEND_DST_ALPHA)];
            let blend = glIsEnabled(GL_BLEND) != 0;
            let mut vp = [0; 4];
            glGetIntegerv(GL_VIEWPORT, vp.as_mut_ptr());
            self.stats.target(get(GL_FRAMEBUFFER_BINDING), vp);

            let timer = gl
                .ext
                .timer
                .as_ref()
                .filter(|_| TIMING.load(Ordering::Relaxed) && self.query_next - self.query_read < QUERIES);
            if let Some(timer) = timer {
                (timer.begin)(GL_TIME_ELAPSED_EXT, gl.queries[self.query_next % QUERIES]);
            }

            // Restore scissor testing for Kodi's current dirty region; its state block
            // disables the test around add-on rendering but preserves the rectangle.
            glEnable(GL_SCISSOR_TEST);
            glEnable(GL_BLEND);
            glBlendFuncSeparate(GL_ONE, GL_ONE_MINUS_SRC_ALPHA, GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
            glBindVertexArray(gl.vao);
            let (x, y, w, h) = (vp[0] as f32, vp[1] as f32, vp[2] as f32, vp[3] as f32);
            let mut drew = false;
            if PATTERN_MODE.load(Ordering::Relaxed) {
                glUseProgram(gl.pattern);
                glUniform1f(gl.pattern_t, (t0 - self.start) as f32);
                glUniform4f(gl.pattern_viewport, x, y, w, h);
                glDrawArrays(GL_TRIANGLES, 0, 3);
                drew = true;
            } else if let Some(page) = current.and_then(|id| self.textures.get_mut(&id)) {
                glUseProgram(gl.page);
                glUniform4f(gl.page_viewport, x, y, w, h);
                glBindTexture(GL_TEXTURE_2D, page.texture);
                glDrawArrays(GL_TRIANGLES, 0, 3);
                drew = true;
                if !page.fence.is_null() {
                    (gl.ext.destroy_sync)(gl.display, page.fence);
                }
                page.fence = (gl.ext.create_sync)(gl.display, EGL_SYNC_FENCE_KHR, std::ptr::null());
                if page.fence.is_null() {
                    // Without a fence, retire() would release this buffer while the GPU still reads it.
                    if !std::mem::replace(&mut self.logged_fence, true) {
                        error!("oriel-display: read fence failed: {:#x}; waiting for the GPU instead", eglGetError());
                    }
                    glFinish();
                }
                let replies = producer::queue().book.drew();
                if !replies.is_empty() {
                    self.stats.new_frame(t0);
                }
                producer::reply(replies);
            }
            if let Some(timer) = timer {
                (timer.end)(GL_TIME_ELAPSED_EXT);
                self.query_next += 1;
            }

            glBindVertexArray(vao as GLuint);
            glUseProgram(program as GLuint);
            glBindTexture(GL_TEXTURE_2D, texture as GLuint);
            glActiveTexture(active_texture as GLenum);
            glBlendFuncSeparate(
                blend_func[0] as GLenum,
                blend_func[1] as GLenum,
                blend_func[2] as GLenum,
                blend_func[3] as GLenum,
            );
            if !blend {
                glDisable(GL_BLEND);
            }
            glDisable(GL_SCISSOR_TEST);
            drew
        }
    }

    /// Import a dma-buf and return its id, or None if the import or texture bind fails.
    fn import(&mut self, gl: &Gl, incoming: Incoming) -> Option<u32> {
        let d = incoming.desc;
        let mut attrs = vec![EGL_WIDTH, d.width, EGL_HEIGHT, d.height, EGL_LINUX_DRM_FOURCC_EXT, d.format as GLint];
        let with_modifier = d.modifier != oriel_display_protocol::MODIFIER_INVALID;
        for (p, &[fd_attr, offset, pitch, lo, hi]) in EGL_DMA_BUF_PLANE.iter().enumerate().take(d.planes as usize) {
            let Some(fd) = incoming.fds.get(p).or(incoming.fds.first()) else { break };
            attrs.extend([fd_attr, fd.as_raw_fd(), offset, d.offsets[p] as GLint, pitch, d.strides[p] as GLint]);
            if with_modifier {
                attrs.extend([lo, d.modifier as u32 as GLint, hi, (d.modifier >> 32) as u32 as GLint]);
            }
        }
        attrs.push(EGL_NONE);
        let image = unsafe {
            (gl.ext.create_image)(
                gl.display,
                std::ptr::null_mut(),
                EGL_LINUX_DMA_BUF_EXT,
                std::ptr::null_mut(),
                attrs.as_ptr(),
            )
        };
        // EGL retains its own descriptor references after image creation.
        drop(incoming.fds);
        if image.is_null() {
            error!(
                "oriel-display: import of buffer {} failed: {:#x} ({}x{} fourcc {:#x} mod {:#x})",
                d.id,
                unsafe { eglGetError() },
                d.width,
                d.height,
                d.format,
                d.modifier
            );
            return None;
        }
        let (texture, error) = unsafe {
            while glGetError() != GL_NO_ERROR {}
            let mut bound = 0;
            glGetIntegerv(GL_TEXTURE_BINDING_2D, &mut bound);
            let mut texture = 0;
            glGenTextures(1, &mut texture);
            glBindTexture(GL_TEXTURE_2D, texture);
            (gl.ext.image_target_texture)(GL_TEXTURE_2D, image);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            glBindTexture(GL_TEXTURE_2D, bound as GLuint);
            (texture, glGetError())
        };
        if error != GL_NO_ERROR {
            error!("oriel-display: binding buffer {} to texture failed: {error:#x}", d.id);
            free(gl, Texture { image, texture, fence: std::ptr::null_mut() });
            return None;
        }
        info!(
            "oriel-display: imported buffer {} {}x{} fourcc {:#x} mod {:#x} planes {} stride {} -> texture {}",
            d.id, d.width, d.height, d.format, d.modifier, d.planes, d.strides[0], texture
        );
        self.stats.import();
        let texture = Texture { image, texture, fence: std::ptr::null_mut() };
        if let Some(old) = self.textures.insert(d.id, texture) {
            free(gl, old);
        }
        Some(d.id)
    }

    /// Apply queued imports, removals and resets, then select the latest frame
    /// and return the current buffer id.
    fn take_from_link(&mut self, gl: &Gl) -> Option<u32> {
        let (buffers, forget, next, reset) = {
            let mut queue = producer::queue();
            let next = queue.book.take_next();
            let reset = std::mem::take(&mut queue.reset);
            if reset {
                queue.book.clear();
            }
            (std::mem::take(&mut queue.buffers), std::mem::take(&mut queue.forget), next, reset)
        };
        if reset {
            for (_, texture) in self.textures.drain() {
                free(gl, texture);
            }
        }
        let imported: Vec<u32> = buffers.into_iter().filter_map(|incoming| self.import(gl, incoming)).collect();
        for id in &forget {
            if let Some(texture) = self.textures.remove(id) {
                free(gl, texture);
            }
        }
        let (current, replies) = {
            let mut queue = producer::queue();
            for id in imported {
                queue.book.imported(id);
            }
            for id in forget {
                queue.book.forget(id);
            }
            let replies = queue.book.show(next);
            (queue.book.current(), replies)
        };
        producer::reply(replies);
        current
    }

    /// Release retired buffers after their EGL fences signal GPU completion.
    fn retire(&mut self) {
        let Some(gl) = self.gl.as_ref() else { return };
        let textures = &mut self.textures;
        let replies = producer::queue().book.retire(|id| {
            if let Some(texture) = textures.get_mut(&id)
                && !texture.fence.is_null()
            {
                let signalled = unsafe { (gl.ext.client_wait_sync)(gl.display, texture.fence, 0, 0) }
                    == EGL_CONDITION_SATISFIED_KHR;
                if !signalled {
                    return false;
                }
                unsafe { (gl.ext.destroy_sync)(gl.display, texture.fence) };
                texture.fence = std::ptr::null_mut();
            }
            true
        });
        producer::reply(replies);
    }

    fn read_queries(&mut self, gl: &Gl) {
        let Some(timer) = &gl.ext.timer else { return };
        while self.query_read < self.query_next {
            let query = gl.queries[self.query_read % QUERIES];
            let mut available = 0;
            unsafe { (timer.get_u32)(query, GL_QUERY_RESULT_AVAILABLE_EXT, &mut available) };
            if available == 0 {
                break;
            }
            let mut ns = 0u64;
            unsafe { (timer.get_u64)(query, GL_QUERY_RESULT_EXT, &mut ns) };
            self.stats.gpu(ns);
            self.query_read += 1;
        }
    }
}
