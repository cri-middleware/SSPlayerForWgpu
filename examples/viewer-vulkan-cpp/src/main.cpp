// Vulkan (Linux) verification viewer for the ssplayer-wgpu C API.
//
// This is the "③ ffi" face end to end: the process owns the window — an X11 one
// through Xlib, or a Wayland one through xdg-shell — hands its raw handle to
// `ss_scene_create`, and drives the scene with `ss_scene_update` /
// `ss_scene_render`. No Rust in sight. The scene's own host (host.rs) builds the
// wgpu stack behind the ABI, with backend=Vulkan forced so this really exercises
// Vulkan rather than whatever wgpu would pick.
//
// **Both window systems live in this one binary on purpose.** `SS_WINDOW_XLIB`
// and `SS_WINDOW_WAYLAND` are separate arms of `surface_target` in ffi.rs, and
// running the Xlib one under a Wayland session goes through Xwayland — which
// never touches the Wayland arm. So there is no single window that covers both:
// the viewer picks by `WAYLAND_DISPLAY`, `--x11` / `--wayland` override it, and
// the status block prints which arm is live.
//
// It is the Linux twin of examples/viewer-dx12-cpp and examples/viewer-metal-cpp,
// and the three deliberately duplicate the playback loop instead of sharing it:
// the point of the sample is that one directory is self-contained. What actually
// differs is window creation — the C-API calls below are the same calls in the
// same order as the other two.
//
// Dropping a .ssab on the window is the one thing the other two do that this one
// does not: on X11 that is the XDnD handshake by hand, on Wayland it is
// wl_data_device, and neither says anything about the C API. Pass a directory as
// the first argument instead.
//
//   ssviewer-vulkan [--x11|--wayland] [asset-dir] [pack-name]
//
//   Space  pause / resume        Left/Right  previous / next .ssab
//   R      restart animation     Up/Down     previous / next animation
//   +/-    add / remove player   Escape      quit

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <filesystem>
#include <string>
#include <vector>

#include "ssplayer_wgpu.h"

#if SS_HAVE_WAYLAND
#include <cerrno>
#include <linux/input-event-codes.h>
#include <poll.h>
#include <unistd.h>
#include <wayland-client.h>

#include "xdg-shell-client-protocol.h"
#endif

// Xlib last: it defines `None`, `Bool` and `Status` as macros, which nothing
// below uses but which would be visible to any header included after it.
#if SS_HAVE_X11
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/keysym.h>
#endif

/// Where scripts/deploy-examples.sh puts the demo packs. Baked in by CMake so
/// the viewer runs from any working directory; the first argument overrides it.
#ifndef SS_DEFAULT_ASSET_DIR
#define SS_DEFAULT_ASSET_DIR "assets/overall"
#endif

/// How many characters to show at startup. Two is the point of the exercise.
static const size_t kInitialPlayers = 2;
/// Most players the +/- keys will go to.
static const size_t kMaxPlayers = 6;
/// Stand-in canvas for a player that has not been given an animation yet, so the
/// first lay-out has something to divide by. Same value the Rust viewer uses.
static const float kFallbackCanvas = 320.0f;
/// Window size asked for at startup, in pixels. Both window systems may be given
/// something else by the compositor, and both report what they got.
static const uint32_t kDefaultWidth = 960;
static const uint32_t kDefaultHeight = 540;

// --- assets ---------------------------------------------------------------
//
// Paths are UTF-8 `std::string` throughout: that is what the C API takes
// (`ffi.rs` decodes with `CStr::to_str`) and what the file system hands back
// here, so unlike the Win32 viewer there is no conversion at the boundary.

static bool IsDirectory(const std::string &path) {
    std::error_code error;
    return std::filesystem::is_directory(path, error);
}

static std::string FileName(const std::string &path) {
    return std::filesystem::path(path).filename().string();
}

/// Every `.ssab` directly inside `dir`, sorted, so a directory plays in the
/// order the Left/Right keys will walk it.
static std::vector<std::string> SsabFilesIn(const std::string &dir) {
    std::vector<std::string> found;
    std::error_code error;
    for (const std::filesystem::directory_entry &entry : std::filesystem::directory_iterator(dir, error)) {
        if (entry.is_regular_file(error) && entry.path().extension() == ".ssab") {
            found.push_back(entry.path().string());
        }
    }
    std::sort(found.begin(), found.end());
    return found;
}

/// The first argument, else the directory CMake baked in, else assets/overall
/// walking up from the working directory.
static std::string FindAssetDir(const std::string &requested) {
    std::vector<std::string> candidates;
    if (!requested.empty()) {
        candidates.push_back(requested);
    }
    candidates.push_back(SS_DEFAULT_ASSET_DIR);
    candidates.push_back("assets/overall");
    candidates.push_back("../assets/overall");
    candidates.push_back("../../assets/overall");

    for (size_t i = 0; i < candidates.size(); i++) {
        if (IsDirectory(candidates[i])) {
            return candidates[i];
        }
    }
    return candidates[0];
}

