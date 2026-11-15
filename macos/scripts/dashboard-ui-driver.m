// Loaded only by the two CI fixture processes. Query AppKit on their main
// threads so the comparison also works without system-wide Accessibility access.
#import <AppKit/AppKit.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#include <math.h>
#include <stdlib.h>

static NSWindow *dashboardWindow(void) {
    for (NSWindow *window in NSApp.windows)
        if (window.isVisible && !window.sheetParent && [window.title isEqual:@"QuotaBar"]) return window;
    return nil;
}

// Both helpers use this identical WindowServer capture path. Only the current
// process can supply windows, and the ID must match the selected AppKit window.
static void captureRegion(NSWindow *root, NSRect target, NSURL *output, void (^done)(NSDictionary *)) {
    if (!root) { done(@{@"pending": @"window"}); return; }
    CGWindowID number = (CGWindowID)root.windowNumber;
    CGFloat scale = root.backingScaleFactor;
    NSRect frame = root.frame;
    CGRect crop = CGRectMake(round((NSMinX(target)-NSMinX(frame))*scale), round((NSMaxY(frame)-NSMaxY(target))*scale),
                             round(NSWidth(target)*scale), round(NSHeight(target)*scale));
    [SCShareableContent getCurrentProcessShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *error) {
        if (error) { done(@{@"error": error.localizedDescription}); return; }
        for (SCWindow *item in content.windows) {
            if (item.windowID != number) continue;
            SCContentFilter *filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:item];
            SCStreamConfiguration *config = [SCStreamConfiguration new];
            config.width = lround(item.frame.size.width * scale);
            config.height = lround(item.frame.size.height * scale);
            config.showsCursor = NO;
            config.ignoreShadowsSingleWindow = YES;
            config.includeChildWindows = YES;
            config.colorSpaceName = kCGColorSpaceSRGB;
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
                NSData *png = [rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
                if (![png writeToURL:output atomically:YES]) { done(@{@"error": @"could not save PNG"}); return; }
                done(@{@"width": @(rep.pixelsWide), @"height": @(rep.pixelsHigh)});
            }];
            return;
        }
        done(@{@"pending": @"window has no composited frame"});
    }];
}

int QBTestCaptureView(void *rawView, const char *filename) {
    NSView *view = (__bridge NSView *)rawView;
    NSWindow *window = view.window;
    if (!window || !NSThread.isMainThread) return 0;
    NSRect target = [window convertRectToScreen:[view convertRect:view.bounds toView:nil]];
    NSURL *file = [NSURL fileURLWithFileSystemRepresentation:filename isDirectory:NO relativeToURL:nil];
    __block NSDictionary *result;
    captureRegion(window, target, file, ^(NSDictionary *value) {
        dispatch_async(dispatch_get_main_queue(), ^{ result = value; });
    });
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:6];
    while (!result && deadline.timeIntervalSinceNow > 0)
        [NSRunLoop.mainRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.025]];
    if (!result[@"width"]) NSLog(@"Popover window capture: %@", result);
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
static BOOL clickReference(NSWindow *window, NSString *label) {
    if (!getenv("QUOTABAR_TEST_REFERENCE_INPUT")) return NO;
    NSView *view = window.contentView;
    CGFloat w = NSWidth(view.bounds), h = NSHeight(view.bounds), x, y;
    if ([label isEqual:@"계정 연결"]) { x = w - 76; y = 85; }
    else if ([label isEqual:@"Claude"]) { x = w * 0.55; y = 116; }
    else if ([label isEqual:@"Gemini"]) { x = w * 0.81; y = 116; }
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

static NSDictionary *perform(NSDictionary *request) {
    NSWindow *window = dashboardWindow();
    if (!window) return @{@"pending": @"dashboard window"};
    NSString *op = request[@"op"], *label = request[@"label"];
    if ([op isEqual:@"size"]) {
        [window setContentSize:NSMakeSize(1120, 810)];
        return @{@"resized": @YES};
    }
    if ([op isEqual:@"sheet"])
        return @{@"sheet": @(window.attachedSheet != nil)};
    if ([op isEqual:@"menu"]) {
        for (NSMenuItem *entry in NSApp.mainMenu.itemArray) {
            NSMenu *menu = entry.submenu;
            NSMenuItem *item = [menu itemWithTitle:label];
            if (!item) continue;
            [menu update];
            if (!item.enabled) return @{@"error": @"disabled menu item", @"label": label};
            NSString *key = item.keyEquivalent;
            NSUInteger modifiers = item.keyEquivalentModifierMask & NSEventModifierFlagDeviceIndependentFlagsMask;
            [menu performActionForItemAtIndex:[menu indexOfItem:item]];
            return @{@"label": label, @"key": key, @"modifiers": @(modifiers)};
        }
        return @{@"pending": @"menu item", @"label": label};
    }
    NSMutableArray *available = [NSMutableArray array];
    NSWindow *target = window.attachedSheet ?: window;
    id element = control(target, label, available);
    if (!element && ([op isEqual:@"press"] || [op isEqual:@"attempt"]) && clickReference(target, label))
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

__attribute__((constructor)) static void startDashboardDriver(void) {
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
                        captureRegion(window.sheetParent ?: window, window.frame, [dir URLByAppendingPathComponent:@"frame.png"], reply);
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
