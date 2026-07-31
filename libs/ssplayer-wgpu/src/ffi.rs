//! The "ffi" face (SPEC §11 / §12 P5): the C ABI over the Rust API, for C/C++
//! hosts on DX12 / Vulkan / Metal.
//!
//! **This module is a translation layer and nothing else.** Every entry point
//! does the same three things: validate the C arguments, convert them to Rust
//! types, and call [`NativeHost`] (the wgpu host, SPEC §9.4) or [`SSPlayer`]
//! (playback). No rendering or playback logic lives here — when something needs
//! doing it belongs in `host.rs` or `player.rs`, where the Rust API gets it too.
//!
//! Raw window handles cannot cross the ABI as Rust traits (SPEC §14), so the
//! caller fills in a flat [`SsWindowHandle`] (HWND / NSView / CAMetalLayer /
//! Xlib / Wayland) which [`surface_target`] turns into what wgpu wants.
//!
//! **Two handles, not one (SPEC §11).** `ss_scene_t` is the window — surface,
//! device, renderer, scene — and `ss_player_t` is one character drawn into it.
//! They used to be the same object, which made a second character in one window
//! structurally impossible: creating one meant creating a second surface on the
//! same window, and the two cleared and presented over each other.
//!
//! Conventions:
//! * Status is `int32_t`: `SS_OK` (0), positive = an `ssruntime` error code
//!   (same numbering as the SDK's C-API), negative = a player/host error.
//!   [`ss_last_error`] carries the human-readable detail.
//! * No entry point unwinds: the workspace builds with `panic = "abort"`.
//! * Nothing here retains the window; the caller must keep it (and its layer)
//!   alive until [`ss_scene_destroy`].

use crate::host::{NativeHost, NativeHostConfig, PlayerId};
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

/// Everything `ss_scene_create` needs: where to draw, how big, and how to clear.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SsSceneDesc {
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
    /// `false` the scene runs without masking, exactly as if the host attached
    /// no stencil.
    pub enable_mask: bool,
    /// Flip the projection to Y-down (screen coordinates). `false` = Y-up.
    pub y_down: bool,
}

/// What the last frame cost (SPEC §14.2 item 10), flattened for the ABI.
///
/// `gpu_pass_time_ns` is 0 when the adapter has no timestamp query, or before
/// the first measurement has come back — it is read without stalling the
/// pipeline, so it lags a frame or two.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SsSceneStats {
    /// Players that contributed geometry this frame.
    pub players: u32,
    /// Primitives in the frame: parts, shape fans and effect particles.
    pub prims: u32,
    /// Draw calls recorded. The gap from `prims` is draw-call merging headroom.
    pub draw_calls: u32,
    pub _reserved: u32,
    /// Bytes written to the frame's storage buffers.
    pub uploaded_bytes: u64,
    /// GPU time for the last timed render pass, in nanoseconds. 0 = no reading.
    pub gpu_pass_time_ns: u64,
}

/// One atlas a pack asks for (SPEC §8.1), flattened for the ABI. What
/// `ss_player_texture_at` fills in.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SsTextureRef {
    /// The key `ss_scene_add_texture` / `ss_scene_add_texture_png` expects for
    /// this image — the cellmap's fnv1a hash, as `DrawBatch.texture_hash` reports it.
    pub hash: u32,
    /// `true` when the image rides inside the `.ssab` (embedded mode): the pack
    /// supplies it on the next `ss_scene_update` and the host loads nothing.
    pub embedded: bool,
    /// The image path the exporter wrote, **relative to the `.ssab`'s own
    /// directory** — join it with the directory the file was read from.
    ///
    /// Owned by the scene and valid until the next `ss_player_texture_at` on it;
    /// copy it if you need to keep it. Never null.
    pub path: *const c_char,
}

/// Opaque scene handle: one window's [`NativeHost`], the player handles it has
/// issued, and the buffer backing the string `ss_player_animation_name` hands
/// back to C.
pub struct SsScene {
    host: NativeHost,
    /// Every handle this scene created, alive until the scene is destroyed.
    ///
    /// `ss_player_destroy` deliberately does **not** free the box: a handle that
    /// outlives its player then resolves to "no such player" instead of reading
    /// freed memory, so a double destroy — the easy mistake — is a safe no-op.
    handles: Vec<*mut SsPlayer>,
    name_scratch: CString,
    /// Backs `ss_player_texture_at`'s `path`. Separate from `name_scratch` so the
    /// two "valid until the next call" contracts do not invalidate each other:
    /// walking a pack's textures and its animation names interleaved is normal.
    path_scratch: CString,
}