// --- viewer ---------------------------------------------------------------

/// What the two window backends translate their key events into, so everything
/// below this line is the same code on X11 and on Wayland.
enum class Key { Unhandled, Quit, Pause, PrevPack, NextPack, PrevAnim, NextAnim, Restart, AddPlayer, RemovePlayer };

class Viewer {
  public:
    Viewer(const std::string &assetDir, const std::string &initialPack, const char *windowSystem)
        : assetDir_(assetDir), initialPack_(initialPack), windowSystem_(windowSystem) {}

    bool Start(const ss_window_handle_t &window, uint32_t width, uint32_t height);
    void Shutdown();

    void Tick();
    void Resize(uint32_t width, uint32_t height);
    void HandleKey(Key key);

  private:
    ss_player_t *PlayerAt(size_t index) const { return players_[index]; }
    void AddPlayer();
    void RemovePlayer();
    void LayOutPlayers();

    bool CollectPacks();
    void LoadPackAtIndex(size_t index);
    void PlayAnimationAtIndex(size_t index);
    void PrintStatus();

    ss_scene_t *scene_ = nullptr;
    /// The scene's size in physical pixels, which is also what placement is
    /// measured in — the projection maps one unit to one target pixel. Kept
    /// here because the lay-out divides the window between the cast.
    uint32_t width_ = 0;
    uint32_t height_ = 0;
    /// One handle per character. All of them draw into `scene_`.
    std::vector<ss_player_t *> players_;

    std::string assetDir_;
    std::string initialPack_; // pack to show first, or empty for Basic
    const char *windowSystem_;

    std::vector<std::string> packs_; // .ssab paths, in the order Left/Right walks them
    size_t packIndex_ = 0;
    std::vector<std::string> anims_; // animation names of the current pack, UTF-8 as the API gave them
    size_t animIndex_ = 0;

    bool paused_ = false;
    double lastTick_ = 0.0;
};

/// Seconds on a clock that cannot jump backwards, which is all the frame delta
/// needs.
static double Now() {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1.0e9;
}

// --- setup ---------------------------------------------------------------

bool Viewer::Start(const ss_window_handle_t &window, uint32_t width, uint32_t height) {
    ss_scene_desc_t desc;
    memset(&desc, 0, sizeof(desc));
    desc.window = window;
    desc.backend = SS_BACKEND_VULKAN;
    desc.width = width;
    desc.height = height;
    // The first `AddPlayer` lays the cast out, so the size has to be known by
    // then rather than only from the first resize event.
    width_ = width;
    height_ = height;
    desc.clear_color[0] = 0.08f;
    desc.clear_color[1] = 0.08f;
    desc.clear_color[2] = 0.12f;
    desc.clear_color[3] = 1.0f;
    desc.enable_mask = true; // stencil buffer, so Mask.ssab renders
    desc.y_down = false;

    scene_ = ss_scene_create(&desc);
    if (scene_ == nullptr) {
        fprintf(stderr, "ss_scene_create failed: %s\n", ss_last_error());
        return false;
    }

    for (size_t i = 0; i < kInitialPlayers; i++) {
        AddPlayer();
    }
    if (players_.empty()) {
        return false; // AddPlayer already reported why
    }
    if (!CollectPacks()) {
        return false;
    }
    LoadPackAtIndex(packIndex_);

    lastTick_ = Now();
    return true;
}

void Viewer::Shutdown() {
    // Destroying the scene takes its players with it; the handles must not be
    // touched afterwards. It has to happen before the window goes away, since
    // the surface was built from that window's handle.
    if (scene_ != nullptr) {
        ss_scene_destroy(scene_);
        scene_ = nullptr;
    }
    players_.clear();
}

// --- players -------------------------------------------------------------

/// Add one character to the scene and lay the row out again.
void Viewer::AddPlayer() {
    ss_player_t *player = ss_scene_create_player(scene_);
    if (player == nullptr) {
        fprintf(stderr, "ss_scene_create_player: %s\n", ss_last_error());
        return;
    }
    players_.push_back(player);
    // Nothing to register: the caller follows with `LoadPackAtIndex`, and that
    // one call brings the pack, its atlases and the packs its instance parts
    // play from.
    LayOutPlayers();
}

void Viewer::RemovePlayer() {
    if (players_.size() <= 1) {
        return; // keep at least one, or there is nothing to look at
    }
    ss_player_destroy(players_.back());
    players_.pop_back();
    LayOutPlayers();
}

