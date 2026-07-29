// Metal (macOS) verification viewer for the ssplayer-wgpu C API (SPEC §12 P5, §13).
//
// This is the "③ ffi" face end to end: Cocoa owns the window and its
// CAMetalLayer, hands the layer to `ss_scene_create`, and drives the scene with
// `ss_scene_update` / `ss_scene_render` — no Rust in sight. The scene's own host
// (host.rs) builds the wgpu stack behind the ABI, with backend=Metal forced so
// this really exercises Metal rather than whatever wgpu would pick.
//
// It plays the embedded-mode packs from assets/overall_emb, so the atlases ride
// inside the .ssab and no texture supply is needed (SPEC §8.1).
//
// **It shows two characters at once by default** (SPEC §11, phase P7-7). That
// used to be impossible through this API: the handle fused the window and the
// player, so a second character meant a second surface on the same window and
// the two cleared over each other. Now the window is `ss_scene_t` and each
// character is an `ss_player_t` placed with `ss_player_set_transform`, sharing
// one device, one renderer and one atlas upload.
//
//   ssviewer-metal [asset-dir] [pack-name]     e.g. ssviewer-metal "" Mask
//
//   Space       pause / resume        Left/Right  previous / next .ssab
//   R           restart animation     Up/Down     previous / next animation
//   +/-         add / remove player   Escape      quit
//
// The other C++ viewers (DX12/Win32, Vulkan/Linux) deliberately duplicate this
// loop instead of sharing it: the point of the sample is that one directory is
// self-contained (SPEC §2).

#import <Cocoa/Cocoa.h>
#import <QuartzCore/CAMetalLayer.h>

#include "ssplayer_wgpu.h"

/// Where scripts/deploy-examples.sh puts the embedded demo packs. Baked in by
/// CMake so the viewer runs from any working directory; argv[1] overrides it.
#ifndef SS_DEFAULT_ASSET_DIR
#define SS_DEFAULT_ASSET_DIR "assets/overall_emb"
#endif

// A layer-backed view whose backing layer is the CAMetalLayer we hand to the
// player. AppKit calls makeBackingLayer once `wantsLayer` is set.
@interface SSMetalView : NSView
@end

@implementation SSMetalView
- (CALayer *)makeBackingLayer {
    return [CAMetalLayer layer];
}
- (BOOL)wantsUpdateLayer {
    return YES;
}
@end

@interface SSViewerApp : NSObject <NSApplicationDelegate, NSWindowDelegate>
@end

/// How many characters to show at startup. Two is the point of the exercise.
static const NSUInteger kInitialPlayers = 2;
/// Most players the +/- keys will go to.
static const NSUInteger kMaxPlayers = 6;

@implementation SSViewerApp {
    NSWindow *_window;
    SSMetalView *_view;
    ss_scene_t *_scene;
    /// One handle per character. All of them draw into `_scene`.
    NSMutableArray<NSValue *> *_players;

    NSString *_assetDir;
    NSString *_initialPack; // pack to show first, or nil for Basic

    NSArray<NSString *> *_packs; // .ssab paths, sorted
    NSUInteger _packIndex;
    NSArray<NSString *> *_anims; // animation names of the current pack
    NSUInteger _animIndex;

    BOOL _paused;
    CFTimeInterval _lastTick;
}

// --- setup ---------------------------------------------------------------

- (instancetype)initWithAssetDir:(NSString *)assetDir initialPack:(NSString *)pack {
    if ((self = [super init])) {
        _assetDir = assetDir;
        _initialPack = pack;
    }
    return self;
}

