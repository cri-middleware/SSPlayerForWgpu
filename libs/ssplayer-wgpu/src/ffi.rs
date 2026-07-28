//! The "ffi" face (SPEC §11 / §12 P5): the C ABI over the Rust API, for C/C++
//! hosts on DX12 / Vulkan / Metal.
//!
//! **This module is a translation layer and nothing else.** Every entry point
//! does the same three things: validate the C arguments, convert them to Rust
//! types, and call [`NativeHost`] (the wgpu host, SPEC §9.3) or [`SSPlayer`]
//! (playback). No rendering or playback logic lives here — when something needs
//! doing it belongs in `host.rs` or `player.rs`, where the Rust API gets it too.
//!
//! Raw window handles cannot cross the ABI as Rust traits (SPEC §14), so the
//! caller fills in a flat [`SsWindowHandle`] (HWND / NSView / CAMetalLayer /
//! Xlib / Wayland) which [`surface_target`] turns into what wgpu wants.
//!
//! Conventions:
//! * Status is `int32_t`: `SS_OK` (0), positive = an `ssruntime` error code
//!   (same numbering as the SDK's C-API), negative = a player/host error.
//!   [`ss_last_error`] carries the human-readable detail.
//! * No entry point unwinds: the workspace builds with `panic = "abort"`.
//! * Nothing here retains the window; the caller must keep it (and its layer)
//!   alive until [`ss_player_destroy`].

use crate::host::{NativeHost, NativeHostConfig};
use crate::player::SSPlayer;
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::num::NonZeroIsize;
use std::ptr::NonNull;
use wgpu::rwh;

// --- status codes ---------------------------------------------------------

/// Success.
pub const SS_OK: i32 = 0;
/// A pointer argument was null, a string was not UTF-8, or a value was out of range.
pub const SS_ERROR_INVALID_ARGUMENT: i32 = -1;
/// wgpu could not provide what was asked for (no adapter/device, surface creation
/// or presentation failed).
pub const SS_ERROR_GPU: i32 = -2;

// --- window handle kinds --------------------------------------------------

/// Win32 `HWND` (+ optional `HINSTANCE` in `display`).
pub const SS_WINDOW_WIN32: u32 = 0;
/// macOS `NSView*`. wgpu attaches (or reuses) the view's `CAMetalLayer`; must be
/// passed from the main thread.
pub const SS_WINDOW_APPKIT: u32 = 1;
/// macOS `CAMetalLayer*` directly. Apple targets only.
pub const SS_WINDOW_METAL_LAYER: u32 = 2;
/// X11 `Window` XID in `window`, `Display*` in `display`, screen in `screen`.
pub const SS_WINDOW_XLIB: u32 = 3;
/// Wayland `wl_surface*` in `window`, `wl_display*` in `display`.
pub const SS_WINDOW_WAYLAND: u32 = 4;

// --- backends -------------------------------------------------------------

/// Let wgpu pick the platform's primary backend.
pub const SS_BACKEND_AUTO: u32 = 0;
/// Force Direct3D 12 (Windows).
pub const SS_BACKEND_DX12: u32 = 1;
/// Force Metal (macOS / iOS).
pub const SS_BACKEND_METAL: u32 = 2;
/// Force Vulkan (Linux / Windows / Android).
pub const SS_BACKEND_VULKAN: u32 = 3;

// --- C structs ------------------------------------------------------------

/// A platform window, flattened for the ABI. Which fields are read depends on
/// `kind` (`SS_WINDOW_*`); unused ones must be null/zero.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SsWindowHandle {
    /// One of the `SS_WINDOW_*` constants.
    pub kind: u32,
    /// X11 screen number. Ignored unless `kind` is `SS_WINDOW_XLIB`.
    pub screen: i32,
    /// Win32: `HWND`. AppKit: `NSView*`. Metal: `CAMetalLayer*`.
    /// Xlib: the `Window` XID cast to a pointer. Wayland: `wl_surface*`.
    pub window: *mut c_void,
    /// Win32: `HINSTANCE` (may be null). Xlib: `Display*`. Wayland: `wl_display*`.
    /// Unused otherwise.
    pub display: *mut c_void,
}

/// Everything `ss_player_create` needs: where to draw, how big, and how to clear.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SsPlayerDesc {
    /// Target window/layer.
    pub window: SsWindowHandle,
    /// One of the `SS_BACKEND_*` constants.
    pub backend: u32,
    /// Drawable size in **pixels** (not points — scale by the display factor).
    pub width: u32,
    /// See `width`.
    pub height: u32,
    /// Clear colour, straight RGBA in 0..1. Alpha 0 leaves the window transparent
    /// where nothing is drawn.
    pub clear_color: [f32; 4],
    /// Allocate a depth/stencil buffer so mask parts rasterise (SPEC §6). With
    /// `false` the player runs without masking, exactly as if the host attached
    /// no stencil.
    pub enable_mask: bool,
    /// Flip the projection to Y-down (screen coordinates). `false` = Y-up.
    pub y_down: bool,
}