/// Give each character an equal slice of the window, fit it to that slice, and
/// stack the cast front to back.
///
/// Placement is per player; the projection belongs to the scene, and it maps one
/// unit to one target pixel, so the window's own size is the right thing to
/// divide. Fitting rather than a fixed scale is what the demo packs need: they
/// disagree about canvas size, so one constant either crowds two characters into
/// each other or runs a tall pack off the top. This is the Rust viewer's Row
/// layout, so the two faces can be read side by side.
void Viewer::LayOutPlayers() {
    size_t count = players_.size();
    if (count == 0 || width_ == 0 || height_ == 0) {
        return; // nothing to place, or no window to place it in
    }
    float slot = (float)width_ / (float)count;
    float last = (float)(count - 1);
    for (size_t i = 0; i < count; i++) {
        // Each pack names its own canvas. Before a pack is loaded there is none,
        // and the stand-in keeps the first lay-out finite.
        float cw = 0.0f, ch = 0.0f;
        if (ss_player_canvas_size(players_[i], &cw, &ch) != SS_OK) {
            cw = ch = kFallbackCanvas;
        }
        cw = cw > 1.0f ? cw : 1.0f;
        ch = ch > 1.0f ? ch : 1.0f;

        // The tighter of the two axes wins, so a tall pack stays on screen.
        float scale = slot * 0.9f / cw;
        float fitHeight = (float)height_ * 0.9f / ch;
        if (fitHeight < scale) {
            scale = fitHeight;
        }
        scale = scale < 0.05f ? 0.05f : (scale > 1.0f ? 1.0f : scale);

        float offset = (float)i - last * 0.5f;
        ss_player_set_transform(players_[i], offset * slot, 0.0f, scale, scale, 0.0f);
        ss_player_set_order(players_[i], (int32_t)i);
    }
}

// --- assets ---------------------------------------------------------------

/// Gather every .ssab in the asset directory and pick the one to open on.
bool Viewer::CollectPacks() {
    packs_ = SsabFilesIn(assetDir_);
    if (packs_.empty()) {
        fprintf(stderr, "no .ssab files in %s\n  run scripts/deploy-examples.sh, or pass a directory as the first argument\n", assetDir_.c_str());
        return false;
    }

    // Show the pack named on the command line, else Basic.ssab like the Rust viewer.
    std::string wanted = (initialPack_.empty() ? std::string("Basic") : initialPack_) + ".ssab";
    packIndex_ = 0;
    for (size_t i = 0; i < packs_.size(); i++) {
        if (FileName(packs_[i]) == wanted) {
            packIndex_ = i;
            break;
        }
    }
    return true;
}

/// Play one of the listed packs on every player.
///
/// `ss_player_load` is the whole of it: the pack, its atlases in either export
/// mode, and the sibling .ssab files its instance parts play from, all resolved
/// against the directory the file was read from. A host whose assets are not
/// files walks `ss_player_texture_at` and feeds `ss_scene_add_texture_png`
/// instead.
void Viewer::LoadPackAtIndex(size_t index) {
    const std::string &path = packs_[index];
    for (size_t i = 0; i < players_.size(); i++) {
        if (ss_player_load(players_[i], path.c_str()) != SS_OK) {
            fprintf(stderr, "load %s: %s\n", path.c_str(), ss_last_error());
            return;
        }
    }

    anims_.clear();
    uint32_t count = ss_player_animation_count(PlayerAt(0));
    for (uint32_t i = 0; i < count; i++) {
        const char *name = ss_player_animation_name(PlayerAt(0), i);
        // The pointer is only valid until the next call, so copy it now.
        if (name != nullptr) {
            anims_.push_back(std::string(name));
        }
    }
    packIndex_ = index;
    PlayAnimationAtIndex(0);
    // Packs differ in canvas size, and a player only has one once it has an
    // animation, so the fit is recomputed here rather than at load.
    LayOutPlayers();
}

void Viewer::PlayAnimationAtIndex(size_t index) {
    if (anims_.empty()) {
        return;
    }
    animIndex_ = index % anims_.size();
    for (size_t i = 0; i < players_.size(); i++) {
        // Selecting is the allocating step; starting is free.
        if (ss_player_set_animation(players_[i], anims_[animIndex_].c_str()) != SS_OK) {
            fprintf(stderr, "set_animation %s: %s\n", anims_[animIndex_].c_str(), ss_last_error());
            return;
        }
        ss_player_play(players_[i]);
    }
    paused_ = false;
    PrintStatus();
}

// --- frame loop ------------------------------------------------------------

void Viewer::Tick() {
    if (scene_ == nullptr) {
        return;
    }

    double now = Now();
    float delta = (float)(now - lastTick_);
    lastTick_ = now;
    // A resize drag or a stalled present can leave a gap of seconds; stepping
    // the animation by all of it would look like a skip.
    if (delta > 0.25f) {
        delta = 0.25f;
    }

    // One update and one render for the whole window, whatever the player count:
    // they share a scene, so they share the pass.
    ss_scene_update(scene_, delta);
    if (ss_scene_render(scene_) != SS_OK) {
        fprintf(stderr, "render: %s\n", ss_last_error());
    }
}

void Viewer::Resize(uint32_t width, uint32_t height) {
    if (scene_ == nullptr || width == 0 || height == 0) {
        return; // the scene keeps its last good size
    }
    if (width == width_ && height == height_) {
        return; // X11 sends ConfigureNotify for moves too
    }
    ss_scene_resize(scene_, width, height);
    // Placement is in projection units, which are the target's pixels, so the
    // cast has to be re-fitted to the new window rather than left where it was.
    width_ = width;
    height_ = height;
    LayOutPlayers();
}