- (void)applicationDidFinishLaunching:(NSNotification *)note {
    NSRect frame = NSMakeRect(0, 0, 960, 540);
    _window = [[NSWindow alloc] initWithContentRect:frame
                                          styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable
                                            backing:NSBackingStoreBuffered
                                              defer:NO];
    _window.title = @"SpriteStudio 7 wgpu Viewer (Metal / C API)";
    _window.delegate = self;

    _view = [[SSMetalView alloc] initWithFrame:frame];
    _view.wantsLayer = YES;
    _window.contentView = _view;
    [_window center];
    [_window makeKeyAndOrderFront:nil];

    CAMetalLayer *layer = (CAMetalLayer *)_view.layer;
    layer.contentsScale = _window.backingScaleFactor;

    // The player wants pixels, not points.
    NSSize pixels = [_view convertSizeToBacking:_view.bounds.size];

    ss_scene_desc_t desc;
    memset(&desc, 0, sizeof(desc));
    desc.window.kind = SS_WINDOW_METAL_LAYER;
    desc.window.window = (__bridge void *)layer;
    desc.backend = SS_BACKEND_METAL;
    desc.width = (uint32_t)pixels.width;
    desc.height = (uint32_t)pixels.height;
    desc.clear_color[0] = 0.08f;
    desc.clear_color[1] = 0.08f;
    desc.clear_color[2] = 0.12f;
    desc.clear_color[3] = 1.0f;
    desc.enable_mask = true; // stencil buffer, so Mask.ssab renders (SPEC §6)
    desc.y_down = false;

    _scene = ss_scene_create(&desc);
    if (_scene == NULL) {
        fprintf(stderr, "ss_scene_create failed: %s\n", ss_last_error());
        [NSApp terminate:nil];
        return;
    }

    _players = [NSMutableArray array];
    for (NSUInteger i = 0; i < kInitialPlayers; i++) {
        [self addPlayer];
    }

    if (![self collectPacks]) {
        [NSApp terminate:nil];
        return;
    }
    [self loadPackAtIndex:_packIndex];

    // Common run loop modes keep the animation running while the window is
    // being resized or a menu is tracking.
    _lastTick = CACurrentMediaTime();
    NSTimer *timer = [NSTimer timerWithTimeInterval:1.0 / 60.0 target:self selector:@selector(tick:) userInfo:nil repeats:YES];
    [[NSRunLoop currentRunLoop] addTimer:timer forMode:NSRunLoopCommonModes];

    [self installKeyHandler];
}

- (void)applicationWillTerminate:(NSNotification *)note {
    // Destroying the scene takes its players with it; the handles must not be
    // touched afterwards (SPEC §11).
    ss_scene_destroy(_scene);
    _scene = NULL;
    [_players removeAllObjects];
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
}

// --- players -------------------------------------------------------------

- (ss_player_t *)playerAt:(NSUInteger)index {
    return (ss_player_t *)_players[index].pointerValue;
}

/// Add one character to the scene and lay the row out again.
- (void)addPlayer {
    ss_player_t *player = ss_scene_create_player(_scene);
    if (player == NULL) {
        fprintf(stderr, "ss_scene_create_player: %s\n", ss_last_error());
        return;
    }
    [_players addObject:[NSValue valueWithPointer:player]];
    [self layOutPlayers];
}

- (void)removePlayer {
    if (_players.count <= 1) {
        return; // keep at least one, or there is nothing to look at
    }
    ss_player_destroy([self playerAt:_players.count - 1]);
    [_players removeLastObject];
    [self layOutPlayers];
}

/// Spread the characters across the window and stack them front to back.
/// Placement is per player (SPEC §5.4); the projection belongs to the scene.
- (void)layOutPlayers {
    NSUInteger count = _players.count;
    float spacing = count > 1 ? 220.0f : 0.0f;
    float first = -spacing * (float)(count - 1) * 0.5f;
    for (NSUInteger i = 0; i < count; i++) {
        ss_player_t *player = [self playerAt:i];
        ss_player_set_transform(player, first + spacing * (float)i, 0.0f, 0.85f, 0.85f, 0.0f);
        ss_player_set_order(player, (int32_t)i);
    }
}

// --- assets --------------------------------------------------------------

/// Gather every .ssab in the asset directory. All of them are registered with
/// the player up front so external instance parts, which resolve by pack name,
/// can find their source animation (SPEC §7.1).
- (BOOL)collectPacks {
    NSFileManager *fm = NSFileManager.defaultManager;
    NSArray<NSString *> *entries = [fm contentsOfDirectoryAtPath:_assetDir error:nil];
    if (entries == nil) {
        fprintf(stderr, "asset directory not found: %s\n  run scripts/deploy-examples.sh, or pass a directory as argv[1]\n", _assetDir.UTF8String);
        return NO;
    }

    NSMutableArray<NSString *> *packs = [NSMutableArray array];
    for (NSString *entry in [entries sortedArrayUsingSelector:@selector(compare:)]) {
        if ([entry.pathExtension isEqualToString:@"ssab"]) {
            [packs addObject:[_assetDir stringByAppendingPathComponent:entry]];
        }
    }
    if (packs.count == 0) {
        fprintf(stderr, "no .ssab files in %s\n", _assetDir.UTF8String);
        return NO;
    }
    _packs = packs;

    for (NSString *path in _packs) {
        NSData *data = [NSData dataWithContentsOfFile:path];
        if (data == nil) {
            continue;
        }
        NSString *pack = path.lastPathComponent.stringByDeletingPathExtension;
        // Every player needs every pack registered: instance parts resolve their
        // sub-animation by pack name, per player (SPEC §7.1).
        for (NSValue *entry in _players) {
            if (ss_player_add_ssab((ss_player_t *)entry.pointerValue, pack.UTF8String, (const uint8_t *)data.bytes, data.length) != SS_OK) {
                fprintf(stderr, "add_ssab %s: %s\n", pack.UTF8String, ss_last_error());
            }
        }
    }

    // Show the pack named on the command line, else Basic.ssab like the Rust viewer.
    NSString *wanted = [(_initialPack ?: @"Basic") stringByAppendingPathExtension:@"ssab"];
    _packIndex = 0;
    for (NSUInteger i = 0; i < _packs.count; i++) {
        if ([_packs[i].lastPathComponent caseInsensitiveCompare:wanted] == NSOrderedSame) {
            _packIndex = i;
            break;
        }
    }
    return YES;
}

