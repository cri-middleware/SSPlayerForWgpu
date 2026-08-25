//! The "ffi" face: the C ABI over the Rust API, for C/C++
//! hosts on DX12 / Vulkan / Metal.
//!
//! **This module is a translation layer and nothing else.** Every entry point
//! does the same three things: validate the C arguments, convert them to Rust
//! types, and call [`NativeHost`] (the wgpu host) or [`SSPlayer`]
//! (playback). No rendering or playback logic lives here — when something needs
//! doing it belongs in `host.rs` or `player.rs`, where the Rust API gets it too.
//!
//! Raw window handles cannot cross the ABI as Rust traits, so the
//! caller fills in a flat [`SsWindowHandle`] (HWND / NSView / CAMetalLayer /
//! Xlib / Wayland) which [`surface_target`] turns into what wgpu wants.
//!
//! **Two handles, not one.** `ss_scene_t` is the window — surface, device,
//! renderer, scene — and `ss_player_t` is one character drawn into it. Fusing
//! them would make a second character in one window need a second surface, and
//! the two would clear and present over each other.
//!
//! Conventions:
//! * Status is `int32_t`: `SS_OK` (0), positive = an `ssruntime` error code
//! (same numbering as the SDK's C-API), negative = a player/host error.
//! [`ss_last_error`] carries the human-readable detail.
//! * No entry point unwinds: the workspace builds with `panic = "abort"`.
//! * Nothing here retains the window; the caller must keep it (and its layer)
//! alive until [`ss_scene_destroy`].

use crate::events::{SignalValue, SsEvent};
use crate::host::{NativeHost, NativeHostConfig, PlayerId};
use crate::player::{BlendOp, MissingKind, OverridePriority, PlaybackDirection, PlaybackStyle, SSPlayer};
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
    /// Allocate a depth/stencil buffer so mask parts rasterise. With
    /// `false` the scene runs without masking, exactly as if the host attached
    /// no stencil.
    pub enable_mask: bool,
    /// Flip the projection to Y-down (screen coordinates). `false` = Y-up.
    pub y_down: bool,
}

/// What the last frame cost, flattened for the ABI.
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

/// One atlas a pack asks for, flattened for the ABI. What
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

/// Something a load needed and did not get, flattened for the ABI.
/// What `ss_player_missing_at` fills in.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SsMissingAsset {
    /// `SS_MISSING_TEXTURE` or `SS_MISSING_PACK`.
    pub kind: u32,
    /// For a texture, the key a draw batch will ask for. Zero for a pack.
    pub hash: u32,
    /// The path asked for, or the pack's name.
    ///
    /// For a texture this is the path the exporter wrote, **relative to its
    /// pack's directory** — the walk hands the loader bare names and never
    /// builds a full path, so no full path exists to report. Join it
    /// onto the directory of what you passed `ss_player_load` if you want one.
    ///
    /// Owned by the scene and valid until the next `ss_player_missing_at` on it;
    /// copy it if you need to keep it. Never null.
    pub name: *const c_char,
}

/// What kind of thing an `ss_event_t` is; read it before the payload.
pub const SS_EVENT_USER_DATA: u32 = 0;
/// See `SS_EVENT_USER_DATA`.
pub const SS_EVENT_SIGNAL: u32 = 1;
/// See `SS_EVENT_USER_DATA`.
pub const SS_EVENT_AUDIO: u32 = 2;

/// One timeline event, flattened for the ABI.
///
/// The three kinds share one struct rather than a real union, because a union
/// across the ABI costs the caller a cast for no gain at this size. `kind` says
/// which fields carry anything; the rest are zeroed.
///
/// `text` is owned by the scene and valid until the next `ss_player_event_at`
/// on it — copy it if you need to keep it. Never null.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SsEventData {
    /// One of the `SS_EVENT_*` constants.
    pub kind: u32,
    /// Which part raised it.
    pub part_index: u32,

    /// UserData: whether the animator filled each optional field in. Unset is
    /// not the same as zero, which is what these distinguish.
    pub has_integer: bool,
    pub has_rect: bool,
    pub has_point: bool,
    pub has_text: bool,
    /// UserData, when `has_integer`.
    pub integer: i32,
    /// UserData, when `has_rect`: `[x1, y1, x2, y2]` — two corners, not origin+size.
    pub rect: [f32; 4],
    /// UserData, when `has_point`.
    pub point: [f32; 2],

    /// UserData's `str` (when `has_text`), Signal's `command_id`, or Audio's
    /// `sound_name`. Empty for the kinds that have none.
    pub text: *const c_char,

    /// Signal: how many parameters `ss_player_event_signal_param_at` will serve.
    pub param_count: u32,

    /// Audio: addresses the `SoundList` and the `SoundFile` within it.
    pub sound_list_name_hash: u32,
    pub sound_name_hash: u32,
    /// Audio: how many times to play. A count, never zero — SpriteStudio has no
    /// infinite audio loop.
    pub loop_num: i32,
}