/// Opaque player handle: a [`NativeHost`] plus the buffer backing the string
/// `ss_player_animation_name` hands back to C.
pub struct SsPlayerHost {
    host: NativeHost,
    name_scratch: CString,
}

// --- last error -----------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_last_error(msg: impl Into<Vec<u8>>) {
    let msg = CString::new(msg).unwrap_or_default();
    LAST_ERROR.with(|slot| *slot.borrow_mut() = msg);
}

fn fail(code: i32, msg: impl Into<Vec<u8>>) -> i32 {
    set_last_error(msg);
    code
}

/// The message for the most recent failure **on this thread**.
///
/// The returned string is owned by the library and stays valid until the next
/// failing call on the same thread. Never null; empty when nothing has failed.
///
/// # Safety
/// The caller must not free the pointer or use it past the next library call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr())
}

// --- argument conversion --------------------------------------------------

/// Borrow the player behind a caller-supplied handle.
///
/// # Safety
/// `handle` must be null or a live handle from `ss_player_create`.
unsafe fn player<'a>(handle: *mut SsPlayerHost) -> Option<&'a mut SSPlayer> {
    unsafe { handle.as_mut() }.map(|h| &mut h.host.player)
}

/// Borrow a NUL-terminated UTF-8 string argument.
///
/// # Safety
/// `s` must be null or point at a NUL-terminated string.
unsafe fn c_str<'a>(s: *const c_char) -> Option<&'a str> {
    if s.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(s) }.to_str().ok()
}

fn backend_bits(backend: u32) -> wgpu::Backends {
    match backend {
        SS_BACKEND_DX12 => wgpu::Backends::DX12,
        SS_BACKEND_METAL => wgpu::Backends::METAL,
        SS_BACKEND_VULKAN => wgpu::Backends::VULKAN,
        // AUTO (and anything unknown): every backend wgpu compiled in.
        _ => wgpu::Backends::PRIMARY,
    }
}

/// Turn the flat C window handle into the surface target wgpu takes.
///
/// # Safety
/// `handle` must describe a live window/layer of the advertised `kind`, which
/// must outlive the surface built from the returned target.
unsafe fn surface_target(handle: &SsWindowHandle) -> Result<wgpu::SurfaceTargetUnsafe, String> {
    // A CAMetalLayer has no raw-window-handle representation, so it takes wgpu's
    // dedicated target instead of the RawHandle pair below.
    #[cfg(target_vendor = "apple")]
    if handle.kind == SS_WINDOW_METAL_LAYER {
        if handle.window.is_null() {
            return Err("CAMetalLayer* is null".into());
        }
        return Ok(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(handle.window));
    }

    let window_ptr = NonNull::new(handle.window).ok_or("window handle is null")?;

    let (display, window) = match handle.kind {
        SS_WINDOW_WIN32 => {
            let hwnd = NonZeroIsize::new(handle.window as isize).ok_or("HWND is null")?;
            let mut win = rwh::Win32WindowHandle::new(hwnd);
            win.hinstance = NonZeroIsize::new(handle.display as isize);
            (rwh::RawDisplayHandle::Windows(rwh::WindowsDisplayHandle::new()), rwh::RawWindowHandle::Win32(win))
        }
        SS_WINDOW_APPKIT => (rwh::RawDisplayHandle::AppKit(rwh::AppKitDisplayHandle::new()), rwh::RawWindowHandle::AppKit(rwh::AppKitWindowHandle::new(window_ptr))),
        SS_WINDOW_XLIB => {
            let display = NonNull::new(handle.display);
            (rwh::RawDisplayHandle::Xlib(rwh::XlibDisplayHandle::new(display, handle.screen)), rwh::RawWindowHandle::Xlib(rwh::XlibWindowHandle::new(handle.window as usize as _)))
        }
        SS_WINDOW_WAYLAND => {
            let display = NonNull::new(handle.display).ok_or("wl_display* is null")?;
            (rwh::RawDisplayHandle::Wayland(rwh::WaylandDisplayHandle::new(display)), rwh::RawWindowHandle::Wayland(rwh::WaylandWindowHandle::new(window_ptr)))
        }
        other => return Err(format!("window kind {other} is not supported on this platform (see SS_WINDOW_*)")),
    };
    Ok(wgpu::SurfaceTargetUnsafe::RawHandle { raw_display_handle: Some(display), raw_window_handle: window })
}

