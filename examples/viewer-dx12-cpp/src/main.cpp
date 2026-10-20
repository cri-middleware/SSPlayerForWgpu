// DX12 (Windows) verification viewer for the ssplayer-wgpu C API.
//
// This is the C face end to end: Win32 owns the window, hands its `HWND`
// to `ss_scene_create`, and drives the scene with `ss_scene_update` /
// `ss_scene_render` — no Rust in sight. The scene's own host (host.rs) builds
// the wgpu stack behind the ABI, with backend=DX12 forced so this really
// exercises D3D12 rather than whatever wgpu would pick.
//
// It is the Windows twin of `examples/viewer-metal-cpp`, and the twins
// deliberately duplicate the playback loop instead of sharing it: the point of
// the sample is that one directory is self-contained. What actually
// differs is the first twenty lines of `WinMain` — HWND instead of CAMetalLayer,
// `SS_WINDOW_WIN32` instead of `SS_WINDOW_METAL_LAYER`. The C-API calls below
// are the same calls in the same order.
//
// It starts on the reference-mode packs from assets/overall, matching the Rust
// viewer; the embedded set is in assets/overall_emb and can be passed as argv[1].
//
// **Drop a .ssab (or a folder of them) on the window to play it**, whichever way
// it was exported. `ss_player_load(player, path)` is the whole of loading — the
// pack, its atlases in either export mode, and the sibling .ssab files its
// instance parts play from, all resolved against the directory the drop came
// from.
//
// **It shows two characters at once by default**: the
// window is `ss_scene_t` and each character is an `ss_player_t` placed with
// `ss_player_set_transform`, sharing one device, one renderer and one atlas
// upload.
//
//   ssviewer-dx12 [asset-dir] [pack-name]     e.g. ssviewer-dx12 "" Mask
//
//   Space       pause / resume        Left/Right  previous / next .ssab
//   R           restart animation     Up/Down     previous / next animation
//   +/-         add / remove player   Escape      quit
//   drop        play a .ssab / add every .ssab in a folder

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
// Windows 10 headers: WM_DPICHANGED and the per-monitor DPI entry points are
// guarded on this. The functions are still resolved at runtime (see below), so
// the binary keeps running on whatever the loader gives it.
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0A00
#endif
#include <windows.h>
#include <shellapi.h>

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <cwchar>
#include <filesystem>
#include <string>
#include <vector>

#ifndef ENABLE_VIRTUAL_TERMINAL_PROCESSING
#define ENABLE_VIRTUAL_TERMINAL_PROCESSING 0x0004
#endif

#include "ssplayer_wgpu.h"

/// Where scripts/deploy-examples.ps1 puts the demo packs. Baked in by CMake so
/// the viewer runs from any working directory; argv[1] overrides it.
#ifndef SS_DEFAULT_ASSET_DIR
#define SS_DEFAULT_ASSET_DIR "assets/overall"
#endif

/// How many characters to show at startup. Two is the point of the exercise.
static const size_t kInitialPlayers = 2;
/// Most players the +/- keys will go to.
static const size_t kMaxPlayers = 6;
/// Timer fired while a modal resize/move loop owns the thread, so the animation
/// keeps running while the window is being dragged (the Win32 equivalent of the
/// Metal viewer's NSRunLoopCommonModes timer).
static const UINT_PTR kModalLoopTimer = 1;

// --- strings -------------------------------------------------------------
//
// The C API takes UTF-8 (`ffi.rs` decodes with `CStr::to_str`), while Win32
// gives us UTF-16. So everything stays wide inside this file and is converted
// exactly at the ABI boundary — never through the ANSI code page, which is what
// `main(argc, argv)` and the `*A` entry points would hand us.

static std::string Utf8(const std::wstring &wide) {
    if (wide.empty()) {
        return std::string();
    }
    int len = WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), (int)wide.size(), nullptr, 0, nullptr, nullptr);
    if (len <= 0) {
        return std::string();
    }
    std::string out((size_t)len, '\0');
    WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), (int)wide.size(), &out[0], len, nullptr, nullptr);
    return out;
}

static std::wstring Wide(const std::string &utf8) {
    if (utf8.empty()) {
        return std::wstring();
    }
    int len = MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), (int)utf8.size(), nullptr, 0);
    if (len <= 0) {
        return std::wstring();
    }
    std::wstring out((size_t)len, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), (int)utf8.size(), &out[0], len);
    return out;
}

