// Loaded only by the two CI fixture processes. Query AppKit on their main
// threads so the comparison also works without system-wide Accessibility access.
#import <AppKit/AppKit.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <Vision/Vision.h>
#include <math.h>
#include <stdlib.h>

static NSWindow *dashboardWindow(void) {
    for (NSWindow *window in NSApp.windows)
        if (window.isVisible && !window.sheetParent && [window.title isEqual:@"QuotaBar"]) return window;
    return nil;
}

// Both helpers use this identical WindowServer capture path. Only the current
// process can supply windows, and the ID must match the selected AppKit window.
typedef NS_ENUM(NSInteger, CaptureMode) { DashboardWindow, PopoverOnDisplay, PopoverWindow };

// A missing process surface may be returned as opaque black. Require real
// foreground detail in the header and the button row of the same popover.
static BOOL hasPopoverDetails(NSBitmapImageRep *rep, CGFloat scale) {
    NSInteger step = MAX(1, lround(2 * scale)), inset = lround(12 * scale);
    NSInteger band = MIN(rep.pixelsHigh / 2, lround(44 * scale));
    for (int end = 0; end < 2; end++) {
        CGFloat low = 1, high = 0;
        for (NSInteger y = inset; y < band; y += step) {
            NSInteger row = end ? rep.pixelsHigh - y - 1 : y;
            for (NSInteger x = inset; x < rep.pixelsWide - inset; x += step) {
                NSColor *color = [[rep colorAtX:x y:row] colorUsingColorSpace:NSColorSpace.sRGBColorSpace];
                if (color.alphaComponent < 0.9) continue;
                CGFloat value = (color.redComponent + color.greenComponent + color.blueComponent) / 3;
                low = MIN(low, value);
                high = MAX(high, value);
            }
        }
        if (high - low < 0.15) return NO;
    }
    return YES;
}