// --- lifecycle ------------------------------------------------------------

/// Create a player that renders into the window described by `desc`.
///
/// Returns null on failure; call `ss_last_error` for the reason. The window (and
/// its layer, on macOS) must stay alive until `ss_player_destroy`.
///
/// # Safety
/// `desc` must point at a valid `ss_player_desc_t` whose window handle is live
/// and matches its `kind`. On macOS this must be called from the main thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_create(desc: *const SsPlayerDesc) -> *mut SsPlayerHost {
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        set_last_error("desc is null");
        return std::ptr::null_mut();
    };
    let target = match unsafe { surface_target(&desc.window) } {
        Ok(target) => target,
        Err(msg) => {
            set_last_error(msg);
            return std::ptr::null_mut();
        }
    };
    let config = NativeHostConfig { backends: backend_bits(desc.backend), width: desc.width, height: desc.height, clear_color: desc.clear_color, enable_mask: desc.enable_mask, y_down: desc.y_down };

    // SAFETY: forwarded from this function's contract — the caller keeps the
    // window alive until `ss_player_destroy`.
    match unsafe { NativeHost::new(target, &config) } {
        Ok(host) => Box::into_raw(Box::new(SsPlayerHost { host, name_scratch: CString::default() })),
        Err(msg) => {
            set_last_error(msg);
            std::ptr::null_mut()
        }
    }
}

/// Destroy a player and release its wgpu resources. Null is a no-op.
///
/// # Safety
/// `handle` must come from `ss_player_create` and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_destroy(handle: *mut SsPlayerHost) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

// --- resources ------------------------------------------------------------

/// Register an `.ssab` pack under `pack_name` without changing what plays.
///
/// Every pack an animation can reference must be registered before
/// `ss_player_set_animation`, since external instance parts resolve by pack name
/// (SPEC §7.1). Embedded-mode packs (SPEC §8.1) also carry their atlases; those
/// are decoded here and uploaded on the next `ss_player_update`.
///
/// # Safety
/// `pack_name` must be a NUL-terminated UTF-8 string and `data` must point at
/// `len` readable bytes. Neither is retained beyond the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_add_ssab(handle: *mut SsPlayerHost, pack_name: *const c_char, data: *const u8, len: usize) -> i32 {
    unsafe { load_pack(handle, pack_name, data, len, false) }
}

/// Register `pack_name` and make it the pack `ss_player_set_animation` selects from.
///
/// # Safety
/// As `ss_player_add_ssab`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_load_ssab(handle: *mut SsPlayerHost, pack_name: *const c_char, data: *const u8, len: usize) -> i32 {
    unsafe { load_pack(handle, pack_name, data, len, true) }
}

/// Shared body of `ss_player_add_ssab` / `ss_player_load_ssab`.
///
/// # Safety
/// As `ss_player_add_ssab`.
unsafe fn load_pack(handle: *mut SsPlayerHost, pack_name: *const c_char, data: *const u8, len: usize, main: bool) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player is null") };
    let Some(pack) = (unsafe { c_str(pack_name) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "pack_name is null or not UTF-8") };
    if data.is_null() || len == 0 {
        return fail(SS_ERROR_INVALID_ARGUMENT, "ssab data is empty");
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let result = if main { player.load_ssab(pack, bytes) } else { player.add_ssab(pack, bytes) };
    match result {
        Ok(()) => SS_OK,
        Err(e) => fail(e.as_code(), format!("{pack}: {e}")),
    }
}

/// Supply a texture for a reference-mode pack, keyed by the fnv1a hash the
/// runtime reports in `DrawBatch.texture_hash` (SPEC §8.1). `rgba` is
/// straight-alpha RGBA8, `width * height * 4` bytes; the player premultiplies it.
///
/// Embedded-mode packs need no call: their atlases load with the pack.
///
/// # Safety
/// `rgba` must point at `width * height * 4` readable bytes. It is not retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_add_texture(handle: *mut SsPlayerHost, hash: u32, rgba: *const u8, width: u32, height: u32) -> i32 {
    let Some(host) = (unsafe { handle.as_mut() }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player is null") };
    let len = (width as usize) * (height as usize) * 4;
    if rgba.is_null() || len == 0 {
        return fail(SS_ERROR_INVALID_ARGUMENT, "texture is empty");
    }
    host.host.upload_texture(hash, unsafe { std::slice::from_raw_parts(rgba, len) }, width, height);
    SS_OK
}