/// Opaque player handle: which scene, and which player within it.
///
/// Not a pointer into the scene's player list. That list moves as it grows, and
/// slots are reused, so the id carries a generation and every call re-resolves it
/// (SPEC §11).
pub struct SsPlayer {
    scene: *mut SsScene,
    id: PlayerId,
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

/// Borrow the scene behind a caller-supplied player handle.
///
/// # Safety
/// `handle` must be null or a handle from `ss_scene_create_player` whose scene
/// is still alive.
unsafe fn scene_of<'a>(handle: *mut SsPlayer) -> Option<(&'a mut SsScene, PlayerId)> {
    let handle = unsafe { handle.as_mut() }?;
    let scene = unsafe { handle.scene.as_mut() }?;
    Some((scene, handle.id))
}

/// Borrow the player behind a caller-supplied handle, or `None` if it has been
/// destroyed (SPEC §11 — a stale handle resolves, it does not dangle).
///
/// # Safety
/// As [`scene_of`].
unsafe fn player<'a>(handle: *mut SsPlayer) -> Option<&'a mut SSPlayer> {
    let (scene, id) = unsafe { scene_of(handle) }?;
    scene.host.player_mut(id)
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

/// Borrow an *optional* string argument, where null means "the default" rather
/// than a mistake — so a bad encoding is still reported instead of silently
/// becoming the default.
///
/// # Safety
/// As [`c_str`].
unsafe fn optional_c_str<'a>(s: *const c_char) -> Result<Option<&'a str>, &'static str> {
    if s.is_null() {
        return Ok(None);
    }
    unsafe { c_str(s) }.map(Some).ok_or("string is not UTF-8")
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

/// Create a scene that renders into the window described by `desc`.
///
/// Returns null on failure; call `ss_last_error` for the reason. The window (and
/// its layer, on macOS) must stay alive until `ss_scene_destroy`.
///
/// A scene holds no player of its own — add them with `ss_scene_create_player`,
/// as many as the window should show.
///
/// # Safety
/// `desc` must point at a valid `ss_scene_desc_t` whose window handle is live
/// and matches its `kind`. On macOS this must be called from the main thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_create(desc: *const SsSceneDesc) -> *mut SsScene {
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
    // window alive until `ss_scene_destroy`.
    match unsafe { NativeHost::new(target, &config) } {
        Ok(host) => Box::into_raw(Box::new(SsScene { host, handles: Vec::new(), name_scratch: CString::default(), path_scratch: CString::default() })),
        Err(msg) => {
            set_last_error(msg);
            std::ptr::null_mut()
        }
    }
}

/// Destroy a scene, its players and its wgpu resources. Null is a no-op.
///
/// **Every `ss_player_t` this scene issued becomes invalid**, including handles
/// the caller still holds — using one afterwards reads freed memory. Drop them
/// with the scene.
///
/// # Safety
/// `handle` must come from `ss_scene_create` and must not be used afterwards,
/// nor must any player handle it created.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_destroy(handle: *mut SsScene) {
    if handle.is_null() {
        return;
    }
    let scene = unsafe { Box::from_raw(handle) };
    for player in &scene.handles {
        drop(unsafe { Box::from_raw(*player) });
    }
    drop(scene);
}

/// Add a player to `scene` and return its handle, or null if `scene` is null.
///
/// One scene draws any number of players, in ascending `ss_player_set_order`.
/// They share the window's device, renderer and atlas textures, so the second
/// character of a pack costs no upload and no extra pipeline (SPEC §1.2).
///
/// The handle stays valid until `ss_player_destroy` or `ss_scene_destroy`.
///
/// # Safety
/// `scene` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_create_player(scene: *mut SsScene) -> *mut SsPlayer {
    let Some(scene_ref) = (unsafe { scene.as_mut() }) else {
        set_last_error("scene is null");
        return std::ptr::null_mut();
    };
    let id = scene_ref.host.create_player();
    let handle = Box::into_raw(Box::new(SsPlayer { scene, id }));
    scene_ref.handles.push(handle);
    handle
}