static void captureRegion(NSWindow *root, NSRect target, CaptureMode mode, NSURL *output, void (^done)(NSDictionary *)) {
    if (!root) { done(@{@"pending": @"window"}); return; }
    CGWindowID number = (CGWindowID)root.windowNumber;
    CGFloat scale = root.backingScaleFactor;
    NSRect frame = root.frame;
    BOOL includeSheet = root.attachedSheet != nil;
    CGRect region = CGRectMake(round((NSMinX(target)-NSMinX(frame))*scale), round((NSMaxY(frame)-NSMaxY(target))*scale),
                               round(NSWidth(target)*scale), round(NSHeight(target)*scale));
    NSScreen *screen = root.screen ?: NSScreen.mainScreen;
    CGDirectDisplayID displayID = [screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue];
    [SCShareableContent getCurrentProcessShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *error) {
        if (error) { done(@{@"error": error.localizedDescription}); return; }
        for (SCWindow *item in content.windows) {
            if (item.windowID != number) continue;
            CGRect crop = region;
            SCContentFilter *filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:item];
            SCStreamConfiguration *config = [SCStreamConfiguration new];
            config.width = lround(item.frame.size.width * scale);
            config.height = lround(item.frame.size.height * scale);
            config.showsCursor = NO;
            config.ignoreShadowsSingleWindow = YES;
            config.includeChildWindows = includeSheet;
            config.colorSpaceName = kCGColorSpaceSRGB;
            if (mode == PopoverWindow) {
                config.ignoreGlobalClipSingleWindow = YES;
            } else if (mode == PopoverOnDisplay) {
                // Menu extras can be hosted remotely. The process-local NSWindow
                // origin is then unrelated to its actual WindowServer origin.
                SCDisplay *matched = nil;
                for (SCDisplay *display in content.displays) {
                    if (display.displayID == displayID) { matched = display; break; }
                }
                if (!matched) { done(@{@"error": @"popover display not available"}); return; }
                CGRect onScreen = CGRectMake(item.frame.origin.x + region.origin.x/scale,
                                             item.frame.origin.y + region.origin.y/scale,
                                             NSWidth(target), NSHeight(target));
                if (!CGRectContainsRect(matched.frame, onScreen)) {
                    done(@{@"error": @"popover content outside display", @"actual": NSStringFromRect(onScreen),
                           @"display": NSStringFromRect(matched.frame)}); return;
                }
                filter = [[SCContentFilter alloc] initWithDisplay:matched includingWindows:@[item]];
                config.sourceRect = CGRectOffset(onScreen, -matched.frame.origin.x, -matched.frame.origin.y);
                config.width = lround(region.size.width);
                config.height = lround(region.size.height);
                config.backgroundColor = CGColorGetConstantColor(kCGColorClear);
                crop.origin = CGPointZero;
            }
            [SCScreenshotManager captureImageWithFilter:filter configuration:config completionHandler:^(CGImageRef frame, NSError *failed) {
                if (!frame) { done(@{@"error": failed.localizedDescription ?: @"no window frame"}); return; }
                // WindowServer composites a sheet with its parent. Capture the full
                // owner at native scale, then extract the sheet at its AppKit bounds.
                CGRect extent = CGRectMake(0, 0, CGImageGetWidth(frame), CGImageGetHeight(frame));
                if (!CGRectContainsRect(extent, crop)) { done(@{@"error": @"sheet outside captured owner"}); return; }
                CGImageRef visible = CGImageCreateWithImageInRect(frame, crop);
                if (!visible) { done(@{@"error": @"could not extract window region"}); return; }
                NSBitmapImageRep *rep = [[NSBitmapImageRep alloc] initWithCGImage:visible];
                CGImageRelease(visible);
                NSInteger w = rep.pixelsWide, h = rep.pixelsHigh;
                // The corners may be rounded. The middle of every edge must
                // contain the window, including the rightmost buttons.
                NSPoint edges[] = {{2, h/2}, {w-3, h/2}, {w/2, 2}, {w/2, h-3}};
                for (size_t i = 0; i < sizeof(edges)/sizeof(edges[0]); i++) {
                    NSColor *sample = [rep colorAtX:edges[i].x y:edges[i].y];
                    if (!sample || sample.alphaComponent < 0.9) {
                        done(@{@"error": @"window capture has a clipped edge", @"view": NSStringFromRect(target),
                               @"region": NSStringFromRect(config.sourceRect), @"window": NSStringFromRect(item.frame),
                               @"edge": NSStringFromPoint(edges[i])});
                        return;
                    }
                }
                for (int x = 1; x <= 3; ++x) for (int y = 1; y <= 3; ++y) {
                    NSColor *sample = [rep colorAtX:x*rep.pixelsWide/4 y:y*rep.pixelsHigh/4];
                    if (!sample || sample.alphaComponent < 0.9) {
                        done(@{@"error": @"window capture contains transparent interior",
                               @"window": NSStringFromRect(item.frame), @"filter": NSStringFromRect(filter.contentRect),
                               @"crop": NSStringFromRect(crop)});
                        return;
                    }
                }
                if (mode != DashboardWindow && !hasPopoverDetails(rep, scale)) {
                    done(@{@"error": @"missing popover header or footer", @"window": NSStringFromRect(item.frame),
                           @"region": NSStringFromRect(config.sourceRect)}); return;
                }
                NSData *png = [rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
                if (![png writeToURL:output atomically:YES]) { done(@{@"error": @"could not save PNG"}); return; }
                done(@{@"width": @(rep.pixelsWide), @"height": @(rep.pixelsHigh)});
            }];
            return;
        }
        done(@{@"pending": @"window has no composited frame"});
    }];
}

static NSDictionary *rectangle(NSRect r) {
    return @{@"x": @(NSMinX(r)), @"y": @(NSMinY(r)), @"width": @(NSWidth(r)), @"height": @(NSHeight(r))};
}

static __weak NSPopover *shownPopover;
static NSPanel *fixtureAnchor;

static NSRect actualViewFrame(NSView *view) {
    NSWindow *window = view.window;
    NSArray *entries = CFBridgingRelease(CGWindowListCopyWindowInfo(kCGWindowListOptionIncludingWindow,
                                                                    (CGWindowID)window.windowNumber));
    NSDictionary *entry = entries.firstObject;
    NSDictionary *bounds = entry[(__bridge NSString *)kCGWindowBounds];
    CGRect frame;
    if (!bounds || !CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)bounds, &frame))
        return NSZeroRect;
    NSRect local = [view convertRect:view.bounds toView:nil];
    CGFloat screenTop = NSMaxY(NSScreen.screens.firstObject.frame);
    return NSMakeRect(frame.origin.x + NSMinX(local),
                      screenTop - CGRectGetMinY(frame) - NSHeight(window.frame) + NSMinY(local),
                      NSWidth(local), NSHeight(local));
}