// Path questions go to `std::filesystem::path`, not to a scan for separators —
// the same call the player makes when it hands `src` to the loader instead of
// cutting it itself. It already knows that both `\` and `/` separate
// here, that a trailing one must not be doubled, and where a `\\?\` prefix or a
// drive letter ends; a `find_last_of` has to be told each of those by hand.

static bool HasExtension(const std::wstring &path, const wchar_t *ext) {
    // `extension()` keeps the dot and, unlike a `find_last_of(L'.')`, looks only
    // at the file name — a dot in a parent directory is not an extension.
    std::wstring found = std::filesystem::path(path).extension().native();
    return !found.empty() && _wcsicmp(found.c_str() + 1, ext) == 0;
}

static std::wstring FileName(const std::wstring &path) {
    return std::filesystem::path(path).filename().native();
}

static std::wstring Join(const std::wstring &dir, const std::wstring &leaf) {
    return (std::filesystem::path(dir) / leaf).native();
}

static bool IsDirectory(const std::wstring &path) {
    DWORD attrs = GetFileAttributesW(path.c_str());
    return attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;
}

static bool IsReadableFile(const std::wstring &path) {
    DWORD attrs = GetFileAttributesW(path.c_str());
    return attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_DIRECTORY) == 0;
}

/// Every `.ssab` directly inside `dir`, sorted, so a folder plays in the order
/// the Left/Right keys will walk it.
static std::vector<std::wstring> SsabFilesIn(const std::wstring &dir) {
    std::vector<std::wstring> found;
    WIN32_FIND_DATAW entry;
    HANDLE search = FindFirstFileW(Join(dir, L"*.ssab").c_str(), &entry);
    if (search == INVALID_HANDLE_VALUE) {
        return found;
    }
    do {
        if ((entry.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) == 0) {
            found.push_back(Join(dir, entry.cFileName));
        }
    } while (FindNextFileW(search, &entry));
    FindClose(search);
    std::sort(found.begin(), found.end(), [](const std::wstring &a, const std::wstring &b) { return _wcsicmp(a.c_str(), b.c_str()) < 0; });
    return found;
}

/// Collect the `.ssab` files behind a drop: the files themselves, plus the ones
/// directly inside any dropped folder.
static std::vector<std::wstring> CollectSsabPaths(HDROP drop) {
    std::vector<std::wstring> paths;
    UINT count = DragQueryFileW(drop, 0xFFFFFFFF, nullptr, 0);
    for (UINT i = 0; i < count; i++) {
        UINT len = DragQueryFileW(drop, i, nullptr, 0);
        if (len == 0) {
            continue;
        }
        std::wstring path(len + 1, L'\0');
        DragQueryFileW(drop, i, &path[0], len + 1);
        path.resize(len);
        if (IsDirectory(path)) {
            std::vector<std::wstring> inside = SsabFilesIn(path);
            paths.insert(paths.end(), inside.begin(), inside.end());
        } else if (HasExtension(path, L"ssab")) {
            paths.push_back(path);
        }
    }
    return paths;
}

// --- console -------------------------------------------------------------

/// Whether the console understands the ANSI clear-screen the status uses. Older
/// consoles print the escape as garbage, so it is only emitted when the mode
/// actually took.
static bool g_ansiConsole = false;