/// Make one registered pack the main pack and play its first animation.
- (void)loadPackAtIndex:(NSUInteger)index {
    NSString *path = _packs[index];
    NSData *data = [NSData dataWithContentsOfFile:path];
    NSString *pack = path.lastPathComponent.stringByDeletingPathExtension;
    if (data == nil) {
        fprintf(stderr, "cannot read %s\n", path.UTF8String);
        return;
    }
    for (NSValue *entry in _players) {
        if (ss_player_load_ssab((ss_player_t *)entry.pointerValue, pack.UTF8String, (const uint8_t *)data.bytes, data.length) != SS_OK) {
            fprintf(stderr, "load_ssab %s: %s\n", pack.UTF8String, ss_last_error());
            return;
        }
    }

    NSMutableArray<NSString *> *anims = [NSMutableArray array];
    uint32_t count = ss_player_animation_count([self playerAt:0]);
    for (uint32_t i = 0; i < count; i++) {
        const char *name = ss_player_animation_name([self playerAt:0], i);
        // The pointer is only valid until the next call, so copy it now.
        if (name != NULL) {
            [anims addObject:@(name)];
        }
    }
    _anims = anims;
    _packIndex = index;
    [self playAnimationAtIndex:0];
}

- (void)playAnimationAtIndex:(NSUInteger)index {
    if (_anims.count == 0) {
        return;
    }
    _animIndex = index % _anims.count;
    for (NSValue *entry in _players) {
        ss_player_t *player = (ss_player_t *)entry.pointerValue;
        // Selecting is the allocating step; starting is free.
        if (ss_player_set_animation(player, _anims[_animIndex].UTF8String) != SS_OK) {
            fprintf(stderr, "set_animation %s: %s\n", _anims[_animIndex].UTF8String, ss_last_error());
            return;
        }
        ss_player_play(player);
    }
    _paused = NO;
    [self printStatus];
}

// --- frame loop ----------------------------------------------------------

- (void)tick:(NSTimer *)timer {
    CFTimeInterval now = CACurrentMediaTime();
    float delta = (float)(now - _lastTick);
    _lastTick = now;

    // One update and one render for the whole window, whatever the player count:
    // they share a scene, so they share the pass (SPEC §9.2).
    ss_scene_update(_scene, delta);
    if (ss_scene_render(_scene) != SS_OK) {
        fprintf(stderr, "render: %s\n", ss_last_error());
    }
}

- (void)windowDidResize:(NSNotification *)note {
    [self syncDrawableSize];
}

- (void)windowDidChangeBackingProperties:(NSNotification *)note {
    ((CAMetalLayer *)_view.layer).contentsScale = _window.backingScaleFactor;
    [self syncDrawableSize];
}

- (void)syncDrawableSize {
    NSSize pixels = [_view convertSizeToBacking:_view.bounds.size];
    ss_scene_resize(_scene, (uint32_t)pixels.width, (uint32_t)pixels.height);
}

// --- input ---------------------------------------------------------------

- (void)installKeyHandler {
    [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskKeyDown
                                          handler:^NSEvent *(NSEvent *event) {
                                              return [self handleKey:event] ? nil : event;
                                          }];
}