// --- input -----------------------------------------------------------------

void Viewer::HandleKey(Key key) {
    switch (key) {
    case Key::Pause:
        paused_ = !paused_;
        for (size_t i = 0; i < players_.size(); i++) {
            if (paused_) {
                ss_player_pause(players_[i]);
            } else {
                ss_player_play(players_[i]);
            }
        }
        PrintStatus();
        break;
    case Key::NextPack:
        LoadPackAtIndex((packIndex_ + 1) % packs_.size());
        break;
    case Key::PrevPack:
        LoadPackAtIndex((packIndex_ + packs_.size() - 1) % packs_.size());
        break;
    case Key::NextAnim:
        PlayAnimationAtIndex(animIndex_ + 1);
        break;
    case Key::PrevAnim:
        if (!anims_.empty()) {
            PlayAnimationAtIndex(animIndex_ + anims_.size() - 1);
        }
        break;
    case Key::Restart:
        PlayAnimationAtIndex(animIndex_);
        break;
    // Adding a character costs a playback tree and nothing on the GPU: the pack
    // and its atlas are already there.
    case Key::AddPlayer:
        if (players_.size() < kMaxPlayers) {
            AddPlayer();
            LoadPackAtIndex(packIndex_);
        }
        break;
    case Key::RemovePlayer:
        RemovePlayer();
        PrintStatus();
        break;
    case Key::Quit:      // the window backend owns the loop, so it owns quitting
    case Key::Unhandled: // a key this viewer has nothing to do with
        break;
    }
}

void Viewer::PrintStatus() {
    float canvasWidth = 0.0f, canvasHeight = 0.0f;
    ss_player_canvas_size(PlayerAt(0), &canvasWidth, &canvasHeight);
    printf("\033[2J\033[H");
    printf("====================================================\n");
    printf("   SpriteStudio 7 wgpu Viewer — Vulkan / C API\n");
    printf("====================================================\n");
    // Which arm of `surface_target` this run is exercising. Xwayland makes the
    // two indistinguishable from the desktop, so it is worth printing.
    printf("  Window:        %s\n", windowSystem_);
    printf("  Active File:   %s (%zu/%zu)\n", FileName(packs_[packIndex_]).c_str(), packIndex_ + 1, packs_.size());
    printf("  Animation:     %s (%zu/%zu)\n", anims_.empty() ? "none" : anims_[animIndex_].c_str(), animIndex_ + 1, anims_.size());
    printf("  Canvas:        %.0f x %.0f\n", canvasWidth, canvasHeight);
    printf("  Players:       %u in one scene\n", ss_scene_player_count(scene_));
    printf("  Playback:      %s\n", paused_ ? "Paused" : "Playing");
    // What the frame cost. GPU time is 0 when the adapter has no timestamp query
    // — which is the case on lavapipe — and lags a frame or two when it does.
    ss_scene_stats_t stats;
    if (ss_scene_stats(scene_, &stats) == SS_OK) {
        printf("  Frame:         %u prims / %u draws / %.1f KiB\n", stats.prims, stats.draw_calls, (double)stats.uploaded_bytes / 1024.0);
        if (stats.gpu_pass_time_ns > 0) {
            printf("  GPU pass:      %.3f ms\n", (double)stats.gpu_pass_time_ns / 1.0e6);
        } else {
            printf("  GPU pass:      n/a (no timestamp query)\n");
        }
    }
    printf("----------------------------------------------------\n");
    printf("  Space  pause    Left/Right  .ssab    Up/Down  anim\n");
    printf("  R      restart  +/-  players  Escape   quit\n");
    printf("====================================================\n");
    fflush(stdout);
}

// --- X11 window ------------------------------------------------------------
//
// `SS_WINDOW_XLIB` wants the `Window` XID, the `Display*` and the screen number,
// which is all Xlib hands back from creating a window. Mesa's Vulkan WSI takes
// that same `Display*`, pulls the XCB connection out of it and presents on its
// own thread — hence the `XInitThreads` below.

#if SS_HAVE_X11
static Key KeyFromX11(KeySym symbol) {
    switch (symbol) {
    case XK_Escape:
        return Key::Quit;
    case XK_space:
        return Key::Pause;
    case XK_Left:
        return Key::PrevPack;
    case XK_Right:
        return Key::NextPack;
    case XK_Up:
        return Key::PrevAnim;
    case XK_Down:
        return Key::NextAnim;
    case XK_r:
    case XK_R:
        return Key::Restart;
    // The unshifted keysym is what `XLookupKeysym(.., 0)` returns, so the `+`
    // that shares a key with `=` arrives as `equal`.
    case XK_plus:
    case XK_equal:
    case XK_KP_Add:
        return Key::AddPlayer;
    case XK_minus:
    case XK_KP_Subtract:
        return Key::RemovePlayer;
    default:
        return Key::Unhandled;
    }
}

