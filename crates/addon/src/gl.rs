//! GLES 3 and EGL calls used for page rendering in Kodi's GL context.
//! Kodi loads the Mali libraries; resolve extension functions through eglGetProcAddress.

#![allow(non_snake_case)]

use std::ffi::{CStr, c_char, c_int, c_uint, c_void};

pub type GLenum = c_uint;
pub type GLuint = c_uint;
pub type GLint = c_int;
pub type EGLDisplay = *mut c_void;
pub type EGLImage = *mut c_void;
pub type EGLSync = *mut c_void;

pub const GL_NO_ERROR: GLenum = 0;
pub const GL_TRIANGLES: GLenum = 0x0004;
pub const GL_BLEND: GLenum = 0x0BE2;
pub const GL_SCISSOR_TEST: GLenum = 0x0C11;
pub const GL_DEPTH_WRITEMASK: GLenum = 0x0B72;
pub const GL_VIEWPORT: GLenum = 0x0BA2;
pub const GL_ONE: GLenum = 1;
pub const GL_ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;
pub const GL_BLEND_SRC_RGB: GLenum = 0x80C9;
pub const GL_BLEND_DST_RGB: GLenum = 0x80C8;
pub const GL_BLEND_SRC_ALPHA: GLenum = 0x80CB;
pub const GL_BLEND_DST_ALPHA: GLenum = 0x80CA;
pub const GL_CURRENT_PROGRAM: GLenum = 0x8B8D;
pub const GL_VERTEX_ARRAY_BINDING: GLenum = 0x85B5;
pub const GL_ACTIVE_TEXTURE: GLenum = 0x84E0;
pub const GL_TEXTURE0: GLenum = 0x84C0;
pub const GL_TEXTURE_2D: GLenum = 0x0DE1;
pub const GL_TEXTURE_BINDING_2D: GLenum = 0x8069;
pub const GL_FRAMEBUFFER_BINDING: GLenum = 0x8CA6;
pub const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
pub const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
pub const GL_NEAREST: GLint = 0x2600;
pub const GL_CLAMP_TO_EDGE: GLint = 0x812F;
pub const GL_VERTEX_SHADER: GLenum = 0x8B31;
pub const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
pub const GL_COMPILE_STATUS: GLenum = 0x8B81;
pub const GL_LINK_STATUS: GLenum = 0x8B82;
pub const GL_RENDERER: GLenum = 0x1F01;
pub const GL_VERSION: GLenum = 0x1F02;
pub const GL_TIME_ELAPSED_EXT: GLenum = 0x88BF;
pub const GL_QUERY_RESULT_EXT: GLenum = 0x8866;
pub const GL_QUERY_RESULT_AVAILABLE_EXT: GLenum = 0x8867;

pub const EGL_NONE: GLint = 0x3038;
pub const EGL_WIDTH: GLint = 0x3057;
pub const EGL_HEIGHT: GLint = 0x3056;
pub const EGL_LINUX_DMA_BUF_EXT: c_uint = 0x3270;
pub const EGL_LINUX_DRM_FOURCC_EXT: GLint = 0x3271;
pub const EGL_SYNC_FENCE_KHR: c_uint = 0x30F9;
pub const EGL_CONDITION_SATISFIED_KHR: GLint = 0x30F6;
/// EGL attributes for planes 0–3: fd, offset, pitch and modifier low/high words.
pub const EGL_DMA_BUF_PLANE: [[GLint; 5]; 4] = [
    [0x3272, 0x3273, 0x3274, 0x3443, 0x3444],
    [0x3275, 0x3276, 0x3277, 0x3445, 0x3446],
    [0x3278, 0x3279, 0x327A, 0x3447, 0x3448],
    [0x3440, 0x3441, 0x3442, 0x3449, 0x344A],
];

#[link(name = "EGL")]
unsafe extern "C" {
    pub fn eglGetCurrentDisplay() -> EGLDisplay;
    pub fn eglGetCurrentContext() -> *mut c_void;
    pub fn eglGetError() -> GLint;
    fn eglGetProcAddress(name: *const c_char) -> *mut c_void;
}

#[link(name = "GLESv2")]
unsafe extern "C" {
    pub fn glGetError() -> GLenum;
    pub fn glGetString(name: GLenum) -> *const c_char;
    pub fn glGetIntegerv(name: GLenum, value: *mut GLint);
    pub fn glGetBooleanv(name: GLenum, value: *mut u8);
    pub fn glIsEnabled(cap: GLenum) -> u8;
    pub fn glEnable(cap: GLenum);
    pub fn glDisable(cap: GLenum);
    pub fn glBlendFuncSeparate(src_rgb: GLenum, dst_rgb: GLenum, src_alpha: GLenum, dst_alpha: GLenum);
    pub fn glCreateShader(kind: GLenum) -> GLuint;
    pub fn glShaderSource(shader: GLuint, count: GLint, strings: *const *const c_char, lengths: *const GLint);
    pub fn glCompileShader(shader: GLuint);
    pub fn glGetShaderiv(shader: GLuint, name: GLenum, value: *mut GLint);
    pub fn glGetShaderInfoLog(shader: GLuint, size: GLint, length: *mut GLint, log: *mut c_char);
    pub fn glDeleteShader(shader: GLuint);
    pub fn glCreateProgram() -> GLuint;
    pub fn glAttachShader(program: GLuint, shader: GLuint);
    pub fn glLinkProgram(program: GLuint);
    pub fn glGetProgramiv(program: GLuint, name: GLenum, value: *mut GLint);
    pub fn glDeleteProgram(program: GLuint);
    pub fn glUseProgram(program: GLuint);
    pub fn glGetUniformLocation(program: GLuint, name: *const c_char) -> GLint;
    pub fn glUniform1i(location: GLint, value: GLint);
    pub fn glUniform1f(location: GLint, value: f32);
    pub fn glUniform4f(location: GLint, x: f32, y: f32, z: f32, w: f32);
    pub fn glGenVertexArrays(n: GLint, arrays: *mut GLuint);
    pub fn glDeleteVertexArrays(n: GLint, arrays: *const GLuint);
    pub fn glBindVertexArray(array: GLuint);
    pub fn glActiveTexture(unit: GLenum);
    pub fn glGenTextures(n: GLint, textures: *mut GLuint);
    pub fn glDeleteTextures(n: GLint, textures: *const GLuint);
    pub fn glBindTexture(target: GLenum, texture: GLuint);
    pub fn glTexParameteri(target: GLenum, name: GLenum, value: GLint);
    pub fn glDrawArrays(mode: GLenum, first: GLint, count: GLint);
    pub fn glFinish();
    pub fn glDeleteQueries(n: GLint, ids: *const GLuint);
}

