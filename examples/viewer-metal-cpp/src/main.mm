// Metal (macOS) verification viewer for the ssplayer-wgpu C API.
//
// This is the "③ ffi" face end to end: Cocoa owns the window and its
// CAMetalLayer, hands the layer to `ss_scene_create`, and drives the scene with
// `ss_scene_update` / `ss_scene_render` — no Rust in sight. The scene's own host
// (host.rs) builds the wgpu stack behind the ABI, with backend=Metal forced so
// this really exercises Metal rather than whatever wgpu would pick.
//
// It starts on the reference-mode packs from assets/overall, matching the Rust
// viewer; the embedded set is in assets/overall_emb and can be passed as argv[1].
//
// **Drop a .ssab (or a folder of them) on the window to play it**, whichever way
// it was exported. `ss_player_load(player, path)` is the whole of loading — the
// pack, its atlases in either export mode, and the sibling .ssab files its
// instance parts play from, all resolved against the directory the drop came
// from. This viewer used to walk `ss_player_texture_at` and
// call `ss_scene_add_texture_png` per image; that loop is what the one call
// replaced. Those entry points remain for a host whose assets are not files —
// an archive, a platform asset manager.
//
// **It shows two characters at once by default**. That
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
//   drop        play a .ssab / add every .ssab in a folder
//
// The other C++ viewers (DX12/Win32, Vulkan/Linux) deliberately duplicate this
// loop instead of sharing it: the point of the sample is that one directory is
// self-contained.

#import <Cocoa/Cocoa.h>
#import <QuartzCore/CAMetalLayer.h>

#include "ssplayer_wgpu.h"

/// Where scripts/deploy-examples.sh puts the embedded demo packs. Baked in by
/// CMake so the viewer runs from any working directory; argv[1] overrides it.
#ifndef SS_DEFAULT_ASSET_DIR
#define SS_DEFAULT_ASSET_DIR "assets/overall"
#endif

/// Collect the `.ssab` files behind a drop: the files themselves, plus the ones
/// directly inside any dropped folder. Sorted, so a folder plays in the order
/// the Left/Right keys will walk it.
static NSArray<NSString *> *SSCollectSsabPaths(NSArray<NSURL *> *urls) {
    NSFileManager *fm = NSFileManager.defaultManager;
    NSMutableArray<NSString *> *paths = [NSMutableArray array];
    for (NSURL *url in urls) {
        NSString *path = url.path;
        BOOL isDir = NO;
        if (![fm fileExistsAtPath:path isDirectory:&isDir]) {
            continue;
        }
        if (!isDir) {
            if ([path.pathExtension caseInsensitiveCompare:@"ssab"] == NSOrderedSame) {
                [paths addObject:path];
            }
            continue;
        }
        for (NSString *entry in [[fm contentsOfDirectoryAtPath:path error:nil] sortedArrayUsingSelector:@selector(compare:)]) {
            if ([entry.pathExtension caseInsensitiveCompare:@"ssab"] == NSOrderedSame) {
                [paths addObject:[path stringByAppendingPathComponent:entry]];
            }
        }
    }
    return paths;
}

/// What the view hands a drop to.
@protocol SSFileDropTarget <NSObject>
- (BOOL)playDroppedPaths:(NSArray<NSString *> *)paths;
@end

// A layer-backed view whose backing layer is the CAMetalLayer we hand to the
// player. AppKit calls makeBackingLayer once `wantsLayer` is set.
//
// It is also the window's drop target: dragging a .ssab onto the animation is
// the shortest path from "the converter just wrote this" to seeing it move.
@interface SSMetalView : NSView
@property(weak) id<SSFileDropTarget> dropTarget;
@end

@implementation SSMetalView
- (CALayer *)makeBackingLayer {
    return [CAMetalLayer layer];
}
- (BOOL)wantsUpdateLayer {
    return YES;
}

- (NSArray<NSString *> *)ssabPathsIn:(id<NSDraggingInfo>)drag {
    NSArray<NSURL *> *urls = [drag.draggingPasteboard readObjectsForClasses:@[ NSURL.class ] options:@{NSPasteboardURLReadingFileURLsOnlyKey : @YES}];
    return SSCollectSsabPaths(urls ?: @[]);
}

- (NSDragOperation)draggingEntered:(id<NSDraggingInfo>)drag {
    return [self ssabPathsIn:drag].count > 0 ? NSDragOperationCopy : NSDragOperationNone;
}

- (BOOL)prepareForDragOperation:(id<NSDraggingInfo>)drag {
    return [self ssabPathsIn:drag].count > 0;
}