/// Which field of an `ss_signal_param_t` carries the value.
pub const SS_SIGNAL_VALUE_NONE: u32 = 0;
/// See `SS_SIGNAL_VALUE_NONE`.
pub const SS_SIGNAL_VALUE_BOOL: u32 = 1;
/// A choice from a list, as its index. See `SS_SIGNAL_VALUE_NONE`.
pub const SS_SIGNAL_VALUE_INDEX: u32 = 2;
/// See `SS_SIGNAL_VALUE_NONE`.
pub const SS_SIGNAL_VALUE_INTEGER: u32 = 3;
/// See `SS_SIGNAL_VALUE_NONE`.
pub const SS_SIGNAL_VALUE_FLOATING: u32 = 4;
/// See `SS_SIGNAL_VALUE_NONE`.
pub const SS_SIGNAL_VALUE_TEXT: u32 = 5;

/// One signal parameter, flattened for the ABI. `value_type` says which field
/// to read; `SS_SIGNAL_VALUE_NONE` means the parameter exists but was left
/// unset.
///
/// `id` and `text` are owned by the scene and valid until the next
/// `ss_player_event_signal_param_at` on it. Never null.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SsSignalParamData {
    pub id: *const c_char,
    /// One of the `SS_SIGNAL_VALUE_*` constants.
    pub value_type: u32,
    /// When `value_type` is `BOOL`.
    pub bool_value: bool,
    /// When `value_type` is `INDEX` or `INTEGER`.
    pub int_value: i32,
    /// When `value_type` is `FLOATING`.
    pub float_value: f32,
    /// When `value_type` is `TEXT`.
    pub text: *const c_char,
}

/// An atlas that did not load. The parts using it draw against the white
/// fallback, so they appear as silhouettes.
pub const SS_MISSING_TEXTURE: u32 = 0;
/// An animation pack that did not load. The instance parts playing it do not appear.
pub const SS_MISSING_PACK: u32 = 1;

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
    /// Backs `ss_player_missing_at`'s `name`, for the same reason `path_scratch`
    /// is separate: a caller walking the missing list while reading texture paths
    /// must not have one invalidate the other.
    missing_scratch: CString,
    /// Backs `ss_player_event_at`'s `text`, and the two below back
    /// `ss_player_event_signal_param_at`. Three separate slots for the same
    /// reason the ones above are separate: reading an event's payload must not
    /// invalidate the parameter a caller is part-way through walking.
    ///
    /// [`CScratch`] rather than `CString`, unlike the three above: these three
    /// are on the frame path — a host drains the event list every tick — and the
    /// ones above are walked once, after a load.
    event_text_scratch: CScratch,
    param_id_scratch: CScratch,
    param_text_scratch: CScratch,
}

/// A NUL-terminated string handed back to C, refilled in place.
///
/// `CString` cannot be refilled: assigning a new one frees the old buffer and
/// allocates another, which is an allocation per call on a path called once per
/// event per frame. The bytes live in a `Vec` that keeps its capacity instead,
/// so a steady stream of events reuses one buffer.
#[derive(Default)]
struct CScratch(Vec<u8>);

impl CScratch {
    /// Refill with `s`, and return a pointer good until the next call on this
    /// slot — the same contract the `CString` fields carry.
    ///
    /// A string with an interior NUL comes back empty, which is what
    /// `CString::new(..).unwrap_or_default()` did here before: C cannot be handed
    /// the tail past a NUL, and quietly delivering only the head would be a
    /// different string wearing the right name.
    fn set(&mut self, s: &str) -> *const c_char {
        self.0.clear();
        if !s.as_bytes().contains(&0) {
            self.0.extend_from_slice(s.as_bytes());
        }
        self.0.push(0);
        self.0.as_ptr().cast()
    }
}

/// Opaque player handle: which scene, and which player within it.
///
/// Not a pointer into the scene's player list. That list moves as it grows, and
/// slots are reused, so the id carries a generation and every call re-resolves it.
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
/// destroyed (a stale handle resolves, it does not dangle).
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
        Ok(host) => Box::into_raw(Box::new(SsScene {
            host,
            handles: Vec::new(),
            name_scratch: CString::default(),
            path_scratch: CString::default(),
            missing_scratch: CString::default(),
            event_text_scratch: CScratch::default(),
            param_id_scratch: CScratch::default(),
            param_text_scratch: CScratch::default(),
        })),
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
/// character of a pack costs no upload and no extra pipeline.
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