static NSDictionary *capturePopoverFrame(NSView *view, CaptureMode mode, NSURL *file) {
    __block NSDictionary *result;
    NSRect target = [view.window convertRectToScreen:[view convertRect:view.bounds toView:nil]];
    captureRegion(view.window, target, mode, file, ^(NSDictionary *value) {
        dispatch_async(dispatch_get_main_queue(), ^{ result = value; });
    });
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:3];
    while (!result && deadline.timeIntervalSinceNow > 0)
        [NSRunLoop.mainRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.025]];
    return result ?: @{@"error": @"popover capture timed out"};
}

int QBTestCaptureView(void *rawView, const char *filename) {
    NSView *view = (__bridge NSView *)rawView;
    NSScreen *screen = view.window.screen ?: NSScreen.mainScreen;
    if (!view.window || !screen || !NSThread.isMainThread) return 0;
    NSURL *file = [NSURL fileURLWithFileSystemRepresentation:filename isDirectory:NO relativeToURL:nil];
    NSRect anchored = actualViewFrame(view);
    NSString *method = @"status item · display";
    NSDictionary *result = capturePopoverFrame(view, PopoverOnDisplay, file);
    NSMutableArray *attempts = [NSMutableArray arrayWithObject:result];
    NSPopover *popover = shownPopover;
    if (!result[@"width"] && popover.isShown && popover.contentViewController.view.window == view.window) {
        // AppKit can re-anchor a shown NSPopover without closing it, replacing its
        // content controller or changing its size. An ordinary on-display view
        // avoids the private coordinate system of the hosted menu extra.
        NSRect area = screen.visibleFrame;
        NSRect anchor = NSMakeRect(NSMidX(area) - 10, NSMaxY(area) - 32, 20, 20);
        fixtureAnchor = [[NSPanel alloc] initWithContentRect:anchor
                      styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel
                      backing:NSBackingStoreBuffered defer:NO];
        fixtureAnchor.releasedWhenClosed = NO;
        fixtureAnchor.hidesOnDeactivate = NO;
        fixtureAnchor.hasShadow = NO;
        [fixtureAnchor orderFrontRegardless];
        [popover showRelativeToRect:fixtureAnchor.contentView.bounds ofView:fixtureAnchor.contentView preferredEdge:NSMinYEdge];
        [NSRunLoop.mainRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.4]];
        method = @"center anchor · display";
        result = capturePopoverFrame(view, PopoverOnDisplay, file);
        [attempts addObject:result];
    }
    if (!result[@"width"]) {
        method = @"desktop-independent popover window";
        result = capturePopoverFrame(view, PopoverWindow, file);
        [attempts addObject:result];
    }
    NSDictionary *position = @{@"display": rectangle(screen.frame), @"anchored": rectangle(anchored),
                               @"captured": rectangle(actualViewFrame(view)), @"scale": @(view.window.backingScaleFactor),
                               @"anchoredFullyOnScreen": @(!NSIsEmptyRect(anchored) && NSContainsRect(screen.frame, anchored)),
                               @"method": method, @"attempts": attempts};
    NSURL *geometry = [[file URLByDeletingPathExtension] URLByAppendingPathExtension:@"geometry.json"];
    [[NSJSONSerialization dataWithJSONObject:position options:0 error:NULL] writeToURL:geometry atomically:YES];
    if (!result[@"width"]) NSLog(@"Popover window capture: %@", position);
    return result[@"width"] != nil;
}

static NSArray<NSString *> *names(id element) {
    NSMutableArray *values = [NSMutableArray array];
    if ([element respondsToSelector:@selector(accessibilityLabel)] && [element accessibilityLabel])
        [values addObject:[element accessibilityLabel]];
    if ([element respondsToSelector:@selector(accessibilityTitle)] && [element accessibilityTitle])
        [values addObject:[element accessibilityTitle]];
    if ([element respondsToSelector:@selector(accessibilityValue)]) {
        id value = [element accessibilityValue];
        if ([value isKindOfClass:NSString.class]) [values addObject:value];
    }
    return values;
}

