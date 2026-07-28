// Metal (macOS) verification viewer for the ssplayer-wgpu C API (SPEC §12 P5, §13).
//
// This is the "③ ffi" face end to end: Cocoa owns the window and its
// CAMetalLayer, hands the layer to `ss_player_create`, and drives the player
// with `ss_player_update` / `ss_player_render` — no Rust in sight. The player's
// own host (host.rs) builds the wgpu stack behind the ABI, with backend=Metal
// forced so this really exercises Metal rather than whatever wgpu would pick.
//
// It plays the embedded-mode packs from assets/overall_emb, so the atlases ride
// inside the .ssab and no texture supply is needed (SPEC §8.1).
//
//   ssviewer-metal [asset-dir] [pack-name]     e.g. ssviewer-metal "" Mask
//
//   Space       pause / resume        Left/Right  previous / next .ssab
//   R           restart animation     Up/Down     previous / next animation
//   Escape      quit
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

@implementation SSViewerApp {
    NSWindow *_window;
    SSMetalView *_view;
    ss_player_t *_player;

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

    ss_player_desc_t desc;
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

    _player = ss_player_create(&desc);
    if (_player == NULL) {
        fprintf(stderr, "ss_player_create failed: %s\n", ss_last_error());
        [NSApp terminate:nil];
        return;
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
    ss_player_destroy(_player);
    _player = NULL;
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
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
        if (ss_player_add_ssab(_player, pack.UTF8String, (const uint8_t *)data.bytes, data.length) != SS_OK) {
            fprintf(stderr, "add_ssab %s: %s\n", pack.UTF8String, ss_last_error());
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
    if (data == nil || ss_player_load_ssab(_player, pack.UTF8String, (const uint8_t *)data.bytes, data.length) != SS_OK) {
        fprintf(stderr, "load_ssab %s: %s\n", pack.UTF8String, ss_last_error());
        return;
    }

    NSMutableArray<NSString *> *anims = [NSMutableArray array];
    uint32_t count = ss_player_animation_count(_player);
    for (uint32_t i = 0; i < count; i++) {
        const char *name = ss_player_animation_name(_player, i);
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
    // Selecting is the allocating step; starting is free.
    if (ss_player_set_animation(_player, _anims[_animIndex].UTF8String) != SS_OK) {
        fprintf(stderr, "set_animation %s: %s\n", _anims[_animIndex].UTF8String, ss_last_error());
        return;
    }
    ss_player_play(_player);
    _paused = NO;
    [self printStatus];
}

// --- frame loop ----------------------------------------------------------

- (void)tick:(NSTimer *)timer {
    CFTimeInterval now = CACurrentMediaTime();
    float delta = (float)(now - _lastTick);
    _lastTick = now;

    // Always update: while paused the runtime holds the frame, so this rebuilds
    // the same geometry rather than advancing it.
    ss_player_update(_player, delta);
    if (ss_player_render(_player) != SS_OK) {
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
    ss_player_resize(_player, (uint32_t)pixels.width, (uint32_t)pixels.height);
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
        if (_paused) {
            ss_player_pause(_player);
        } else {
            ss_player_play(_player);
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

    if ([event.charactersIgnoringModifiers caseInsensitiveCompare:@"r"] == NSOrderedSame) {
        [self playAnimationAtIndex:_animIndex];
        return YES;
    }
    return NO;
}

- (void)printStatus {
    float canvasWidth = 0.0f, canvasHeight = 0.0f;
    ss_player_canvas_size(_player, &canvasWidth, &canvasHeight);
    printf("\033[2J\033[H");
    printf("====================================================\n");
    printf("   SpriteStudio 7 wgpu Viewer — Metal / C API\n");
    printf("====================================================\n");
    printf("  Active File:   %s (%lu/%lu)\n", _packs[_packIndex].lastPathComponent.UTF8String, (unsigned long)_packIndex + 1, (unsigned long)_packs.count);
    printf("  Animation:     %s (%lu/%lu)\n", _anims.count ? _anims[_animIndex].UTF8String : "none", (unsigned long)_animIndex + 1, (unsigned long)_anims.count);
    printf("  Canvas:        %.0f x %.0f\n", canvasWidth, canvasHeight);
    printf("  Playback:      %s\n", _paused ? "Paused" : "Playing");
    printf("----------------------------------------------------\n");
    printf("  Space  pause    Left/Right  .ssab    Up/Down  anim\n");
    printf("  R      restart  Escape      quit\n");
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