/// Remove a player from its scene. Null, or a handle already destroyed, is a
/// safe no-op — the handle itself stays allocated until the scene goes.
///
/// # Safety
/// `handle` must be null or come from `ss_scene_create_player` on a scene that
/// is still alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_destroy(handle: *mut SsPlayer) {
    if let Some((scene, id)) = unsafe { scene_of(handle) } {
        scene.host.destroy_player(id);
    }
}

/// How many players `scene` currently holds.
///
/// # Safety
/// `scene` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_player_count(scene: *mut SsScene) -> u32 {
    unsafe { scene.as_ref() }.map(|s| s.host.player_count() as u32).unwrap_or(0)
}

// --- resources ------------------------------------------------------------

/// Register an `.ssab` pack under `pack_name` without changing what plays.
///
/// Every pack an animation can reference must be registered before
/// `ss_player_set_animation`, since external instance parts resolve by pack name
/// (SPEC §7.1). Embedded-mode packs (SPEC §8.1) also carry their atlases; those
/// are decoded here and uploaded on the next `ss_scene_update`.
///
/// # Safety
/// `pack_name` must be a NUL-terminated UTF-8 string and `data` must point at
/// `len` readable bytes. Neither is retained beyond the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_add_ssab(handle: *mut SsPlayer, pack_name: *const c_char, data: *const u8, len: usize) -> i32 {
    unsafe { load_pack(handle, pack_name, data, len, false) }
}

/// Register `pack_name` and make it the pack `ss_player_set_animation` selects from.
///
/// # Safety
/// As `ss_player_add_ssab`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_load_ssab(handle: *mut SsPlayer, pack_name: *const c_char, data: *const u8, len: usize) -> i32 {
    unsafe { load_pack(handle, pack_name, data, len, true) }
}

/// Shared body of `ss_player_add_ssab` / `ss_player_load_ssab`.
///
/// # Safety
/// As `ss_player_add_ssab`.
unsafe fn load_pack(handle: *mut SsPlayer, pack_name: *const c_char, data: *const u8, len: usize, main: bool) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
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
/// straight-alpha RGBA8, `width * height * 4` bytes; it is premultiplied on the
/// way to the GPU.
///
/// Registered on the **scene**, because the texture map is device-scoped: every
/// player in the window sees it, and a pack shown five times is uploaded once
/// (SPEC §1.2). Embedded-mode packs need no call — their atlases load with the
/// pack.
///
/// # Safety
/// `rgba` must point at `width * height * 4` readable bytes. It is not retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_add_texture(scene: *mut SsScene, hash: u32, rgba: *const u8, width: u32, height: u32) -> i32 {
    let Some(scene) = (unsafe { scene.as_mut() }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "scene is null") };
    let len = (width as usize) * (height as usize) * 4;
    if rgba.is_null() || len == 0 {
        return fail(SS_ERROR_INVALID_ARGUMENT, "texture is empty");
    }
    scene.host.upload_texture(hash, unsafe { std::slice::from_raw_parts(rgba, len) }, width, height);
    SS_OK
}

/// `ss_scene_add_texture` from **encoded** image bytes (PNG) instead of raw RGBA:
/// the library decodes, premultiplies and uploads.
///
/// It carries a PNG decoder for embedded packs anyway (SPEC §8.1), so a C host
/// showing a reference-mode `.ssab` does not have to bring ImageIO / WIC /
/// libpng along just to hand the pixels over. Reading the file is still the
/// host's job — nothing here opens one (SPEC §8.2).
///
/// # Safety
/// `data` must point at `len` readable bytes. It is not retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_add_texture_png(scene: *mut SsScene, hash: u32, data: *const u8, len: usize) -> i32 {
    let Some(scene) = (unsafe { scene.as_mut() }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "scene is null") };
    if data.is_null() || len == 0 {
        return fail(SS_ERROR_INVALID_ARGUMENT, "image is empty");
    }
    match scene.host.upload_texture_encoded(hash, unsafe { std::slice::from_raw_parts(data, len) }) {
        Ok(()) => SS_OK,
        Err(msg) => fail(SS_ERROR_INVALID_ARGUMENT, msg),
    }
}