static id control(NSWindow *window, NSString *name, NSMutableArray *available) {
    if (!window.contentView) return nil;
    NSMutableArray *queue = [NSMutableArray arrayWithObject:window.contentView];
    NSHashTable *seen = [NSHashTable hashTableWithOptions:NSPointerFunctionsObjectPointerPersonality];
    for (NSUInteger i = 0; i < queue.count && i < 3000; i++) {
        id node = queue[i];
        if ([seen containsObject:node]) continue;
        [seen addObject:node];
        NSString *role = [node respondsToSelector:@selector(accessibilityRole)] ? [node accessibilityRole] : nil;
        if ([@[NSAccessibilityButtonRole, NSAccessibilityRadioButtonRole, NSAccessibilityCheckBoxRole,
              NSAccessibilityPopUpButtonRole, @"AXSwitch"] containsObject:role ?: @""]) {
            for (NSString *label in names(node)) {
                if ([label isEqual:name] || ([name isEqual:@"계정 연결"] && [label hasPrefix:@"+"] && [label hasSuffix:name]))
                    return node;
                if (available.count < 60) [available addObject:label];
            }
        }
        if ([node respondsToSelector:@selector(accessibilityChildren)])
            [queue addObjectsFromArray:[node accessibilityChildren] ?: @[]];
        if ([node respondsToSelector:@selector(accessibilityChildrenInNavigationOrder)])
            [queue addObjectsFromArray:[node accessibilityChildrenInNavigationOrder] ?: @[]];
        if ([node isKindOfClass:NSView.class]) [queue addObjectsFromArray:[node subviews]];
    }
    return nil;
}

// A SwiftUI hosting view can expose only its AppKit-backed controls without an
// active system accessibility client. Click the visible control in that window.
// Points are in the original reference layout, relative to its content edges.
static BOOL clickControl(NSWindow *window, NSString *label) {
    // The Dock switch occupies the same part of both windows. Give both helpers
    // one down/up pair so an accessibility action cannot introduce a duplicate.
    BOOL sheetSegment = window.sheetParent && [@[@"Codex", @"Claude", @"Gemini", @"브라우저 로그인", @"파일 가져오기",
                                                @"저작권 및 변경 고지", @"Apache License 2.0 전문"] containsObject:label];
    BOOL licenseButton = [label isEqual:@"라이선스 및 고지 보기"] || [label isEqual:@"닫기"] || [label isEqual:@"문서 본문"];
    BOOL popup = [label isEqual:@"Provider 필터"] || [label isEqual:@"자동 새로고침 간격"];
    if (!getenv("QUOTABAR_TEST_REFERENCE_INPUT") && !sheetSegment && !popup && !licenseButton && ![label isEqual:@"Dock에 앱 아이콘 표시"] &&
        ![label hasSuffix:@" 계정 메뉴"]) return NO;
    NSView *view = window.contentView;
    CGFloat w = NSWidth(view.bounds), h = NSHeight(view.bounds), x, y;
    if ([label isEqual:@"계정 연결"]) { x = w - 76; y = 85; }
    else if ([label isEqual:@"다크"]) { x = 609; y = 230; }
    else if ([label isEqual:@"라이트"]) { x = 469; y = 230; }
    else if ([label isEqual:@"아이콘만"]) { x = 609; y = 387; }
    else if ([label isEqual:@"이름 + 잔여량"]) { x = 329; y = 387; }
    else if ([label isEqual:@"Dock에 앱 아이콘 표시"]) { x = w - 83; y = 292; }
    else if (popup) {
        BOOL interval = [label isEqual:@"자동 새로고침 간격"];
        x = interval ? 452 : 602;
        y = (interval ? 569 : 142) + ([NSProcessInfo.processInfo.arguments containsObject:@"--demo"] ? 31 : 0);
    }
    else if ([label hasSuffix:@" 계정 메뉴"]) {
        x = w - 68;
        y = [NSProcessInfo.processInfo.arguments containsObject:@"--demo"] ? 240 : 209;
    }
    else if ([label hasSuffix:@") 모니터링"]) { x = w - 60; y = 251; }
    else if ([label isEqual:@"Codex"]) { x = w * 0.29; y = 116; }
    else if ([label isEqual:@"Claude"]) { x = w * 0.55; y = 116; }
    else if ([label isEqual:@"Gemini"]) { x = w * 0.81; y = 116; }
    else if ([label isEqual:@"브라우저 로그인"]) { x = w * 0.35; y = 158; }
    else if ([label isEqual:@"파일 가져오기"]) { x = w * 0.75; y = 158; }
    else if ([label isEqual:@"라이선스 및 고지 보기"]) { x = 318; y = h - 59; }
    else if ([label isEqual:@"저작권 및 변경 고지"]) { x = w * 0.26; y = 72; }
    else if ([label isEqual:@"Apache License 2.0 전문"]) { x = w * 0.74; y = 72; }
    else if ([label isEqual:@"문서 본문"]) { x = 120; y = 137; }
    else if ([label isEqual:@"닫기"]) { x = w - 49; y = h - 35; }
    else if ([label hasPrefix:@"이 폴더의 statusLine"]) { x = 50; y = h - 99; }
    else if ([label isEqual:@"연결"]) { x = w - 52; y = h - 42; }
    else if ([label isEqual:@"취소"]) { x = w - 103; y = h - 42; }
    else return NO;
    NSPoint point = [view convertPoint:NSMakePoint(x, view.isFlipped ? y : h-y) toView:nil];
    for (NSNumber *kind in @[@(NSEventTypeLeftMouseDown), @(NSEventTypeLeftMouseUp)]) {
        NSEvent *event = [NSEvent mouseEventWithType:kind.unsignedIntegerValue location:point modifierFlags:0
                        timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber context:nil
                        eventNumber:0 clickCount:1 pressure:kind.unsignedIntegerValue == NSEventTypeLeftMouseDown ? 1 : 0];
        [NSApp postEvent:event atStart:NO];
    }
    return YES;
}