static void PrepareConsole() {
    SetConsoleOutputCP(CP_UTF8);
    HANDLE out = GetStdHandle(STD_OUTPUT_HANDLE);
    DWORD mode = 0;
    if (out != INVALID_HANDLE_VALUE && GetConsoleMode(out, &mode)) {
        g_ansiConsole = SetConsoleMode(out, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0;
    }
}

// --- DPI -----------------------------------------------------------------
//
// Resolved at runtime rather than compiled against, so the viewer builds with
// any Windows SDK and still runs unscaled on a per-monitor-DPI desktop. Without
// this the OS would stretch a 96-DPI backbuffer and the "does DX12 present at
// the right size" question this viewer exists to answer would be answered by
// the compositor instead.

typedef BOOL(WINAPI *SetProcessDpiAwarenessContextFn)(HANDLE);
typedef UINT(WINAPI *GetDpiForWindowFn)(HWND);

static void EnablePerMonitorDpi() {
    HMODULE user32 = GetModuleHandleW(L"user32.dll");
    if (user32 != nullptr) {
        SetProcessDpiAwarenessContextFn setContext = (SetProcessDpiAwarenessContextFn)GetProcAddress(user32, "SetProcessDpiAwarenessContext");
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 == (HANDLE)-4
        if (setContext != nullptr && setContext((HANDLE)(INT_PTR)-4)) {
            return;
        }
    }
    SetProcessDPIAware(); // Vista+, system-DPI only
}

static UINT DpiForWindow(HWND hwnd) {
    HMODULE user32 = GetModuleHandleW(L"user32.dll");
    if (user32 != nullptr) {
        GetDpiForWindowFn getDpi = (GetDpiForWindowFn)GetProcAddress(user32, "GetDpiForWindow");
        if (getDpi != nullptr) {
            UINT dpi = getDpi(hwnd);
            if (dpi != 0) {
                return dpi;
            }
        }
    }
    return USER_DEFAULT_SCREEN_DPI;
}

// --- viewer --------------------------------------------------------------

class Viewer {
  public:
    Viewer(const std::wstring &assetDir, const std::wstring &initialPack) : assetDir_(assetDir), initialPack_(initialPack) {}

    bool Start(HWND hwnd);
    void Shutdown();

    void Tick();
    void Resize(uint32_t width, uint32_t height);
    bool HandleKey(WPARAM key);
    bool HandleDrop(HDROP drop);

  private:
    ss_player_t *PlayerAt(size_t index) const { return players_[index]; }
    void AddPlayer();
    void RemovePlayer();
    void LayOutPlayers();

    bool CollectPacks();
    void LoadPackAtIndex(size_t index);
    void PlayAnimationAtIndex(size_t index);
    void PrintStatus();

    HWND hwnd_ = nullptr;
    ss_scene_t *scene_ = nullptr;
    /// The scene's size in physical pixels, which is also what placement is
    /// measured in — the projection maps one unit to one target pixel. Kept
    /// here because the lay-out divides the window between the cast.
    uint32_t width_ = 0;
    uint32_t height_ = 0;
    /// One handle per character. All of them draw into `scene_`.
    std::vector<ss_player_t *> players_;

    std::wstring assetDir_;
    std::wstring initialPack_; // pack to show first, or empty for Basic

    std::vector<std::wstring> packs_; // .ssab paths: the asset dir, then whatever was dropped
    size_t packIndex_ = 0;
    std::vector<std::string> anims_; // animation names of the current pack, UTF-8 as the API gave them
    size_t animIndex_ = 0;

    bool paused_ = false;
    LARGE_INTEGER lastTick_ = {};
    LARGE_INTEGER frequency_ = {};
};

// --- setup ---------------------------------------------------------------

bool Viewer::Start(HWND hwnd) {
    hwnd_ = hwnd;

    // GetClientRect is already in physical pixels once the process is
    // per-monitor aware, which is exactly what the scene wants.
    RECT client;
    GetClientRect(hwnd, &client);

    ss_scene_desc_t desc;
    memset(&desc, 0, sizeof(desc));
    desc.window.kind = SS_WINDOW_WIN32;
    desc.window.window = (void *)hwnd;
    desc.window.display = (void *)GetWindowLongPtrW(hwnd, GWLP_HINSTANCE); // HINSTANCE, optional
    desc.backend = SS_BACKEND_DX12;
    desc.width = (uint32_t)(client.right - client.left);
    desc.height = (uint32_t)(client.bottom - client.top);
    // The first `AddPlayer` lays the cast out, so the size has to be known by
    // then rather than only from the first WM_SIZE.
    width_ = desc.width;
    height_ = desc.height;
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

    QueryPerformanceFrequency(&frequency_);
    QueryPerformanceCounter(&lastTick_);
    return true;
}

void Viewer::Shutdown() {
    // Destroying the scene takes its players with it; the handles must not be
    // touched afterwards.
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
/// Placement is per player; the projection belongs to the scene, and
/// it maps one unit to one target pixel, so the window's own size is the right
/// thing to divide. Fitting rather than a fixed scale is what the demo packs
/// need: they disagree about canvas size, so one constant either crowds two
/// characters into each other or runs a tall pack off the top. This is the Rust
/// viewer's Row layout, so the two faces can be read side by side.
void Viewer::LayOutPlayers() {
    size_t count = players_.size();
    if (count == 0 || width_ == 0 || height_ == 0) {
        return; // minimised, or nothing to place
    }
    float slot = (float)width_ / (float)count;
    float last = (float)(count - 1);
    SsPlacement placement;
    ss_placement_default(&placement);
    placement.scale = 0.9f;
    for (size_t i = 0; i < count; i++) {
        // The pack's authored canvas, placed by its pivot, contained in this
        // member's column at 90%; the offset moves the column's centre off the
        // view's. A player with no animation yet lands centred and unscaled.
        placement.offset_x = ((float)i - last * 0.5f) * slot;
        ss_player_fit_into(players_[i], slot, (float)height_, &placement, NULL);
        ss_player_set_order(players_[i], (int32_t)i);
    }
}

// --- assets --------------------------------------------------------------

/// Gather every .ssab in the asset directory and pick the one to open on.
bool Viewer::CollectPacks() {
    packs_ = SsabFilesIn(assetDir_);
    if (packs_.empty()) {
        fprintf(stderr,
                "no .ssab files in %s\n  run scripts\\deploy-examples.ps1, or pass a directory as argv[1]\n",
                Utf8(assetDir_).c_str());
        return false;
    }

    // Show the pack named on the command line, else Basic.ssab like the Rust viewer.
    std::wstring wanted = (initialPack_.empty() ? std::wstring(L"Basic") : initialPack_) + L".ssab";
    packIndex_ = 0;
    for (size_t i = 0; i < packs_.size(); i++) {
        if (_wcsicmp(FileName(packs_[i]).c_str(), wanted.c_str()) == 0) {
            packIndex_ = i;
            break;
        }
    }
    return true;
}

/// Play what was dropped on the window.
///
/// Dropped packs join the list the Left/Right keys walk rather than replacing
/// it, so a character dropped in can still be compared against the demo set —
/// and a folder dropped in brings its instance-referenced packs along, which is
/// what a pack with external references needs to resolve.
bool Viewer::HandleDrop(HDROP drop) {
    std::vector<std::wstring> paths = CollectSsabPaths(drop);
    size_t first = (size_t)-1;
    for (size_t i = 0; i < paths.size(); i++) {
        if (!IsReadableFile(paths[i])) {
            fprintf(stderr, "not a readable .ssab: %s\n", Utf8(paths[i]).c_str());
            continue;
        }
        size_t existing = packs_.size();
        for (size_t j = 0; j < packs_.size(); j++) {
            if (_wcsicmp(packs_[j].c_str(), paths[i].c_str()) == 0) {
                existing = j;
                break;
            }
        }
        if (existing == packs_.size()) {
            packs_.push_back(paths[i]);
        }
        if (first == (size_t)-1) {
            first = existing;
        }
    }
    if (first == (size_t)-1) {
        return false;
    }
    LoadPackAtIndex(first);
    return true;
}

/// Play one of the listed packs on every player.
///
/// `ss_player_load` is the whole of it: the pack, its atlases in either export
/// mode, and the sibling .ssab files its instance parts play from. A host whose
/// assets are not files walks `ss_player_texture_at` and feeds
/// `ss_scene_add_texture_png` instead.
void Viewer::LoadPackAtIndex(size_t index) {
    std::string path = Utf8(packs_[index]);
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

// --- frame loop ----------------------------------------------------------

void Viewer::Tick() {
    if (scene_ == nullptr || IsIconic(hwnd_)) {
        return;
    }

    LARGE_INTEGER now;
    QueryPerformanceCounter(&now);
    float delta = (float)((double)(now.QuadPart - lastTick_.QuadPart) / (double)frequency_.QuadPart);
    lastTick_ = now;
    // A modal resize loop or a stalled present can leave a gap of seconds;
    // stepping the animation by all of it would look like a skip.
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
        return; // minimised; the scene keeps its last good size
    }
    ss_scene_resize(scene_, width, height);
    // Placement is in projection units, which are the target's pixels, so the
    // cast has to be re-fitted to the new window rather than left where it was.
    width_ = width;
    height_ = height;
    LayOutPlayers();
}

// --- input ---------------------------------------------------------------

bool Viewer::HandleKey(WPARAM key) {
    switch (key) {
    case VK_ESCAPE:
        PostMessageW(hwnd_, WM_CLOSE, 0, 0);
        return true;
    case VK_SPACE:
        paused_ = !paused_;
        for (size_t i = 0; i < players_.size(); i++) {
            if (paused_) {
                ss_player_pause(players_[i]);
            } else {
                ss_player_resume(players_[i]);
            }
        }
        PrintStatus();
        return true;
    case VK_RIGHT:
        LoadPackAtIndex((packIndex_ + 1) % packs_.size());
        return true;
    case VK_LEFT:
        LoadPackAtIndex((packIndex_ + packs_.size() - 1) % packs_.size());
        return true;
    case VK_DOWN:
        PlayAnimationAtIndex(animIndex_ + 1);
        return true;
    case VK_UP:
        if (!anims_.empty()) {
            PlayAnimationAtIndex(animIndex_ + anims_.size() - 1);
        }
        return true;
    case 'R':
        PlayAnimationAtIndex(animIndex_);
        return true;
    // Adding a character costs a playback tree and nothing on the GPU: the pack
    // and its atlas are already there.
    case VK_OEM_PLUS:
    case VK_ADD:
        if (players_.size() < kMaxPlayers) {
            AddPlayer();
            LoadPackAtIndex(packIndex_);
        }
        return true;
    case VK_OEM_MINUS:
    case VK_SUBTRACT:
        RemovePlayer();
        PrintStatus();
        return true;
    default:
        return false;
    }
}

void Viewer::PrintStatus() {
    float canvasWidth = 0.0f, canvasHeight = 0.0f;
    ss_player_canvas_size(PlayerAt(0), &canvasWidth, &canvasHeight);
    if (g_ansiConsole) {
        printf("\033[2J\033[H");
    }
    printf("====================================================\n");
    printf("   SpriteStudio wgpu Viewer - DX12 / C API\n");
    printf("====================================================\n");
    printf("  Active File:   %s (%zu/%zu)\n", Utf8(FileName(packs_[packIndex_])).c_str(), packIndex_ + 1, packs_.size());
    printf("  Animation:     %s (%zu/%zu)\n", anims_.empty() ? "none" : anims_[animIndex_].c_str(), animIndex_ + 1, anims_.size());
    printf("  Canvas:        %.0f x %.0f\n", canvasWidth, canvasHeight);
    printf("  Players:       %u in one scene\n", ss_scene_player_count(scene_));
    printf("  Playback:      %s\n", paused_ ? "Paused" : "Playing");
    // What the frame cost. GPU time is 0 when the adapter
    // has no timestamp query, and lags a frame or two when it does.
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
    printf("  Drop a .ssab or a folder on the window to play it\n");
    printf("====================================================\n");
    fflush(stdout);
}

// --- window --------------------------------------------------------------

static LRESULT CALLBACK WindowProc(HWND hwnd, UINT msg, WPARAM wParam, LPARAM lParam) {
    Viewer *viewer = (Viewer *)GetWindowLongPtrW(hwnd, GWLP_USERDATA);

    switch (msg) {
    case WM_NCCREATE: {
        CREATESTRUCTW *create = (CREATESTRUCTW *)lParam;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (LONG_PTR)create->lpCreateParams);
        break;
    }
    case WM_SIZE:
        if (viewer != nullptr) {
            viewer->Resize((uint32_t)LOWORD(lParam), (uint32_t)HIWORD(lParam));
        }
        return 0;
    case WM_DPICHANGED: {
        // Take the size Windows suggests for the new scale; WM_SIZE follows and
        // reconfigures the surface.
        const RECT *suggested = (const RECT *)lParam;
        SetWindowPos(hwnd, nullptr, suggested->left, suggested->top, suggested->right - suggested->left, suggested->bottom - suggested->top,
                     SWP_NOZORDER | SWP_NOACTIVATE);
        return 0;
    }
    // A resize or move drag runs its own message loop, so the frame loop in
    // `main` does not get to run. A timer keeps the animation going.
    case WM_ENTERSIZEMOVE:
        SetTimer(hwnd, kModalLoopTimer, USER_TIMER_MINIMUM, nullptr);
        return 0;
    case WM_EXITSIZEMOVE:
        KillTimer(hwnd, kModalLoopTimer);
        return 0;
    case WM_TIMER:
        if (wParam == kModalLoopTimer && viewer != nullptr) {
            viewer->Tick();
        }
        return 0;
    case WM_ERASEBKGND:
        return 1; // the scene clears; letting GDI paint first only flickers
    case WM_KEYDOWN:
        if (viewer != nullptr && viewer->HandleKey(wParam)) {
            return 0;
        }
        break;
    case WM_DROPFILES:
        if (viewer != nullptr) {
            viewer->HandleDrop((HDROP)wParam);
        }
        DragFinish((HDROP)wParam);
        return 0;
    case WM_CLOSE:
        // Before DestroyWindow, not after: the window has to outlive the
        // surface built from its HWND (`ss_scene_create`).
        if (viewer != nullptr) {
            viewer->Shutdown();
        }
        DestroyWindow(hwnd);
        return 0;
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    default:
        break;
    }
    return DefWindowProcW(hwnd, msg, wParam, lParam);
}

/// argv[1], else the directory CMake baked in, else assets/overall walking up
/// from the working directory.
static std::wstring FindAssetDir(int argc, wchar_t **argv) {
    std::vector<std::wstring> candidates;
    if (argc > 1 && argv[1][0] != L'\0') {
        candidates.push_back(argv[1]);
    }
    candidates.push_back(Wide(SS_DEFAULT_ASSET_DIR));
    candidates.push_back(L"assets\\overall");
    candidates.push_back(L"..\\assets\\overall");
    candidates.push_back(L"..\\..\\assets\\overall");

    for (size_t i = 0; i < candidates.size(); i++) {
        if (IsDirectory(candidates[i])) {
            return candidates[i];
        }
    }
    return candidates[0];
}

int main() {
    PrepareConsole();
    EnablePerMonitorDpi();

    // Not `main(argc, argv)`: those arrive through the ANSI code page, which
    // cannot spell every path the file system can.
    int argc = 0;
    wchar_t **argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    if (argv == nullptr) {
        argc = 0; // fall through to the baked-in asset directory
    }
    std::wstring assetDir = FindAssetDir(argc, argv);
    std::wstring initialPack = argc > 2 ? argv[2] : L"";
    if (argv != nullptr) {
        LocalFree(argv);
    }

    HINSTANCE instance = GetModuleHandleW(nullptr);
    WNDCLASSEXW cls;
    memset(&cls, 0, sizeof(cls));
    cls.cbSize = sizeof(cls);
    cls.style = CS_HREDRAW | CS_VREDRAW;
    cls.lpfnWndProc = WindowProc;
    cls.hInstance = instance;
    cls.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    cls.lpszClassName = L"SSPlayerWgpuViewerDX12";
    if (RegisterClassExW(&cls) == 0) {
        fprintf(stderr, "RegisterClassEx failed (%lu)\n", GetLastError());
        return 1;
    }

    Viewer viewer(assetDir, initialPack);

    HWND hwnd = CreateWindowExW(WS_EX_ACCEPTFILES, cls.lpszClassName, L"SpriteStudio wgpu Viewer (DX12 / C API)", WS_OVERLAPPEDWINDOW, CW_USEDEFAULT,
                                CW_USEDEFAULT, 960, 540, nullptr, nullptr, instance, &viewer);
    if (hwnd == nullptr) {
        fprintf(stderr, "CreateWindowEx failed (%lu)\n", GetLastError());
        return 1;
    }

    // 960x540 of *client* area at this monitor's scale — the window was created
    // with that as its outer size, which is smaller and DPI-blind.
    UINT dpi = DpiForWindow(hwnd);
    RECT wanted = {0, 0, (LONG)MulDiv(960, (int)dpi, USER_DEFAULT_SCREEN_DPI), (LONG)MulDiv(540, (int)dpi, USER_DEFAULT_SCREEN_DPI)};
    AdjustWindowRectEx(&wanted, WS_OVERLAPPEDWINDOW, FALSE, WS_EX_ACCEPTFILES);
    SetWindowPos(hwnd, nullptr, 0, 0, wanted.right - wanted.left, wanted.bottom - wanted.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);

    DragAcceptFiles(hwnd, TRUE);
    ShowWindow(hwnd, SW_SHOW);

    if (!viewer.Start(hwnd)) {
        // Shutdown first: a half-built viewer may already own a scene, and the
        // window has to outlive the surface built from its HWND.
        viewer.Shutdown();
        DestroyWindow(hwnd);
        return 1;
    }

    // Present mode is Fifo (host.rs), so the render call is what paces us: no
    // sleep, no timer, one frame per vblank.
    MSG msg;
    bool running = true;
    while (running) {
        while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
            if (msg.message == WM_QUIT) {
                running = false;
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if (!running) {
            break;
        }
        if (IsIconic(hwnd)) {
            WaitMessage(); // nothing to present; do not spin
            continue;
        }
        viewer.Tick();
    }

    viewer.Shutdown();
    return 0;
}