/// Load a `.ssab` from `path` **and everything it depends on**, then select it.
///
/// The whole of loading in one call: the pack, both of its texture tables in
/// either export mode, and the sibling packs its instance parts play from,
/// transitively. Nothing is left for the caller to discover — no
/// `ss_player_texture_at` loop, no `ss_scene_add_texture_png` per atlas.
///
/// ```c
/// ss_player_load(player, "assets/Instance.ssab");
/// ss_player_set_animation(player, "anime_1");
/// ss_player_play(player);
/// ```
///
/// Everything the pack names is resolved against **the `.ssab`'s own directory**,
/// which is what the exporter wrote those paths relative to. Atlases reach the
/// GPU on the next `ss_scene_update`, the same moment an embedded pack's do.
///
/// A host whose assets are not files — a pack file, an archive, a platform asset
/// manager — keeps using `ss_player_load_ssab` plus `ss_scene_add_texture_png`,
/// which is why those remain.
///
/// # Safety
/// `path` must be a NUL-terminated UTF-8 string. It is not retained beyond the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_load(handle: *mut SsPlayer, path: *const c_char) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    let Some(path) = (unsafe { c_str(path) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "path is null or not UTF-8") };
    match player.load(path) {
        Ok(()) => SS_OK,
        Err(e) => fail(e.as_code(), format!("{path}: {e}")),
    }
}

/// Register an `.ssab` pack under `pack_name` without changing what plays.
///
/// Every pack an animation can reference must be registered before
/// `ss_player_set_animation`, since external instance parts resolve by pack
/// name. Embedded-mode packs also carry their atlases; those
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
/// runtime reports in `DrawBatch.texture_hash`. `rgba` is
/// straight-alpha RGBA8, `width * height * 4` bytes; it is premultiplied on the
/// way to the GPU.
///
/// Registered on the **scene**, because the texture map is device-scoped: every
/// player in the window sees it, and a pack shown five times is uploaded once.
/// Embedded-mode packs need no call — their atlases load with the
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
/// It carries a PNG decoder for embedded packs anyway, so a C host
/// showing a reference-mode `.ssab` does not have to bring ImageIO / WIC /
/// libpng along just to hand the pixels over. Reading the file is still the
/// host's job — nothing here opens one.
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

/// How many atlases a registered pack asks for. `pack_name` null
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
/// knows *what* to load and *where the exporter put it*.
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

/// How many things the player's last load could not get.
///
/// `ss_player_load` reports an error only when the `.ssab` itself is unreadable —
/// one missing atlas must not cost you the character — so this is how a caller
/// tells a complete load from one that played anyway:
///
/// ```c
/// if (ss_player_load(player, path) != SS_OK) {
/// // the .ssab itself
/// } else if (ss_player_missing_count(player) > 0) {
/// // it plays, but something is a silhouette or absent
/// }
/// ```
///
/// (Line comments above, deliberately: cbindgen renders these docs into a C
/// block comment, so a nested block-comment terminator would close it early
/// and break the generated header for every C and C++ host.)
///
/// Describes the most recent load, not the player's history. Zero for a null or
/// destroyed handle, which is indistinguishable from success — check the load's
/// own return value for that.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_missing_count(handle: *mut SsPlayer) -> u32 {
    unsafe { player(handle) }.map(|p| p.missing_assets().len() as u32).unwrap_or(0)
}

/// Detail of one entry in the list `ss_player_missing_count` sizes.
///
/// Writes `out` and returns `SS_OK`, or leaves it untouched and reports why.
///
/// # Safety
/// `out` must point at a writable `ss_missing_asset_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_missing_at(handle: *mut SsPlayer, index: u32, out: *mut SsMissingAsset) -> i32 {
    if out.is_null() {
        return fail(SS_ERROR_INVALID_ARGUMENT, "out is null");
    }
    let Some((scene, id)) = (unsafe { scene_of(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };

    // Copied out before the scratch is touched: the borrow of the player ends
    // here, and `scene` is needed mutably to own the string handed back.
    let Some(missing) = scene.host.player_mut(id).and_then(|p| p.missing_assets().get(index as usize)).cloned() else {
        return fail(SS_ERROR_INVALID_ARGUMENT, format!("nothing missing at index {index}"));
    };
    let kind = match missing.kind {
        MissingKind::Texture => SS_MISSING_TEXTURE,
        MissingKind::Pack => SS_MISSING_PACK,
    };
    let Ok(name) = CString::new(missing.name) else {
        return fail(SS_ERROR_INVALID_ARGUMENT, "name contains an interior NUL");
    };
    scene.missing_scratch = name;
    unsafe { out.write(SsMissingAsset { kind, hash: missing.hash, name: scene.missing_scratch.as_ptr() }) };
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

/// Start the selected animation from the head of its section, re-arming the loop
/// count. Not a resume: it rewinds, so lifting a `ss_player_pause` is
/// `ss_player_resume`'s job.
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

/// Hold playback on the current frame. Idempotent, and ignored while nothing is
/// playing. Lift it with `ss_player_resume`.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_pause(handle: *mut SsPlayer) {
    if let Some(player) = unsafe { player(handle) } {
        player.pause();
    }
}

/// Lift a `ss_player_pause`, carrying on from the same frame. Idempotent, and a
/// no-op on a stopped animation — starting one is `ss_player_play`'s job.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_resume(handle: *mut SsPlayer) {
    if let Some(player) = unsafe { player(handle) } {
        player.resume();
    }
}