static NSDictionary *scrollSettings(NSWindow *window) {
    NSView *view = window.contentView;
    NSPoint local = NSMakePoint(NSMidX(view.bounds), NSMidY(view.bounds));
    NSPoint point = [view convertPoint:local toView:nil];
    NSPoint screen = [window convertPointToScreen:point];
    NSEvent *move = [NSEvent mouseEventWithType:NSEventTypeMouseMoved location:point modifierFlags:0
        timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber context:nil
        eventNumber:0 clickCount:0 pressure:0];
    [window sendEvent:move];
    CGEventRef wheel = CGEventCreateScrollWheelEvent(NULL, kCGScrollEventUnitPixel, 1, -1800);
    if (!wheel) return @{@"error": @"could not create a scroll event"};
    CGEventSetLocation(wheel, CGPointMake(screen.x, NSMaxY(NSScreen.screens.firstObject.frame) - screen.y));
    CGEventSetIntegerValueField(wheel, kCGMouseEventWindowUnderMousePointer, window.windowNumber);
    NSEvent *event = [NSEvent eventWithCGEvent:wheel];
    NSView *hit = [view hitTest:[view convertPoint:local toView:view.superview]] ?: view;
    [hit scrollWheel:event];
    CFRelease(wheel);
    return @{@"scrolled": @YES};
}

static NSScroller *visibleScroller(NSView *view) {
    if (view.hiddenOrHasHiddenAncestor) return nil;
    if ([view isKindOfClass:NSScroller.class]) {
        NSScroller *bar = (NSScroller *)view;
        if (NSHeight(bar.bounds) > 100 && bar.knobProportion > 0 && bar.knobProportion < 1) return bar;
    }
    for (NSView *child in view.subviews) {
        NSScroller *bar = visibleScroller(child);
        if (bar) return bar;
    }
    return nil;
}

static NSDictionary *settingsScroller(NSWindow *window, BOOL page) {
    NSScroller *bar = visibleScroller(window.contentView);
    if (!bar) return @{@"error": @"visible native settings scroller not found"};
    if (!page) {
        double fraction = bar.doubleValue;
        return @{@"position": fraction < 0.001 ? @"top" : fraction > 0.999 ? @"bottom" : @"middle"};
    }
    NSRect knob = [bar convertRect:[bar rectForPart:NSScrollerKnob] toView:nil];
    NSRect track = [bar convertRect:bar.bounds toView:nil];
    NSPoint point = NSMakePoint(NSMidX(knob), (NSMinY(knob) + NSMinY(track)) / 2);
    NSView *hit = [window.contentView hitTest:[window.contentView.superview convertPoint:point fromView:nil]];
    if (hit != bar && ![hit isDescendantOf:bar]) return @{@"error": @"scroller does not receive clicks"};
    for (NSNumber *kind in @[@(NSEventTypeLeftMouseDown), @(NSEventTypeLeftMouseUp)]) {
        NSEvent *event = [NSEvent mouseEventWithType:kind.unsignedIntegerValue location:point modifierFlags:0
            timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber context:nil
            eventNumber:0 clickCount:1 pressure:kind.unsignedIntegerValue == NSEventTypeLeftMouseDown ? 1 : 0];
        [NSApp postEvent:event atStart:NO];
    }
    return @{@"pressed": @YES};
}