/// How many atlases a registered pack asks for (SPEC §8.1). `pack_name` null
/// selects the main pack; 0 means nothing is registered under that name.
///
/// # Safety
/// `pack_name` must be null or a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_texture_count(handle: *mut SsPlayer, pack_name: *const c_char) -> u32 {
    let Some(player) = (unsafe { player(handle) }) else {
        set_last_error("player handle is null or destroyed");
        return 0;
    };
    let pack = match unsafe { optional_c_str(pack_name) } {
        Ok(pack) => pack,
        Err(msg) => {
            set_last_error(msg);
            return 0;
        }
    };
    player.textures(pack).map(|textures| textures.len() as u32).unwrap_or(0)
}

/// Describe the `index`-th atlas of a registered pack, so a reference-mode host
/// knows *what* to load and *where the exporter put it* (SPEC §8.1).
///
/// This is the half of texture supply only the `.ssab` can answer, and it is the
/// same shape the sibling players expose: register the pack, walk this list, and
/// for every entry whose `embedded` is false read
/// `<directory the .ssab came from>/<path>` and hand it to
/// `ss_scene_add_texture_png`. Embedded entries need no call at all.
///
/// `pack_name` null selects the main pack. Writes `out` and returns `SS_OK`, or
/// leaves it untouched and reports the reason.
///
/// # Safety
/// `pack_name` must be null or a NUL-terminated UTF-8 string, and `out` must
/// point at a writable `ss_texture_ref_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_texture_at(handle: *mut SsPlayer, pack_name: *const c_char, index: u32, out: *mut SsTextureRef) -> i32 {
    if out.is_null() {
        return fail(SS_ERROR_INVALID_ARGUMENT, "out is null");
    }
    let Some((scene, id)) = (unsafe { scene_of(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    let pack = match unsafe { optional_c_str(pack_name) } {
        Ok(pack) => pack,
        Err(msg) => return fail(SS_ERROR_INVALID_ARGUMENT, msg),
    };

    // Copied out before the scratch is touched: the borrow of the player ends
    // here, and `scene` is needed mutably to own the string handed back.
    let Some(texture) = scene.host.player_mut(id).and_then(|p| p.textures(pack)).and_then(|textures| textures.get(index as usize)).cloned() else {
        return fail(SS_ERROR_INVALID_ARGUMENT, format!("no texture at index {index}"));
    };
    let Ok(path) = CString::new(texture.path) else {
        return fail(SS_ERROR_INVALID_ARGUMENT, "image path contains an interior NUL");
    };
    scene.path_scratch = path;
    unsafe { out.write(SsTextureRef { hash: texture.hash, embedded: texture.embedded, path: scene.path_scratch.as_ptr() }) };
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
pub unsafe extern "C" fn ss_player_set_animation(handle: *mut SsPlayer, anim_name: *const c_char) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
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
pub unsafe extern "C" fn ss_player_play(handle: *mut SsPlayer) {
    if let Some(player) = unsafe { player(handle) } {
        player.play();
    }
}

/// Freeze playback where it is; `ss_player_play` resumes from there.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_pause(handle: *mut SsPlayer) {
    if let Some(player) = unsafe { player(handle) } {
        player.pause();
    }
}

/// Stop playback and rewind to the animation's start frame.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_stop(handle: *mut SsPlayer) {
    if let Some(player) = unsafe { player(handle) } {
        player.stop();
    }
}

/// Scale playback speed (1.0 = authored speed, negative plays backwards).
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_speed(handle: *mut SsPlayer, speed: f32) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_speed(speed);
    }
}

/// Whether the root animation is advancing.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_is_playing(handle: *mut SsPlayer) -> bool {
    unsafe { player(handle) }.map(|p| p.is_playing()).unwrap_or(false)
}