/// Stop playback. The playhead stays where it stands; nothing rewinds.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_stop(handle: *mut SsPlayer) {
    if let Some(player) = unsafe { player(handle) } {
        player.stop();
    }
}

/// Scale playback speed; 1.0 is the authored speed.
///
/// Zero and negative values are **ignored** (the runtime keeps 1.0). To play
/// backwards use `ss_player_set_playback_direction`, which is the separate
/// setting that controls direction.
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

/// Whether playback is held in place. `ss_player_is_playing` stays true.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_is_paused(handle: *mut SsPlayer) -> bool {
    unsafe { player(handle) }.map(|p| p.is_paused()).unwrap_or(false)
}

/// Whether every configured loop has played. Sticky until the next play or
/// `ss_player_set_animation`, and never true under an infinite loop.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_is_finished(handle: *mut SsPlayer) -> bool {
    unsafe { player(handle) }.map(|p| p.is_finished()).unwrap_or(false)
}

/// Whether the last `ss_scene_update` crossed a loop boundary. An edge: true
/// only inside the update that crossed.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_is_looped(handle: *mut SsPlayer) -> bool {
    unsafe { player(handle) }.map(|p| p.is_looped()).unwrap_or(false)
}

/// Jump to `frame`. Fractional values are honoured.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_seek(handle: *mut SsPlayer, frame: f32) {
    if let Some(player) = unsafe { player(handle) } {
        player.seek(frame);
    }
}

/// Current playhead position, fractional. 0 when nothing is loaded.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_frame_no(handle: *mut SsPlayer) -> f32 {
    unsafe { player(handle) }.map(|p| p.frame_no()).unwrap_or(0.0)
}

/// First frame of the range being played; follows
/// `ss_player_set_animation_section`.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_start_frame(handle: *mut SsPlayer) -> i32 {
    unsafe { player(handle) }.map(|p| p.start_frame()).unwrap_or(0)
}

/// Last frame of that range.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_end_frame(handle: *mut SsPlayer) -> i32 {
    unsafe { player(handle) }.map(|p| p.end_frame()).unwrap_or(0)
}

/// Length of that range, both ends inclusive.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_total_frames(handle: *mut SsPlayer) -> i32 {
    unsafe { player(handle) }.map(|p| p.total_frames()).unwrap_or(0)
}

/// `count` cycles then stop; `-1` (or `0`) loops forever. Under ping-pong one
/// cycle is a full round trip.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_loop_count(handle: *mut SsPlayer, count: i32) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_loop_count(count);
    }
}

/// The configured loop count.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_loop_count(handle: *mut SsPlayer) -> i32 {
    unsafe { player(handle) }.map(|p| p.loop_count()).unwrap_or(0)
}

/// How many cycles are left of it.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_remaining_loops(handle: *mut SsPlayer) -> i32 {
    unsafe { player(handle) }.map(|p| p.remaining_loops()).unwrap_or(0)
}

/// Override the advance rate in fps. Prefer `ss_player_set_speed`, which scales
/// playback without replacing the authored rate.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_frame_rate(handle: *mut SsPlayer, fps: i32) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_frame_rate(fps);
    }
}

/// The rate the animation advances at. 0 when nothing is loaded.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_frame_rate(handle: *mut SsPlayer) -> i32 {
    unsafe { player(handle) }.map(|p| p.frame_rate()).unwrap_or(0)
}

/// Restrict playback to `start..=end` of the current animation, so looping and
/// ping-pong turn around inside that range.
///
/// Call it **after** `ss_player_set_animation`: frame numbers only mean
/// something inside the animation they were measured in, and setting an
/// animation rebuilds the tree and takes the section with it.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_animation_section(handle: *mut SsPlayer, start: i32, end: i32) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_animation_section(start, end);
    }
}

/// `direction`: 0 forward, 1 backward. `style`: 0 normal, 1 ping-pong. Set as a
/// pair — pass the current value for whichever you are not changing.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_playback_direction(handle: *mut SsPlayer, direction: i32, style: i32) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_playback_direction(PlaybackDirection::from_wire(direction), PlaybackStyle::from_wire(style));
    }
}

/// 0 forward, 1 backward.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_playback_direction(handle: *mut SsPlayer) -> i32 {
    unsafe { player(handle) }.map(|p| p.playback_direction().as_wire()).unwrap_or(0)
}