// Read the actual composited sheet. Picker titles alone cannot prove that the
// body changed, and SwiftUI's text-selection view does not always expose it to AX.
static NSDictionary *visibleDocument(NSURL *frame) {
    VNRecognizeTextRequest *request = [VNRecognizeTextRequest new];
    request.recognitionLanguages = @[@"en-US"];
    request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
    VNImageRequestHandler *reader = [[VNImageRequestHandler alloc] initWithURL:frame options:@{}];
    NSError *error = nil;
    if (![reader performRequests:@[request] error:&error]) return @{@"error": error.localizedDescription};
    NSMutableString *body = [NSMutableString string];
    for (VNRecognizedTextObservation *line in request.results) {
        NSString *value = [line topCandidates:1].firstObject.string;
        if (value) [body appendString:value.lowercaseString];
    }
    NSString *letters = [[body componentsSeparatedByCharactersInSet:NSCharacterSet.alphanumericCharacterSet.invertedSet]
                         componentsJoinedByString:@""];
    if ([letters containsString:@"copyright2026quotabar"] || [letters containsString:@"previouslydistributedas"])
        return @{@"document": @"NOTICE"};
    if ([letters containsString:@"version20january2004"] || [letters containsString:@"termsandconditionsforuse"])
        return @{@"document": @"LICENSE"};
    return @{@"error": @"could not identify the visible license document", @"recognized": body};
}