static int RunX11(Viewer &viewer) {
    // Xlib and the WSI's present thread share this connection, so its locks have
    // to be armed before it is opened. Without this the viewer dies somewhere
    // inside libxcb, minutes in and never in the same place twice.
    XInitThreads();

    Display *display = XOpenDisplay(nullptr);
    if (display == nullptr) {
        fprintf(stderr, "XOpenDisplay failed — is DISPLAY set? (on a Wayland session, Xwayland provides it)\n");
        return 1;
    }
    int screen = DefaultScreen(display);

    XSetWindowAttributes attributes;
    memset(&attributes, 0, sizeof(attributes));
    attributes.background_pixel = BlackPixel(display, screen);
    attributes.event_mask = KeyPressMask | StructureNotifyMask;
    Window window = XCreateWindow(display, RootWindow(display, screen), 0, 0, kDefaultWidth, kDefaultHeight, 0, CopyFromParent, InputOutput,
                                  (Visual *)CopyFromParent, CWBackPixel | CWEventMask, &attributes);

    XStoreName(display, window, "SpriteStudio 7 wgpu Viewer (Vulkan / C API)");
    // Without this the close button kills the connection under us, and the
    // scene would be destroyed after its window rather than before it.
    Atom deleteWindow = XInternAtom(display, "WM_DELETE_WINDOW", False);
    XSetWMProtocols(display, window, &deleteWindow, 1);
    XMapWindow(display, window);

    // Map first, then ask what size the window manager actually gave us: the
    // swapchain is created for that size, and a mismatch would be presented at
    // the wrong scale until the first resize.
    XEvent event;
    do {
        XNextEvent(display, &event);
    } while (event.type != MapNotify);
    XWindowAttributes geometry;
    XGetWindowAttributes(display, window, &geometry);

    ss_window_handle_t handle;
    memset(&handle, 0, sizeof(handle));
    handle.kind = SS_WINDOW_XLIB;
    handle.screen = screen;
    handle.window = (void *)(uintptr_t)window; // the XID, cast to a pointer
    handle.display = display;

    int status = 0;
    if (viewer.Start(handle, (uint32_t)geometry.width, (uint32_t)geometry.height)) {
        // Present mode is Fifo (host.rs), so the render call is what paces us:
        // no sleep, no timer, one frame per vblank.
        bool running = true;
        bool mapped = true;
        while (running) {
            while (XPending(display) > 0) {
                XNextEvent(display, &event);
                switch (event.type) {
                case ConfigureNotify:
                    viewer.Resize((uint32_t)event.xconfigure.width, (uint32_t)event.xconfigure.height);
                    break;
                case MapNotify:
                    mapped = true;
                    break;
                case UnmapNotify:
                    mapped = false;
                    break;
                case KeyPress: {
                    Key key = KeyFromX11(XLookupKeysym(&event.xkey, 0));
                    if (key == Key::Quit) {
                        running = false;
                    } else {
                        viewer.HandleKey(key);
                    }
                    break;
                }
                case ClientMessage:
                    if ((Atom)event.xclient.data.l[0] == deleteWindow) {
                        running = false;
                    }
                    break;
                default:
                    break;
                }
            }
            if (!running) {
                break;
            }
            if (!mapped) {
                XPeekEvent(display, &event); // iconified: nothing to present, do not spin
                continue;
            }
            viewer.Tick();
        }
    } else {
        status = 1;
    }

    // The window has to outlive the surface built from its XID.
    viewer.Shutdown();
    XDestroyWindow(display, window);
    XCloseDisplay(display);
    return status;
}
#endif // SS_HAVE_X11

// --- Wayland window ---------------------------------------------------------
//
// `SS_WINDOW_WAYLAND` wants the `wl_surface*` and the `wl_display*`. Getting a
// surface that a compositor will show means xdg-shell: create the surface, wrap
// it in an xdg_surface and an xdg_toplevel, commit once with no buffer, and wait
// for the configure that answers it. Only then is there a size to build a
// swapchain for, so `ss_scene_create` happens once the frame loop is running
// rather than up front.
//
// There is no xdg-decoration here, so under GNOME the window has no title bar —
// that is the compositor's choice for a client that does not draw its own, not a
// fault in the viewer. Escape still quits, and Super+drag still moves it.

#if SS_HAVE_WAYLAND
struct WaylandState {
    wl_display *display = nullptr;
    wl_registry *registry = nullptr;
    wl_compositor *compositor = nullptr;
    xdg_wm_base *wmBase = nullptr;
    wl_seat *seat = nullptr;
    wl_keyboard *keyboard = nullptr;
    wl_surface *surface = nullptr;
    xdg_surface *xdgSurface = nullptr;
    xdg_toplevel *toplevel = nullptr;

    Viewer *viewer = nullptr;
    /// Size the last toplevel configure asked for, applied when the xdg_surface
    /// configure that follows it is acknowledged. Zero means "you choose".
    uint32_t pendingWidth = 0;
    uint32_t pendingHeight = 0;
    uint32_t width = kDefaultWidth;
    uint32_t height = kDefaultHeight;
    bool configured = false; // a configure has been acknowledged: the size is real
    bool started = false;    // the scene exists, so frames can be drawn
    bool failed = false;     // ss_scene_create said no
    bool running = true;
};