- (BOOL)performDragOperation:(id<NSDraggingInfo>)drag {
    return [self.dropTarget playDroppedPaths:[self ssabPathsIn:drag]];
}
@end

@interface SSViewerApp : NSObject <NSApplicationDelegate, NSWindowDelegate, SSFileDropTarget>
@end

/// How many characters to show at startup. Two is the point of the exercise.
static const NSUInteger kInitialPlayers = 2;
/// Most players the +/- keys will go to.
static const NSUInteger kMaxPlayers = 6;

@implementation SSViewerApp {
    NSWindow *_window;
    SSMetalView *_view;
    ss_scene_t *_scene;
    /// The scene's size in backing pixels, which is also what placement is
    /// measured in — the projection maps one unit to one target pixel. Kept
    /// here because the lay-out divides the drawable between the cast.
    uint32_t _width;
    uint32_t _height;
    /// One handle per character. All of them draw into `_scene`.
    NSMutableArray<NSValue *> *_players;

    NSString *_assetDir;
    NSString *_initialPack; // pack to show first, or nil for Basic

    NSMutableArray<NSString *> *_packs; // .ssab paths: the asset dir, then whatever was dropped
    /// Cellmap hashes already handed to the scene, so a second pack sharing an
    /// atlas (most of the demo set shares `common.png`) does not re-read it.
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
    _window.title = @"SpriteStudio wgpu Viewer (Metal / C API)";
    _window.delegate = self;

    _view = [[SSMetalView alloc] initWithFrame:frame];
    _view.wantsLayer = YES;
    _view.dropTarget = self;
    [_view registerForDraggedTypes:@[ NSPasteboardTypeFileURL ]];
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
    // The first `addPlayer` lays the cast out, so the size has to be known by
    // then rather than only from the first resize notification.
    _width = desc.width;
    _height = desc.height;
    desc.clear_color[0] = 0.08f;
    desc.clear_color[1] = 0.08f;
    desc.clear_color[2] = 0.12f;
    desc.clear_color[3] = 1.0f;
    desc.enable_mask = true; // stencil buffer, so Mask.ssab renders
    desc.y_down = false;

    _scene = ss_scene_create(&desc);
    if (_scene == NULL) {
        fprintf(stderr, "ss_scene_create failed: %s\n", ss_last_error());
        [NSApp terminate:nil];
        return;
    }

    _packs = [NSMutableArray array];
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
    // touched afterwards.
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
    // Nothing to register: the caller follows with `loadPackAtIndex`, and that
    // one call brings the pack, its atlases and the packs its instance parts
    // play from.
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

/// Give each character an equal slice of the window, fit it to that slice, and
/// stack the cast front to back.
///
/// Placement is per player; the projection belongs to the scene, and
/// it maps one unit to one target pixel, so the drawable's own size is the right
/// thing to divide. Fitting rather than a fixed scale is what the demo packs
/// need: they disagree about canvas size, so one constant either crowds two
/// characters into each other or runs a tall pack off the top. This is the Rust
/// viewer's Row layout, so the two faces can be read side by side.
- (void)layOutPlayers {
    NSUInteger count = _players.count;
    if (count == 0 || _width == 0 || _height == 0) {
        return; // zero-sized drawable, or nothing to place
    }
    float slot = (float)_width / (float)count;
    float last = (float)(count - 1);
    SsPlacement placement;
    ss_placement_default(&placement);
    placement.scale = 0.9f;
    for (NSUInteger i = 0; i < count; i++) {
        ss_player_t *player = [self playerAt:i];
        // The pack's authored canvas, placed by its pivot, contained in this
        // member's column at 90%; the offset moves the column's centre off the
        // view's. A player with no animation yet lands centred and unscaled.
        placement.offset_x = ((float)i - last * 0.5f) * slot;
        ss_player_fit_into(player, slot, (float)_height, &placement, NULL);
        ss_player_set_order(player, (int32_t)i);
    }
}

// --- assets --------------------------------------------------------------

/// Whether `path` is worth putting in the list the Left/Right keys walk.
///
/// Only a readability check now: loading is `loadPackAtIndex`'s job, and it takes
/// the path rather than bytes, so nothing needs registering up front.
- (BOOL)adoptPackAtPath:(NSString *)path {
    if (![path.pathExtension isEqualToString:@"ssab"] || ![[NSFileManager defaultManager] isReadableFileAtPath:path]) {
        fprintf(stderr, "not a readable .ssab: %s\n", path.UTF8String);
        return NO;
    }
    return YES;
}

/// Gather every .ssab in the asset directory and register all of them up front.
- (BOOL)collectPacks {
    NSFileManager *fm = NSFileManager.defaultManager;
    NSArray<NSString *> *entries = [fm contentsOfDirectoryAtPath:_assetDir error:nil];
    if (entries == nil) {
        fprintf(stderr, "asset directory not found: %s\n  run scripts/deploy-examples.sh, or pass a directory as argv[1]\n", _assetDir.UTF8String);
        return NO;
    }

    for (NSString *entry in [entries sortedArrayUsingSelector:@selector(compare:)]) {
        if ([entry.pathExtension isEqualToString:@"ssab"]) {
            [_packs addObject:[_assetDir stringByAppendingPathComponent:entry]];
        }
    }
    if (_packs.count == 0) {
        fprintf(stderr, "no .ssab files in %s\n", _assetDir.UTF8String);
        return NO;
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

/// Play what was dropped on the window (`SSFileDropTarget`).
///
/// Dropped packs join the list the Left/Right keys walk rather than replacing
/// it, so a character dropped in can still be compared against the demo set —
/// and a folder dropped in brings its instance-referenced packs along, which is
/// what a pack with external references needs to resolve.
- (BOOL)playDroppedPaths:(NSArray<NSString *> *)paths {
    NSUInteger first = NSNotFound;
    for (NSString *path in paths) {
        if (![self adoptPackAtPath:path]) {
            continue;
        }
        NSUInteger existing = [_packs indexOfObject:path];
        if (existing == NSNotFound) {
            existing = _packs.count;
            [_packs addObject:path];
        }
        if (first == NSNotFound) {
            first = existing;
        }
    }
    if (first == NSNotFound) {
        return NO;
    }
    [self loadPackAtIndex:first];
    return YES;
}

/// Play one of the listed packs on every player.
///
/// `ss_player_load` is the whole of it: the pack, its atlases in either export
/// mode, and the sibling .ssab files its instance parts play from. This viewer
/// used to walk `ss_player_texture_at` and call `ss_scene_add_texture_png` per
/// image, and that loop is what the call replaced.
- (void)loadPackAtIndex:(NSUInteger)index {
    NSString *path = _packs[index];
    for (NSValue *entry in _players) {
        if (ss_player_load((ss_player_t *)entry.pointerValue, path.UTF8String) != SS_OK) {
            fprintf(stderr, "load %s: %s\n", path.UTF8String, ss_last_error());
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
    // Packs differ in canvas size, and a player only has one once it has an
    // animation, so the fit is recomputed here rather than at load.
    [self layOutPlayers];
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
    // they share a scene, so they share the pass.
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
    // Placement is in projection units, which are the target's pixels, so the
    // cast has to be re-fitted to the new drawable rather than left where it was.
    _width = (uint32_t)pixels.width;
    _height = (uint32_t)pixels.height;
    [self layOutPlayers];
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
    // and its atlas are already there.
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
    printf("   SpriteStudio wgpu Viewer — Metal / C API\n");
    printf("====================================================\n");
    printf("  Active File:   %s (%lu/%lu)\n", _packs[_packIndex].lastPathComponent.UTF8String, (unsigned long)_packIndex + 1, (unsigned long)_packs.count);
    printf("  Animation:     %s (%lu/%lu)\n", _anims.count ? _anims[_animIndex].UTF8String : "none", (unsigned long)_animIndex + 1, (unsigned long)_anims.count);
    printf("  Canvas:        %.0f x %.0f\n", canvasWidth, canvasHeight);
    printf("  Players:       %u in one scene\n", ss_scene_player_count(_scene));
    printf("  Playback:      %s\n", _paused ? "Paused" : "Playing");
    // What the frame cost. GPU time is 0 when the adapter
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
    printf("  Drop a .ssab or a folder on the window to play it\n");
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

/// argv[1], else the directory CMake baked in, else assets/overall walking
/// up from the working directory.
static NSString *FindAssetDir(int argc, const char *argv[]) {
    NSFileManager *fm = NSFileManager.defaultManager;
    NSMutableArray<NSString *> *candidates = [NSMutableArray array];
    if (argc > 1) {
        [candidates addObject:@(argv[1])];
    }
    [candidates addObject:@SS_DEFAULT_ASSET_DIR];
    for (NSString *prefix in @[ @"", @"../", @"../../" ]) {
        [candidates addObject:[prefix stringByAppendingString:@"assets/overall"]];
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