static NSDictionary *perform(NSDictionary *request) {
    NSWindow *window = dashboardWindow();
    if (!window) return @{@"pending": @"dashboard window"};
    NSString *op = request[@"op"], *label = request[@"label"];
    if ([op isEqual:@"scrollSettings"]) return scrollSettings(window);
    if ([op isEqual:@"scrollerPosition"] || [op isEqual:@"pageSettings"])
        return settingsScroller(window, [op isEqual:@"pageSettings"]);
    if ([op isEqual:@"document"]) {
        NSURL *dir = [NSURL fileURLWithFileSystemRepresentation:getenv("QUOTABAR_TEST_UI_DIRECTORY") isDirectory:YES relativeToURL:nil];
        return visibleDocument([dir URLByAppendingPathComponent:@"frame.png"]);
    }
    if ([op isEqual:@"size"]) {
        [window setContentSize:NSMakeSize(1120, 810)];
        return @{@"resized": @YES};
    }
    if ([op isEqual:@"sheet"])
        return @{@"sheet": @(window.attachedSheet != nil)};
    if ([op isEqual:@"documentEscape"]) {
        NSWindow *sheet = window.attachedSheet;
        if (!sheet) return @{@"error": @"no document sheet"};
        for (NSNumber *kind in @[@(NSEventTypeKeyDown), @(NSEventTypeKeyUp)]) {
            NSEvent *key = [NSEvent keyEventWithType:kind.unsignedIntegerValue location:NSZeroPoint modifierFlags:0
                timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:sheet.windowNumber context:nil
                characters:@"\x1b" charactersIgnoringModifiers:@"\x1b" isARepeat:NO keyCode:53];
            [NSApp postEvent:key atStart:NO];
        }
        return @{@"key": @"Escape"};
    }
    if ([op isEqual:@"appearance"]) {
        NSString *match = [window.effectiveAppearance bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]];
        return @{@"appearance": [match isEqual:NSAppearanceNameDarkAqua] ? @"dark" : @"light"};
    }
    if ([op isEqual:@"menu"] || [op isEqual:@"shortcut"]) {
        for (NSMenuItem *entry in NSApp.mainMenu.itemArray) {
            NSMenu *menu = entry.submenu;
            NSMenuItem *item = [menu itemWithTitle:label];
            if (!item) continue;
            [menu update];
            if (!item.enabled) return @{@"error": @"disabled menu item", @"label": label};
            NSString *key = item.keyEquivalent;
            NSUInteger modifiers = item.keyEquivalentModifierMask & NSEventModifierFlagDeviceIndependentFlagsMask;
            if ([op isEqual:@"shortcut"]) {
                NSDictionary *codes = @{@"r": @15, @",": @43};
                NSNumber *code = codes[key];
                if (!code) return @{@"error": @"unexpected shortcut", @"key": key};
                NSEvent *event = [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint
                    modifierFlags:modifiers timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber
                    context:nil characters:key charactersIgnoringModifiers:key isARepeat:NO keyCode:code.unsignedShortValue];
                if (![NSApp.mainMenu performKeyEquivalent:event]) return @{@"error": @"shortcut was not handled", @"key": key};
            } else {
                [menu performActionForItemAtIndex:[menu indexOfItem:item]];
            }
            return @{@"label": label, @"key": key, @"modifiers": @(modifiers)};
        }
        return @{@"pending": @"menu item", @"label": label};
    }
    NSMutableArray *available = [NSMutableArray array];
    NSWindow *target = window.attachedSheet ?: window;
    // SwiftUI's segmented picker can expose its enclosing AppKit control for
    // multiple choices. A real mouse click selects the requested segment.
    if ([op isEqual:@"press"] && [@[@"라이트", @"다크", @"아이콘만", @"이름 + 잔여량", @"Dock에 앱 아이콘 표시",
                                    @"Codex", @"Claude", @"Gemini", @"브라우저 로그인", @"파일 가져오기",
                                    @"저작권 및 변경 고지", @"Apache License 2.0 전문"] containsObject:label] &&
        clickControl(target, label)) return @{@"pressed": label};
    id element = control(target, label, available);
    if (!element && ([op isEqual:@"press"] || [op isEqual:@"attempt"]) && clickControl(target, label))
        return @{@"pressed": label};
    if (!element) return @{@"pending": @"control", @"label": label, @"available": available};
    BOOL enabled = ![element respondsToSelector:@selector(isAccessibilityEnabled)] || [element isAccessibilityEnabled];
    if ([op isEqual:@"enabled"]) return @{@"enabled": @(enabled)};
    if ([op isEqual:@"attempt"] && !enabled) return @{@"disabled": label};
    if ((![op isEqual:@"press"] && ![op isEqual:@"attempt"]) || !enabled) return @{@"error": @"control cannot be pressed", @"label": label};
    if ([element respondsToSelector:@selector(accessibilityPerformPress)] && [element accessibilityPerformPress])
        return @{@"pressed": label};
    if ([element respondsToSelector:@selector(accessibilityActionNames)] &&
        [[element accessibilityActionNames] containsObject:NSAccessibilityPressAction]) {
        [element accessibilityPerformAction:NSAccessibilityPressAction];
        return @{@"pressed": label};
    }
    return @{@"error": @"control exposes no press action", @"label": label};
}

static void selectMenuItem(NSDictionary *request, void (^reply)(NSDictionary *)) {
    NSWindow *window = dashboardWindow();
    if (!window) { reply(@{@"pending": @"dashboard window"}); return; }
    NSString *op = request[@"op"];
    BOOL accountMenu = [op hasPrefix:@"account"], interval = [op hasPrefix:@"interval"];
    BOOL cancel = [op isEqual:@"intervalCancel"];
    NSString *control = accountMenu ? request[@"label"] : interval ? @"자동 새로고침 간격" : @"Provider 필터";
    NSString *title = accountMenu ? ([op isEqual:@"accountRefresh"] ? @"새로고침" : @"계정 삭제…") : request[@"label"];
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    __block id observer;
    observer = [center addObserverForName:NSMenuDidBeginTrackingNotification object:nil queue:nil usingBlock:^(NSNotification *notice) {
        NSMenu *menu = notice.object;
        NSArray *known = accountMenu ? @[@"새로고침", @"계정 삭제…"] : interval ? @[@"5분", @"15분"] : @[@"전체 Provider", @"Gemini"];
        for (NSString *entry in known) if (![menu itemWithTitle:entry]) return;
        [NSNotificationCenter.defaultCenter removeObserver:observer];
        observer = nil;
        CFRunLoopPerformBlock(CFRunLoopGetMain(), (__bridge CFStringRef)NSEventTrackingRunLoopMode, ^{
            [menu update];
            NSMenuItem *item = [menu itemWithTitle:title];
            BOOL enabled = item && item.enabled;
            NSMutableArray *choices = [NSMutableArray array], *checked = [NSMutableArray array];
            for (NSMenuItem *entry in menu.itemArray) {
                if (entry.isSeparatorItem || entry.isHidden || !entry.title.length) continue;
                [choices addObject:entry.title];
                if (entry.state == NSControlStateValueOn) [checked addObject:entry.title];
            }
            if (cancel) {
                NSEvent *escape = [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:0
                    timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber context:nil
                    characters:@"\x1b" charactersIgnoringModifiers:@"\x1b" isARepeat:NO keyCode:53];
                [NSApp postEvent:escape atStart:YES];
            } else {
                if (enabled) [menu performActionForItemAtIndex:[menu indexOfItem:item]];
                [menu cancelTracking];
            }
            // A click is only queued by postEvent. Acknowledge the actual
            // selection after AppKit has left its nested menu tracking loop.
            CFRunLoopPerformBlock(CFRunLoopGetMain(), kCFRunLoopDefaultMode, ^{
                if (!enabled) reply(@{@"error": @"disabled menu item", @"label": title});
                else if (accountMenu) reply(@{@"selected": title});
                else reply(@{@"choices": choices, @"checked": checked, @"selection": cancel ? NSNull.null : title});
            });
            CFRunLoopWakeUp(CFRunLoopGetMain());
        });
        CFRunLoopWakeUp(CFRunLoopGetMain());
    }];
    clickControl(window, control);
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 1500 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        if (observer) {
            [center removeObserver:observer];
            observer = nil;
            reply(@{@"pending": @"menu did not begin tracking", @"label": control});
        }
    });
}