// --- playback control -----------------------------------------------------

/// Select the animation to play, rebuilding the instance tree. It is left at its
/// start frame and not playing — call `ss_player_play` to start it.
///
/// This is where playback allocates (contexts, slots), so `ss_player_play` and
/// the other transport calls stay allocation-free.
///
/// # Safety
/// `anim_name` must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_animation(handle: *mut SsPlayerHost, anim_name: *const c_char) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player is null") };
    let Some(name) = (unsafe { c_str(anim_name) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "anim_name is null or not UTF-8") };
    match player.set_animation(name) {
        Ok(()) => SS_OK,
        Err(e) => fail(e.as_code(), format!("{name}: {e}")),
    }
}

/// Start — or, after `ss_player_pause`, resume — the selected animation.
/// Does nothing until `ss_player_set_animation` has succeeded.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_play(handle: *mut SsPlayerHost) {
    if let Some(player) = unsafe { player(handle) } {
        player.play();
    }
}

/// Freeze playback where it is; `ss_player_play` resumes from there.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_pause(handle: *mut SsPlayerHost) {
    if let Some(player) = unsafe { player(handle) } {
        player.pause();
    }
}

/// Stop playback and rewind to the animation's start frame.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_stop(handle: *mut SsPlayerHost) {
    if let Some(player) = unsafe { player(handle) } {
        player.stop();
    }
}

/// Scale playback speed (1.0 = authored speed, negative plays backwards).
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_speed(handle: *mut SsPlayerHost, speed: f32) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_speed(speed);
    }
}

/// Whether the root animation is advancing.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_is_playing(handle: *mut SsPlayerHost) -> bool {
    unsafe { player(handle) }.map(|p| p.is_playing()).unwrap_or(false)
}

/// How many animations the main pack holds (0 if nothing is loaded).
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_animation_count(handle: *mut SsPlayerHost) -> u32 {
    unsafe { player(handle) }.and_then(|p| p.animation_names()).map(|names| names.len() as u32).unwrap_or(0)
}

/// The `index`-th animation name of the main pack, or null if out of range.
///
/// The string is owned by the player and stays valid until the next call to this
/// function on the same player. Copy it if you need to keep it.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_animation_name(handle: *mut SsPlayerHost, index: u32) -> *const c_char {
    let Some(host) = (unsafe { handle.as_mut() }) else {
        set_last_error("player is null");
        return std::ptr::null();
    };
    let Some(name) = host.host.player.animation_names().and_then(|names| names.into_iter().nth(index as usize)) else {
        set_last_error(format!("no animation at index {index}"));
        return std::ptr::null();
    };
    let Ok(name) = CString::new(name) else {
        set_last_error("animation name contains an interior NUL");
        return std::ptr::null();
    };
    host.name_scratch = name;
    host.name_scratch.as_ptr()
}

/// Canvas size of the animation currently playing. Either out-pointer may be null.
///
/// # Safety
/// `width` / `height` must be null or point at writable `float`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_canvas_size(handle: *mut SsPlayerHost, width: *mut f32, height: *mut f32) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player is null") };
    let Some((w, h)) = player.canvas_size() else { return fail(SS_ERROR_INVALID_ARGUMENT, "no animation is playing") };
    if let Some(out) = unsafe { width.as_mut() } {
        *out = w;
    }
    if let Some(out) = unsafe { height.as_mut() } {
        *out = h;
    }
    SS_OK
}

// --- per frame ------------------------------------------------------------

/// Advance the playback tree by `delta_seconds` and build this frame's GPU buffers.
/// Call once per frame, before `ss_player_render`.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_update(handle: *mut SsPlayerHost, delta_seconds: f32) {
    if let Some(host) = unsafe { handle.as_mut() } {
        host.host.update(delta_seconds);
    }
}

/// Acquire the swapchain image, draw the prepared frame into it, and present.
///
/// A lost/outdated surface is absorbed by reconfiguring (SPEC §14): the call
/// returns `SS_OK` having skipped one frame.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_render(handle: *mut SsPlayerHost) -> i32 {
    let Some(host) = (unsafe { handle.as_mut() }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player is null") };
    match host.host.render() {
        Ok(()) => SS_OK,
        Err(msg) => fail(SS_ERROR_GPU, msg),
    }
}

/// Resize the swapchain, the depth/stencil buffer and the projection to a new
/// drawable size in **pixels**. Zero in either dimension is ignored.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_resize(handle: *mut SsPlayerHost, width: u32, height: u32) {
    if let Some(host) = unsafe { handle.as_mut() } {
        host.host.resize(width, height);
    }
}