static Key KeyFromEvdev(uint32_t code) {
    // Raw evdev codes rather than xkbcommon keysyms: the alternative is another
    // dependency and a keymap to compile, for a sample whose subject is the C
    // API. The cost is that R and +/- follow the physical QWERTY positions
    // whatever the layout says; the arrows, Space and Escape are unaffected.
    switch (code) {
    case KEY_ESC:
        return Key::Quit;
    case KEY_SPACE:
        return Key::Pause;
    case KEY_LEFT:
        return Key::PrevPack;
    case KEY_RIGHT:
        return Key::NextPack;
    case KEY_UP:
        return Key::PrevAnim;
    case KEY_DOWN:
        return Key::NextAnim;
    case KEY_R:
        return Key::Restart;
    case KEY_EQUAL:
    case KEY_KPPLUS:
        return Key::AddPlayer;
    case KEY_MINUS:
    case KEY_KPMINUS:
        return Key::RemovePlayer;
    default:
        return Key::Unhandled;
    }
}

// A compositor that gets no pong assumes the client has hung and tells the user
// so; this is the whole of being a well-behaved xdg-shell client.
static void WmBasePing(void *, xdg_wm_base *wmBase, uint32_t serial) {
    xdg_wm_base_pong(wmBase, serial);
}

static const xdg_wm_base_listener kWmBaseListener = {
    .ping = WmBasePing,
};

// The keymap arrives as a file descriptor even though this viewer reads evdev
// codes directly, and the compositor sends a fresh one on every layout change.
// Closing it is not optional.
static void KeyboardKeymap(void *, wl_keyboard *, uint32_t, int32_t fd, uint32_t) {
    close(fd);
}

static void KeyboardEnter(void *, wl_keyboard *, uint32_t, wl_surface *, wl_array *) {}
static void KeyboardLeave(void *, wl_keyboard *, uint32_t, wl_surface *) {}
static void KeyboardModifiers(void *, wl_keyboard *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t) {}
static void KeyboardRepeatInfo(void *, wl_keyboard *, int32_t, int32_t) {}

static void KeyboardKey(void *data, wl_keyboard *, uint32_t, uint32_t, uint32_t code, uint32_t state) {
    WaylandState *wl = (WaylandState *)data;
    if (state != WL_KEYBOARD_KEY_STATE_PRESSED) {
        return;
    }
    Key key = KeyFromEvdev(code);
    if (key == Key::Quit) {
        wl->running = false;
    } else if (wl->started) {
        // Keyboard focus can arrive before the first configure has let the scene
        // be built, and a viewer with no packs yet has nothing to walk.
        wl->viewer->HandleKey(key);
    }
}

static const wl_keyboard_listener kKeyboardListener = {
    .keymap = KeyboardKeymap,
    .enter = KeyboardEnter,
    .leave = KeyboardLeave,
    .key = KeyboardKey,
    .modifiers = KeyboardModifiers,
    .repeat_info = KeyboardRepeatInfo,
};

static void SeatCapabilities(void *data, wl_seat *seat, uint32_t capabilities) {
    WaylandState *wl = (WaylandState *)data;
    bool hasKeyboard = (capabilities & WL_SEAT_CAPABILITY_KEYBOARD) != 0;
    if (hasKeyboard && wl->keyboard == nullptr) {
        wl->keyboard = wl_seat_get_keyboard(seat);
        wl_keyboard_add_listener(wl->keyboard, &kKeyboardListener, wl);
    } else if (!hasKeyboard && wl->keyboard != nullptr) {
        // `destroy`, not `release`: the seat is bound at version 1, and the
        // release request only exists from version 3 — sending it would be an
        // unknown opcode, which is a protocol error the compositor kills the
        // client for.
        wl_keyboard_destroy(wl->keyboard);
        wl->keyboard = nullptr;
    }
}

static void SeatName(void *, wl_seat *, const char *) {}

static const wl_seat_listener kSeatListener = {
    .capabilities = SeatCapabilities,
    .name = SeatName,
};

/// The size the compositor wants, which is not applied until the xdg_surface
/// configure that follows this one — the two events are one atomic change.
static void ToplevelConfigure(void *data, xdg_toplevel *, int32_t width, int32_t height, wl_array *) {
    WaylandState *wl = (WaylandState *)data;
    wl->pendingWidth = width > 0 ? (uint32_t)width : 0;
    wl->pendingHeight = height > 0 ? (uint32_t)height : 0;
}

static void ToplevelClose(void *data, xdg_toplevel *) {
    ((WaylandState *)data)->running = false;
}

static const xdg_toplevel_listener kToplevelListener = {
    .configure = ToplevelConfigure,
    .close = ToplevelClose,
};