__attribute__((constructor)) static void startDashboardDriver(void) {
    if (getenv("QUOTABAR_TEST_POPOVER_FRAME")) {
        @autoreleasepool {
            [NSNotificationCenter.defaultCenter addObserverForName:NSPopoverDidShowNotification object:nil queue:nil
                usingBlock:^(NSNotification *note) { shownPopover = note.object; }];
        }
    }
    const char *path = getenv("QUOTABAR_TEST_UI_DIRECTORY");
    if (!path) return;
    @autoreleasepool {
        NSURL *dir = [NSURL fileURLWithFileSystemRepresentation:path isDirectory:YES relativeToURL:nil];
        NSURL *input = [dir URLByAppendingPathComponent:@"request.json"];
        NSURL *output = [dir URLByAppendingPathComponent:@"reply.json"];
        dispatch_async(dispatch_get_main_queue(), ^{
            if ([NSApp accessibilityIsAttributeSettable:@"AXEnhancedUserInterface"])
                [NSApp accessibilitySetValue:@YES forAttribute:@"AXEnhancedUserInterface"];
            static dispatch_source_t timer;
            __block NSInteger last = 0;
            timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
            dispatch_source_set_timer(timer, DISPATCH_TIME_NOW, 50 * NSEC_PER_MSEC, 5 * NSEC_PER_MSEC);
            dispatch_source_set_event_handler(timer, ^{
                @autoreleasepool {
                    NSData *data = [NSData dataWithContentsOfURL:input];
                    if (!data) return;
                    NSDictionary *request = [NSJSONSerialization JSONObjectWithData:data options:0 error:NULL];
                    NSInteger sequence = [request[@"id"] integerValue];
                    if (sequence <= last) return;
                    last = sequence;
                    void (^reply)(NSDictionary *) = ^(NSDictionary *result) {
                        NSData *encoded = [NSJSONSerialization dataWithJSONObject:@{@"id": @(sequence), @"result": result} options:0 error:NULL];
                        [encoded writeToURL:output atomically:YES];
                    };
                    if ([request[@"op"] isEqual:@"snapshot"]) {
                        NSWindow *window = dashboardWindow();
                        if ([request[@"label"] isEqual:@"sheet"]) window = window.attachedSheet;
                        captureRegion(window.sheetParent ?: window, window.frame, DashboardWindow, [dir URLByAppendingPathComponent:@"frame.png"], reply);
                        return;
                    }
                    if ([@[@"accountRefresh", @"accountRemove", @"providerSelect", @"intervalSelect", @"intervalCancel"] containsObject:request[@"op"]]) {
                        selectMenuItem(request, reply);
                        return;
                    }
                    NSDictionary *result;
                    @try { result = perform(request); }
                    @catch (NSException *e) { result = @{@"error": e.reason ?: e.name}; }
                    reply(result);
                }
            });
            dispatch_resume(timer);
        });
    }
}