/// 0 normal, 1 ping-pong.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_playback_style(handle: *mut SsPlayer) -> i32 {
    unsafe { player(handle) }.map(|p| p.playback_style().as_wire()).unwrap_or(0)
}

/// True (the default) draws between authored frames; false floors the frame that
/// is drawn, pinning playback to whole ones.
///
/// The playhead keeps its fraction either way — `ss_player_frame_no` reports it
/// unrounded — and so does the stepping of instance and effect children. This
/// affects only what is drawn.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_sub_frame(handle: *mut SsPlayer, enabled: bool) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_sub_frame(enabled);
    }
}

/// See `ss_player_set_sub_frame`.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_sub_frame(handle: *mut SsPlayer) -> bool {
    unsafe { player(handle) }.map(|p| p.sub_frame()).unwrap_or(true)
}

/// True (the default) keeps playback on the wall clock by dropping frames under
/// load; false advances exactly one frame per `ss_scene_update`.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_skip_frames(handle: *mut SsPlayer, skip: bool) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_skip_frames(skip);
    }
}

/// See `ss_player_set_skip_frames`.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_skip_frames(handle: *mut SsPlayer) -> bool {
    unsafe { player(handle) }.map(|p| p.skip_frames()).unwrap_or(true)
}

// --- Per-part overrides ---
//
// Indices address the root animation and are stable within one pack, so resolve
// a name once with `ss_player_find_part_index` and keep the index. Overrides do
// not reach inside an instance part — its child runs as its own context. Call
// them only after `ss_player_set_animation`, which sizes the runtime's override
// table and clears everything but a permanent override.

/// Resolve a part name to its index in the current animation, or -1 when the
/// name is unknown, `name` is not valid UTF-8, or no animation is set up.
///
/// Unlike the calls below this returns an index rather than a status, so it does
/// not touch `ss_last_error`.
///
/// # Safety
/// `handle` must be a live handle or null; `name` must be a valid
/// null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_find_part_index(handle: *mut SsPlayer, name: *const c_char) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return -1 };
    let Some(name) = (unsafe { c_str(name) }) else { return -1 };
    player.find_part_index(name).map(|i| i as i32).unwrap_or(-1)
}

/// Force-hide a part (`hide` true) or revert it to the animation. `cascade`
/// propagates a force-hide to the part's descendants — a keyframe hide never
/// cascades, so this is the only way to hide a sub-tree.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_part_visibility_override(handle: *mut SsPlayer, part: u32, hide: bool, cascade: bool) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    match player.set_part_visibility_override(part, hide, cascade) {
        Ok(()) => SS_OK,
        Err(e) => fail(e.as_code(), format!("part {part}: {e}")),
    }
}

/// Revert a part to the animation's own visibility.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_clear_part_visibility_override(handle: *mut SsPlayer, part: u32) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    match player.clear_part_visibility_override(part) {
        Ok(()) => SS_OK,
        Err(e) => fail(e.as_code(), format!("part {part}: {e}")),
    }
}

/// Override a part's colour. `blend_op`: 0 mix, 1 mul, 2 add, 3 sub. `rgba` is
/// packed `0xRRGGBBAA` in the authored 8-bit sRGB space — do not pre-multiply
/// it. `priority`: 0 until the next keyframe, 1 until the next animation, 2
/// permanent. Normal parts only; other kinds ignore it and still return `SS_OK`.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_part_color_override(handle: *mut SsPlayer, part: u32, blend_op: u8, rgba: u32, priority: u8) -> i32 {
    let Some(blend_op) = BlendOp::from_wire(blend_op) else { return fail(SS_ERROR_INVALID_ARGUMENT, "blend_op must be 0..=3") };
    let Some(priority) = OverridePriority::from_wire(priority) else { return fail(SS_ERROR_INVALID_ARGUMENT, "priority must be 0..=2") };
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    match player.set_part_color_override(part, blend_op, rgba, priority) {
        Ok(()) => SS_OK,
        Err(e) => fail(e.as_code(), format!("part {part}: {e}")),
    }
}

/// Four-corner colour override, for a gradient. `corners` points at 16 bytes in
/// the runtime's order — left-top, right-top, left-bottom, right-bottom, each
/// `[r, g, b, a]`. Otherwise as `ss_player_set_part_color_override`.
///
/// # Safety
/// `handle` must be a live handle or null; `corners` must point at 16 readable
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_part_color_override_corners(handle: *mut SsPlayer, part: u32, blend_op: u8, corners: *const u8, priority: u8) -> i32 {
    let Some(blend_op) = BlendOp::from_wire(blend_op) else { return fail(SS_ERROR_INVALID_ARGUMENT, "blend_op must be 0..=3") };
    let Some(priority) = OverridePriority::from_wire(priority) else { return fail(SS_ERROR_INVALID_ARGUMENT, "priority must be 0..=2") };
    if corners.is_null() {
        return fail(SS_ERROR_INVALID_ARGUMENT, "corners is null");
    }
    let mut rgba = [[0u8; 4]; 4];
    let bytes = unsafe { std::slice::from_raw_parts(corners, 16) };
    for (corner, chunk) in rgba.iter_mut().zip(bytes.chunks_exact(4)) {
        corner.copy_from_slice(chunk);
    }
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    match player.set_part_color_override_corners(part, blend_op, rgba, priority) {
        Ok(()) => SS_OK,
        Err(e) => fail(e.as_code(), format!("part {part}: {e}")),
    }
}