/// Where the window becomes real: acknowledging a configure is what turns the
/// pending size into the agreed one, and the first one is the point from which
/// a surface can carry a buffer at all.
///
/// It only records. Building the scene and resizing it both build a swapchain,
/// and a swapchain on this backend talks to the very `wl_display` whose dispatch
/// called this handler — so both happen in the frame loop, one dispatch later.
/// No buffer is attached here either: the swapchain owns the buffers from now
/// on, and attaches the first one when the first frame is presented.
static void XdgSurfaceConfigure(void *data, xdg_surface *xdgSurface, uint32_t serial) {
    WaylandState *wl = (WaylandState *)data;
    xdg_surface_ack_configure(xdgSurface, serial);

    if (wl->pendingWidth != 0 && wl->pendingHeight != 0) {
        wl->width = wl->pendingWidth;
        wl->height = wl->pendingHeight;
    }
    wl->configured = true;
}

static const xdg_surface_listener kXdgSurfaceListener = {
    .configure = XdgSurfaceConfigure,
};

static void RegistryGlobal(void *data, wl_registry *registry, uint32_t name, const char *interface, uint32_t version) {
    WaylandState *wl = (WaylandState *)data;
    if (strcmp(interface, wl_compositor_interface.name) == 0) {
        // Version 4 lets Mesa's WSI post buffer-space damage instead of
        // repainting the whole surface; anything lower and it falls back.
        uint32_t wanted = std::min(version, (uint32_t)wl_compositor_interface.version);
        wl->compositor = (wl_compositor *)wl_registry_bind(registry, name, &wl_compositor_interface, std::min(wanted, 4u));
    } else if (strcmp(interface, xdg_wm_base_interface.name) == 0) {
        // Deliberately version 1: xdg_toplevel later gained configure_bounds and
        // wm_capabilities, and a listener with no handler for an event the
        // compositor is entitled to send is a null call, not a warning.
        wl->wmBase = (xdg_wm_base *)wl_registry_bind(registry, name, &xdg_wm_base_interface, 1);
        xdg_wm_base_add_listener(wl->wmBase, &kWmBaseListener, wl);
    } else if (strcmp(interface, wl_seat_interface.name) == 0 && wl->seat == nullptr) {
        wl->seat = (wl_seat *)wl_registry_bind(registry, name, &wl_seat_interface, 1);
        wl_seat_add_listener(wl->seat, &kSeatListener, wl);
    }
}

static void RegistryGlobalRemove(void *, wl_registry *, uint32_t) {}

static const wl_registry_listener kRegistryListener = {
    .global = RegistryGlobal,
    .global_remove = RegistryGlobalRemove,
};

/// Read whatever the compositor has sent without blocking, and without racing
/// the WSI's own event queue.
///
/// The prepare/read dance rather than `wl_display_dispatch`: Mesa's Wayland WSI
/// dispatches frame callbacks on a queue of its own from another thread, and the
/// plain dispatch can block waiting for a socket that thread has already
/// drained.
///
/// `timeout` must stay finite. Draining the queue up front can dispatch the very
/// event the caller is waiting for, and the poll that follows would then have
/// nothing left to wake it.
static bool PumpWayland(wl_display *display, int timeout) {
    while (wl_display_prepare_read(display) != 0) {
        if (wl_display_dispatch_pending(display) < 0) {
            return false;
        }
    }
    if (wl_display_flush(display) < 0 && errno != EAGAIN) {
        wl_display_cancel_read(display);
        return false;
    }

    struct pollfd fd;
    fd.fd = wl_display_get_fd(display);
    fd.events = POLLIN;
    fd.revents = 0;
    if (poll(&fd, 1, timeout) > 0 && (fd.revents & POLLIN) != 0) {
        if (wl_display_read_events(display) < 0) {
            return false;
        }
    } else {
        wl_display_cancel_read(display);
    }
    return wl_display_dispatch_pending(display) >= 0;
}