/// Extension functions resolved with Kodi's GL context current.
pub struct Ext {
    pub create_image: unsafe extern "C" fn(EGLDisplay, *mut c_void, c_uint, *mut c_void, *const GLint) -> EGLImage,
    pub destroy_image: unsafe extern "C" fn(EGLDisplay, EGLImage) -> c_uint,
    pub image_target_texture: unsafe extern "C" fn(GLenum, EGLImage),
    pub create_sync: unsafe extern "C" fn(EGLDisplay, c_uint, *const GLint) -> EGLSync,
    pub client_wait_sync: unsafe extern "C" fn(EGLDisplay, EGLSync, GLint, u64) -> GLint,
    pub destroy_sync: unsafe extern "C" fn(EGLDisplay, EGLSync) -> c_uint,
    /// Optional EXT_disjoint_timer_query functions for GPU timing.
    pub timer: Option<Timer>,
}

pub struct Timer {
    pub generate: unsafe extern "C" fn(GLint, *mut GLuint),
    pub begin: unsafe extern "C" fn(GLenum, GLuint),
    pub end: unsafe extern "C" fn(GLenum),
    pub get_u32: unsafe extern "C" fn(GLuint, GLenum, *mut GLuint),
    pub get_u64: unsafe extern "C" fn(GLuint, GLenum, *mut u64),
}

fn proc<T>(name: &CStr) -> Option<T> {
    let p = unsafe { eglGetProcAddress(name.as_ptr()) };
    // EGL function addresses and function pointers have matching pointer size.
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy(&p) })
}

impl Ext {
    pub fn load() -> Result<Ext, &'static str> {
        Ok(Ext {
            create_image: proc(c"eglCreateImageKHR").ok_or("eglCreateImageKHR")?,
            destroy_image: proc(c"eglDestroyImageKHR").ok_or("eglDestroyImageKHR")?,
            image_target_texture: proc(c"glEGLImageTargetTexture2DOES").ok_or("glEGLImageTargetTexture2DOES")?,
            create_sync: proc(c"eglCreateSyncKHR").ok_or("eglCreateSyncKHR")?,
            client_wait_sync: proc(c"eglClientWaitSyncKHR").ok_or("eglClientWaitSyncKHR")?,
            destroy_sync: proc(c"eglDestroySyncKHR").ok_or("eglDestroySyncKHR")?,
            timer: (|| {
                Some(Timer {
                    generate: proc(c"glGenQueriesEXT")?,
                    begin: proc(c"glBeginQueryEXT")?,
                    end: proc(c"glEndQueryEXT")?,
                    get_u32: proc(c"glGetQueryObjectuivEXT")?,
                    get_u64: proc(c"glGetQueryObjectui64vEXT")?,
                })
            })(),
        })
    }
}

/// Compile and link shaders, returning the program id or compilation/link error.
pub unsafe fn program(vertex: &CStr, fragment: &CStr) -> Result<GLuint, String> {
    unsafe {
        let program = glCreateProgram();
        for (kind, source) in [(GL_VERTEX_SHADER, vertex), (GL_FRAGMENT_SHADER, fragment)] {
            let shader = glCreateShader(kind);
            glShaderSource(shader, 1, &source.as_ptr(), std::ptr::null());
            glCompileShader(shader);
            let mut ok = 0;
            glGetShaderiv(shader, GL_COMPILE_STATUS, &mut ok);
            if ok == 0 {
                let mut log = [0 as c_char; 1024];
                glGetShaderInfoLog(shader, log.len() as GLint, std::ptr::null_mut(), log.as_mut_ptr());
                return Err(format!("shader: {}", CStr::from_ptr(log.as_ptr()).to_string_lossy()));
            }
            glAttachShader(program, shader);
            glDeleteShader(shader);
        }
        glLinkProgram(program);
        let mut ok = 0;
        glGetProgramiv(program, GL_LINK_STATUS, &mut ok);
        if ok == 0 {
            return Err("link failed".into());
        }
        Ok(program)
    }
}

pub unsafe fn string(name: GLenum) -> String {
    let s = unsafe { glGetString(name) };
    if s.is_null() { String::new() } else { unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned() }
}