/// Revert a part to the animation's own colour.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_clear_part_color_override(handle: *mut SsPlayer, part: u32) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    match player.clear_part_color_override(part) {
        Ok(()) => SS_OK,
        Err(e) => fail(e.as_code(), format!("part {part}: {e}")),
    }
}

/// Make a part draw a different cell. `cellmap_name` is written without the
/// `.ssce` extension. Normal and Mask parts only; fails when the cell cannot be
/// resolved against the loaded pack.
///
/// # Safety
/// `handle` must be a live handle or null; both names must be valid
/// null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_part_cell_override(handle: *mut SsPlayer, part: u32, cellmap_name: *const c_char, cell_name: *const c_char, priority: u8) -> i32 {
    let Some(priority) = OverridePriority::from_wire(priority) else { return fail(SS_ERROR_INVALID_ARGUMENT, "priority must be 0..=2") };
    let Some(cellmap_name) = (unsafe { c_str(cellmap_name) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "cellmap_name is null or not UTF-8") };
    let Some(cell_name) = (unsafe { c_str(cell_name) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "cell_name is null or not UTF-8") };
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    match player.set_part_cell_override(part, cellmap_name, cell_name, priority) {
        Ok(()) => SS_OK,
        Err(e) => fail(e.as_code(), format!("part {part} -> {cellmap_name}/{cell_name}: {e}")),
    }
}

/// Revert a part to the animation's own cell.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_clear_part_cell_override(handle: *mut SsPlayer, part: u32) -> i32 {
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    match player.clear_part_cell_override(part) {
        Ok(()) => SS_OK,
        Err(e) => fail(e.as_code(), format!("part {part}: {e}")),
    }
}

/// Drop every override on the root animation at once.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_clear_all_part_overrides(handle: *mut SsPlayer) {
    if let Some(player) = unsafe { player(handle) } {
        player.clear_all_part_overrides();
    }
}

/// How many parts the root animation has; 0 before one is set up.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_part_count(handle: *mut SsPlayer) -> u32 {
    unsafe { player(handle) }.map(|p| p.part_count() as u32).unwrap_or(0)
}

// --- Events ---
//
// Polled after `ss_scene_update`, which is what advanced the clock. Everything
// below describes that tick and is replaced by the next one, so a host that
// wants to keep a payload copies it before updating again.
//
// The strings are borrowed from the player and stay valid until the next
// `ss_scene_update` on it — the same rule `ss_player_texture_at` follows.

/// Whether the last `ss_scene_update` crossed a loop boundary. One report per
/// update however many cycles it crossed.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_looped_last_update(handle: *mut SsPlayer) -> bool {
    unsafe { player(handle) }.map(|p| p.looped_last_update()).unwrap_or(false)
}

/// Whether the last `ss_scene_update` **completed** playback: true for exactly
/// one update, unlike the sticky `ss_player_is_finished`. Never true under an
/// infinite loop, and not raised by `ss_player_stop`.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_finished_last_update(handle: *mut SsPlayer) -> bool {
    unsafe { player(handle) }.map(|p| p.finished_last_update()).unwrap_or(false)
}

/// How many timeline events the last `ss_scene_update` crossed.
///
/// **Frames skipped over are included**: a long tick reports every frame it
/// stepped through, not just the one it landed on.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_event_count(handle: *mut SsPlayer) -> u32 {
    unsafe { player(handle) }.map(|p| p.events().len() as u32).unwrap_or(0)
}