/// How many animations the main pack holds (0 if nothing is loaded).
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_animation_count(handle: *mut SsPlayer) -> u32 {
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
pub unsafe extern "C" fn ss_player_animation_name(handle: *mut SsPlayer, index: u32) -> *const c_char {
    let Some((scene, id)) = (unsafe { scene_of(handle) }) else {
        set_last_error("player handle is null or destroyed");
        return std::ptr::null();
    };
    let Some(name) = scene.host.player_mut(id).and_then(|p| p.animation_names()).and_then(|names| names.into_iter().nth(index as usize)) else {
        set_last_error(format!("no animation at index {index}"));
        return std::ptr::null();
    };
    let Ok(name) = CString::new(name) else {
        set_last_error("animation name contains an interior NUL");
        return std::ptr::null();
    };
    scene.name_scratch = name;
    scene.name_scratch.as_ptr()
}

// --- placement (SPEC §9.3) ------------------------------------------------

/// Place this player in its scene: translation in pixels from the centre, scale,
/// then rotation in radians.
///
/// Without this every player sits at the origin on top of the others, which is
/// why the old one-player-per-window API never needed it.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_transform(handle: *mut SsPlayer, x: f32, y: f32, scale_x: f32, scale_y: f32, rotation: f32) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_transform(x, y, scale_x, scale_y, rotation);
    }
}

/// Draw order within the scene; lower draws first. Ties keep the order the
/// players were created in.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_order(handle: *mut SsPlayer, order: i32) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_order(order);
    }
}

/// Skip this player entirely when `false`: its timeline does not advance and it
/// produces no geometry. For a player the caller knows cannot be seen; use
/// `ss_player_set_alpha(0)` for a fade that keeps playing.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_visible(handle: *mut SsPlayer, visible: bool) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_visible(visible);
    }
}

/// Player-level fade, clamped to 0..1. Mask coverage is unaffected: a faded
/// player clips exactly as it would at full opacity.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_alpha(handle: *mut SsPlayer, alpha: f32) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_alpha(alpha);
    }
}

/// Canvas size of the animation currently playing. Either out-pointer may be null.
///
/// # Safety
/// `width` / `height` must be null or point at writable `float`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_canvas_size(handle: *mut SsPlayer, width: *mut f32, height: *mut f32) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
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

/// Advance every player in the scene by `delta_seconds` and build this frame's
/// GPU buffers. Call once per frame, before `ss_scene_render`.
///
/// # Safety
/// `scene` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_update(scene: *mut SsScene, delta_seconds: f32) {
    if let Some(scene) = unsafe { scene.as_mut() } {
        scene.host.update(delta_seconds);
    }
}

/// Acquire the swapchain image, draw the prepared frame into it, and present.
///
/// One pass covers every player, in ascending `ss_player_set_order`.
///
/// A lost/outdated surface is absorbed by reconfiguring (SPEC §14): the call
/// returns `SS_OK` having skipped one frame.
///
/// # Safety
/// `scene` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_render(scene: *mut SsScene) -> i32 {
    let Some(scene) = (unsafe { scene.as_mut() }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "scene is null") };
    match scene.host.render() {
        Ok(()) => SS_OK,
        Err(msg) => fail(SS_ERROR_GPU, msg),
    }
}

/// What the last frame cost (SPEC §14.2 item 10). Writes `out` and returns
/// `SS_OK`, or leaves it untouched and reports the reason.
///
/// # Safety
/// `out` must point at a writable `ss_scene_stats_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_stats(scene: *mut SsScene, out: *mut SsSceneStats) -> i32 {
    let Some(scene) = (unsafe { scene.as_ref() }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "scene is null") };
    let Some(out) = (unsafe { out.as_mut() }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "out is null") };
    let stats = scene.host.stats();
    *out = SsSceneStats { players: stats.players, prims: stats.prims, draw_calls: stats.draw_calls, _reserved: 0, uploaded_bytes: stats.uploaded_bytes, gpu_pass_time_ns: stats.gpu_pass_time_ns.unwrap_or(0) };
    SS_OK
}

/// Resize the swapchain, the depth/stencil buffer and the projection to a new
/// drawable size in **pixels**. Zero in either dimension is ignored.
///
/// # Safety
/// `scene` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_resize(scene: *mut SsScene, width: u32, height: u32) {
    if let Some(scene) = unsafe { scene.as_mut() } {
        scene.host.resize(width, height);
    }
}