static int RunWayland(Viewer &viewer) {
    WaylandState wl;
    wl.viewer = &viewer;

    wl.display = wl_display_connect(nullptr);
    if (wl.display == nullptr) {
        fprintf(stderr, "wl_display_connect failed — is WAYLAND_DISPLAY set? (pass --x11 for an X11 session)\n");
        return 1;
    }
    wl.registry = wl_display_get_registry(wl.display);
    wl_registry_add_listener(wl.registry, &kRegistryListener, &wl);
    // Two rounds: the first delivers the globals, the second the events the
    // binds in the first one triggered (the seat's capabilities).
    wl_display_roundtrip(wl.display);
    wl_display_roundtrip(wl.display);

    int status = 1;
    if (wl.compositor == nullptr || wl.wmBase == nullptr) {
        fprintf(stderr, "the compositor offers no %s\n", wl.compositor == nullptr ? "wl_compositor" : "xdg_wm_base");
    } else {
        wl.surface = wl_compositor_create_surface(wl.compositor);
        wl.xdgSurface = xdg_wm_base_get_xdg_surface(wl.wmBase, wl.surface);
        xdg_surface_add_listener(wl.xdgSurface, &kXdgSurfaceListener, &wl);
        wl.toplevel = xdg_surface_get_toplevel(wl.xdgSurface);
        xdg_toplevel_add_listener(wl.toplevel, &kToplevelListener, &wl);
        xdg_toplevel_set_title(wl.toplevel, "SpriteStudio 7 wgpu Viewer (Vulkan / C API)");
        xdg_toplevel_set_app_id(wl.toplevel, "jp.co.webtech.ssplayer-wgpu.viewer");

        // The commit that asks for the first configure. It must carry no buffer:
        // the surface has no agreed size yet, and the swapchain does not exist.
        wl_surface_commit(wl.surface);

        // Idling at 10 Hz until the first configure arrives, and not waiting at
        // all after that — present mode is Fifo (host.rs), so from there the
        // render call is what paces us: one frame per frame callback.
        while (wl.running) {
            if (!PumpWayland(wl.display, wl.started ? 0 : 100)) {
                fprintf(stderr, "the Wayland connection went away\n");
                wl.failed = true;
                break;
            }
            if (!wl.configured || !wl.running) {
                continue; // no agreed size yet, so nothing to build a swapchain on
            }
            if (!wl.started) {
                ss_window_handle_t handle;
                memset(&handle, 0, sizeof(handle));
                handle.kind = SS_WINDOW_WAYLAND;
                handle.window = wl.surface;
                handle.display = wl.display;
                if (!viewer.Start(handle, wl.width, wl.height)) {
                    wl.failed = true;
                    break;
                }
                wl.started = true;
            } else {
                // A no-op unless a configure changed the size since the last
                // frame, which is what makes calling it every frame the whole of
                // resize handling here.
                viewer.Resize(wl.width, wl.height);
            }
            viewer.Tick();
        }
        status = wl.failed ? 1 : 0;
    }

    // The surface has to outlive the swapchain built from it.
    viewer.Shutdown();
    if (wl.toplevel != nullptr) {
        xdg_toplevel_destroy(wl.toplevel);
    }
    if (wl.xdgSurface != nullptr) {
        xdg_surface_destroy(wl.xdgSurface);
    }
    if (wl.surface != nullptr) {
        wl_surface_destroy(wl.surface);
    }
    if (wl.keyboard != nullptr) {
        wl_keyboard_destroy(wl.keyboard);
    }
    wl_display_disconnect(wl.display);
    return status;
}
#endif // SS_HAVE_WAYLAND

// --- entry point ------------------------------------------------------------

enum class WindowSystem { Auto, X11, Wayland };

static void PrintUsage() {
    printf("ssviewer-vulkan [--x11|--wayland] [asset-dir] [pack-name]\n"
           "\n"
           "  --x11        force the SS_WINDOW_XLIB path (Xwayland on a Wayland session)\n"
           "  --wayland    force the SS_WINDOW_WAYLAND path\n"
           "               default: Wayland when WAYLAND_DISPLAY is set, else X11\n"
           "  asset-dir    directory of .ssab files; \"\" keeps the built-in default\n"
           "  pack-name    pack to open on, without the extension (default: Basic)\n");
}

int main(int argc, char **argv) {
    WindowSystem wanted = WindowSystem::Auto;
    std::vector<std::string> positional;
    for (int i = 1; i < argc; i++) {
        std::string arg = argv[i];
        if (arg == "--x11" || arg == "--xlib") {
            wanted = WindowSystem::X11;
        } else if (arg == "--wayland") {
            wanted = WindowSystem::Wayland;
        } else if (arg == "--help" || arg == "-h") {
            PrintUsage();
            return 0;
        } else if (arg.rfind("--", 0) == 0) {
            fprintf(stderr, "unknown option %s\n", arg.c_str());
            PrintUsage();
            return 1;
        } else {
            positional.push_back(arg);
        }
    }

    std::string assetDir = FindAssetDir(positional.empty() ? std::string() : positional[0]);
    std::string initialPack = positional.size() > 1 ? positional[1] : std::string();

    // A Wayland session sets WAYLAND_DISPLAY and, through Xwayland, DISPLAY as
    // well — so the environment answers "which one is native here", and only
    // that way round.
    if (wanted == WindowSystem::Auto) {
        const char *waylandDisplay = getenv("WAYLAND_DISPLAY");
        wanted = (waylandDisplay != nullptr && waylandDisplay[0] != '\0') ? WindowSystem::Wayland : WindowSystem::X11;
    }

    if (wanted == WindowSystem::Wayland) {
#if SS_HAVE_WAYLAND
        Viewer viewer(assetDir, initialPack, "Wayland (SS_WINDOW_WAYLAND)");
        return RunWayland(viewer);
#else
        fprintf(stderr, "this build has no Wayland support (libwayland-dev and wayland-protocols were missing at configure time)\n");
        return 1;
#endif
    }

#if SS_HAVE_X11
    Viewer viewer(assetDir, initialPack, "X11 / Xlib (SS_WINDOW_XLIB)");
    return RunX11(viewer);
#else
    fprintf(stderr, "this build has no X11 support (libx11-dev was missing at configure time)\n");
    return 1;
#endif
}