/// Fill `out` with the `index`-th event of the last update.
///
/// Read `kind` first: it says which of the payload fields carry anything.
///
/// Returns `SS_OK`, or `SS_ERROR_INVALID_ARGUMENT` when the handle or `out` is
/// null or `index` is out of range, leaving `out` untouched.
///
/// # Safety
/// `handle` must be a live handle or null; `out` must point at a writable
/// `ss_event_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_event_at(handle: *mut SsPlayer, index: u32, out: *mut SsEventData) -> i32 {
    if out.is_null() {
        return fail(SS_ERROR_INVALID_ARGUMENT, "out is null");
    }
    let Some((scene, id)) = (unsafe { scene_of(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    // Split into the two fields this needs rather than borrowing the scene whole:
    // the event is read straight out of the player and the string is copied into
    // the scratch beside it, so nothing is cloned to end a borrow. Cloning the
    // event to get there duplicated every string it carried — and a signal's
    // whole parameter list — once per event per frame.
    let SsScene { host, event_text_scratch, .. } = scene;
    let Some(event) = host.player_mut(id).and_then(|p| p.events().get(index as usize)) else {
        return fail(SS_ERROR_INVALID_ARGUMENT, format!("event {index} is out of range"));
    };

    let mut data = SsEventData {
        kind: 0,
        part_index: event.part_index(),
        has_integer: false,
        has_rect: false,
        has_point: false,
        has_text: false,
        integer: 0,
        rect: [0.0; 4],
        point: [0.0; 2],
        text: std::ptr::null(),
        param_count: 0,
        sound_list_name_hash: 0,
        sound_name_hash: 0,
        loop_num: 0,
    };
    let text: &str = match event {
        SsEvent::UserData(u) => {
            data.kind = SS_EVENT_USER_DATA;
            data.has_integer = u.integer.is_some();
            data.has_rect = u.rect.is_some();
            data.has_point = u.point.is_some();
            data.has_text = u.text.is_some();
            data.integer = u.integer.unwrap_or(0);
            data.rect = u.rect.unwrap_or([0.0; 4]);
            data.point = u.point.unwrap_or([0.0; 2]);
            u.text.as_deref().unwrap_or_default()
        }
        SsEvent::Signal(sig) => {
            data.kind = SS_EVENT_SIGNAL;
            data.param_count = sig.params.len() as u32;
            &sig.command_id
        }
        SsEvent::Audio(a) => {
            data.kind = SS_EVENT_AUDIO;
            data.sound_list_name_hash = a.sound_list_name_hash;
            data.sound_name_hash = a.sound_name_hash;
            data.loop_num = a.loop_num;
            &a.sound_name
        }
    };

    data.text = event_text_scratch.set(text);
    unsafe { out.write(data) };
    SS_OK
}

/// How many parameters the `index`-th event carries, or 0 when it is not a
/// signal.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_event_signal_param_count(handle: *mut SsPlayer, index: u32) -> u32 {
    let Some(player) = (unsafe { player(handle) }) else { return 0 };
    match player.events().get(index as usize) {
        Some(SsEvent::Signal(s)) => s.params.len() as u32,
        _ => 0,
    }
}

/// Fill `out` with parameter `param` of the `index`-th event, which must be a
/// signal.
///
/// Returns `SS_OK`, or `SS_ERROR_INVALID_ARGUMENT` when either index is out of
/// range or the event is not a signal, leaving `out` untouched.
///
/// # Safety
/// `handle` must be a live handle or null; `out` must point at a writable
/// `ss_signal_param_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_event_signal_param_at(handle: *mut SsPlayer, index: u32, param: u32, out: *mut SsSignalParamData) -> i32 {
    if out.is_null() {
        return fail(SS_ERROR_INVALID_ARGUMENT, "out is null");
    }
    let Some((scene, id)) = (unsafe { scene_of(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    // Two scratch slots and the host, borrowed apart — see `ss_player_event_at`.
    let SsScene { host, param_id_scratch, param_text_scratch, .. } = scene;
    let found = host.player_mut(id).and_then(|p| match p.events().get(index as usize) {
        Some(SsEvent::Signal(s)) => s.params.get(param as usize),
        _ => None,
    });
    let Some(p) = found else {
        return fail(SS_ERROR_INVALID_ARGUMENT, format!("event {index} parameter {param} is out of range, or the event is not a signal"));
    };

    let mut data = SsSignalParamData { id: std::ptr::null(), value_type: SS_SIGNAL_VALUE_NONE, bool_value: false, int_value: 0, float_value: 0.0, text: std::ptr::null() };
    let mut text = "";
    match &p.value {
        SignalValue::Bool(b) => {
            data.value_type = SS_SIGNAL_VALUE_BOOL;
            data.bool_value = *b;
        }
        SignalValue::Index(i) => {
            data.value_type = SS_SIGNAL_VALUE_INDEX;
            data.int_value = *i;
        }
        SignalValue::Integer(i) => {
            data.value_type = SS_SIGNAL_VALUE_INTEGER;
            data.int_value = *i;
        }
        SignalValue::Floating(f) => {
            data.value_type = SS_SIGNAL_VALUE_FLOATING;
            data.float_value = *f;
        }
        SignalValue::Text(t) => {
            data.value_type = SS_SIGNAL_VALUE_TEXT;
            text = t;
        }
        SignalValue::None => {}
    }

    data.id = param_id_scratch.set(&p.id);
    data.text = param_text_scratch.set(text);
    unsafe { out.write(data) };
    SS_OK
}

/// Write the part's current-frame transform into `out` as the 2D affine
/// `[m00, m01, m10, m11, tx, ty]`, in animation-local (Y-down) space with
/// neither `ss_player_set_transform` nor the scene's projection applied. A point
/// maps as `x' = m00*x + m01*y + tx`, `y' = m10*x + m11*y + ty`.
///
/// Returns `SS_OK`, or `SS_ERROR_INVALID_ARGUMENT` when the handle or `out` is
/// null, no animation is set up, or `part` is out of range — `out` is untouched
/// in that case.
///
/// Call it after `ss_scene_update`: it describes the frame that produced. The
/// matrix exists for every part whether or not it is hidden, so a hidden part
/// still reports where it would have been.
///
/// # Safety
/// `handle` must be a live handle or null; `out` must point at 6 writable
/// `float`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_part_transform(handle: *mut SsPlayer, part: u32, out: *mut f32) -> i32 {
    if out.is_null() {
        return fail(SS_ERROR_INVALID_ARGUMENT, "out is null");
    }
    let Some(player) = (unsafe { player(handle) }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "player handle is null or destroyed") };
    let Some(m) = player.part_transform(part) else {
        return fail(SS_ERROR_INVALID_ARGUMENT, format!("part {part} is out of range, or no animation is set up"));
    };
    unsafe { std::ptr::copy_nonoverlapping(m.as_ptr(), out, 6) };
    SS_OK
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

// --- placement ------------------------------------------------

/// Place this player in its scene: translation in pixels from the centre, scale,
/// then rotation in radians.
///
/// Without this every player sits at the origin, on top of the others.
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

/// Player-level tint, `0xRRGGBB`, multiplied into every part after the PartColor
/// blend. `0xFFFFFF` — the default — leaves colours untouched. Mask coverage is
/// unaffected, as with `ss_player_set_alpha`.
///
/// # Safety
/// `handle` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_player_set_tint(handle: *mut SsPlayer, rgb: u32) {
    if let Some(player) = unsafe { player(handle) } {
        player.set_tint(rgb);
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
/// A lost/outdated surface is absorbed by reconfiguring: the call
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

/// What the last frame cost. Writes `out` and returns
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

/// Whether the driver has taken the GPU device away. False for a null
/// scene. While this is true `ss_scene_update` / `ss_scene_render` do nothing and
/// `ss_scene_resize` only records the size.
///
/// # Safety
/// `scene` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_is_device_lost(scene: *mut SsScene) -> bool {
    unsafe { scene.as_ref() }.is_some_and(|s| s.host.is_device_lost())
}

/// Rebuild the GPU stack after a device loss and restore every player drawing into
/// this window, resuming playback where it was interrupted.
///
/// `SS_ERROR_GPU` means the window has to be rebuilt from `ss_scene_create`.
///
/// # Safety
/// `scene` must be a live handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ss_scene_restore_device(scene: *mut SsScene) -> i32 {
    let Some(scene) = (unsafe { scene.as_mut() }) else { return fail(SS_ERROR_INVALID_ARGUMENT, "scene is null") };
    match scene.host.restore_device() {
        Ok(()) => SS_OK,
        Err(msg) => fail(SS_ERROR_GPU, msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The string slots hand C a pointer rather than a copy, so what is behind it
    /// has to be NUL-terminated and has to stay put until the next call on that
    /// slot. Neither half is visible from the C side until something reads past
    /// the end, and the reuse is what keeps `ss_player_event_at` from allocating
    /// once per event per frame — a `CString` field cannot be refilled, only
    /// replaced.
    ///
    /// Everything else in this module needs a window and a device, so this is the
    /// one thing here a test can reach.
    #[test]
    fn a_refilled_slot_stays_a_valid_c_string() {
        let mut slot = CScratch::default();

        assert_eq!(unsafe { CStr::from_ptr(slot.set("OnFootstep")) }.to_str().unwrap(), "OnFootstep");
        assert_eq!(unsafe { CStr::from_ptr(slot.set("")) }.to_bytes(), b"");

        // An interior NUL comes back empty, which is what the
        // `CString::new(..).unwrap_or_default()` this replaced did: the tail past
        // a NUL cannot reach C, and handing over the head alone would be a
        // different string wearing the right name.
        assert_eq!(unsafe { CStr::from_ptr(slot.set("bad\0tail")) }.to_bytes(), b"");

        // A shorter refill reuses the buffer instead of allocating a new one,
        // which is the whole reason this type exists rather than a `CString`.
        let long = slot.set("a-command-id-considerably-longer-than-the-next-one");
        let short = slot.set("x");
        assert_eq!(long.addr(), short.addr(), "the slot was reallocated instead of refilled");
        assert_eq!(unsafe { CStr::from_ptr(short) }.to_str().unwrap(), "x");
    }
}