- (BOOL)handleKey:(NSEvent *)event {
    enum { kEscape = 53, kSpace = 49, kLeft = 123, kRight = 124, kDown = 125, kUp = 126 };

    switch (event.keyCode) {
    case kEscape:
        [NSApp terminate:nil];
        return YES;
    case kSpace:
        _paused = !_paused;
        for (NSValue *entry in _players) {
            ss_player_t *player = (ss_player_t *)entry.pointerValue;
            if (_paused) {
                ss_player_pause(player);
            } else {
                ss_player_play(player);
            }
        }
        [self printStatus];
        return YES;
    case kRight:
        [self loadPackAtIndex:(_packIndex + 1) % _packs.count];
        return YES;
    case kLeft:
        [self loadPackAtIndex:(_packIndex + _packs.count - 1) % _packs.count];
        return YES;
    case kDown:
        [self playAnimationAtIndex:_animIndex + 1];
        return YES;
    case kUp:
        [self playAnimationAtIndex:_animIndex + _anims.count - 1];
        return YES;
    default:
        break;
    }

    NSString *chars = event.charactersIgnoringModifiers;
    if ([chars caseInsensitiveCompare:@"r"] == NSOrderedSame) {
        [self playAnimationAtIndex:_animIndex];
        return YES;
    }
    // Adding a character costs a playback tree and nothing on the GPU: the pack
    // and its atlas are already there (SPEC §1.2 / §8.2).
    if ([chars isEqualToString:@"+"] || [chars isEqualToString:@"="]) {
        if (_players.count < kMaxPlayers) {
            [self addPlayer];
            [self loadPackAtIndex:_packIndex];
        }
        return YES;
    }
    if ([chars isEqualToString:@"-"]) {
        [self removePlayer];
        [self printStatus];
        return YES;
    }
    return NO;
}

- (void)printStatus {
    float canvasWidth = 0.0f, canvasHeight = 0.0f;
    ss_player_canvas_size([self playerAt:0], &canvasWidth, &canvasHeight);
    printf("\033[2J\033[H");
    printf("====================================================\n");
    printf("   SpriteStudio 7 wgpu Viewer — Metal / C API\n");
    printf("====================================================\n");
    printf("  Active File:   %s (%lu/%lu)\n", _packs[_packIndex].lastPathComponent.UTF8String, (unsigned long)_packIndex + 1, (unsigned long)_packs.count);
    printf("  Animation:     %s (%lu/%lu)\n", _anims.count ? _anims[_animIndex].UTF8String : "none", (unsigned long)_animIndex + 1, (unsigned long)_anims.count);
    printf("  Canvas:        %.0f x %.0f\n", canvasWidth, canvasHeight);
    printf("  Players:       %u in one scene\n", ss_scene_player_count(_scene));
    printf("  Playback:      %s\n", _paused ? "Paused" : "Playing");
    // What the frame cost (SPEC §14.2 item 10). GPU time is 0 when the adapter
    // has no timestamp query, and lags a frame or two when it does.
    ss_scene_stats_t stats;
    if (ss_scene_stats(_scene, &stats) == SS_OK) {
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

@end

/// Minimal menu bar so Cmd+Q works when run straight from a terminal.
static void InstallMenuBar(void) {
    NSMenu *menuBar = [[NSMenu alloc] init];
    NSMenuItem *appItem = [[NSMenuItem alloc] init];
    [menuBar addItem:appItem];
    NSMenu *appMenu = [[NSMenu alloc] init];
    [appMenu addItemWithTitle:@"Quit" action:@selector(terminate:) keyEquivalent:@"q"];
    appItem.submenu = appMenu;
    NSApp.mainMenu = menuBar;
}

/// argv[1], else the directory CMake baked in, else assets/overall_emb walking
/// up from the working directory.
static NSString *FindAssetDir(int argc, const char *argv[]) {
    NSFileManager *fm = NSFileManager.defaultManager;
    NSMutableArray<NSString *> *candidates = [NSMutableArray array];
    if (argc > 1) {
        [candidates addObject:@(argv[1])];
    }
    [candidates addObject:@SS_DEFAULT_ASSET_DIR];
    for (NSString *prefix in @[ @"", @"../", @"../../" ]) {
        [candidates addObject:[prefix stringByAppendingString:@"assets/overall_emb"]];
    }

    BOOL isDir = NO;
    for (NSString *candidate in candidates) {
        if ([fm fileExistsAtPath:candidate isDirectory:&isDir] && isDir) {
            return candidate;
        }
    }
    return candidates.firstObject;
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSApplication *app = NSApplication.sharedApplication;
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        InstallMenuBar();

        NSString *initialPack = argc > 2 ? @(argv[2]) : nil;
        SSViewerApp *delegate = [[SSViewerApp alloc] initWithAssetDir:FindAssetDir(argc, argv) initialPack:initialPack];
        app.delegate = delegate;
        [app activateIgnoringOtherApps:YES];
        [app run];
    }
    return 0;
}
