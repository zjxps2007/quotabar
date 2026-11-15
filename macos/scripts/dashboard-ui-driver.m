// Loaded only by the two CI fixture processes. Query AppKit on their main
// threads so the comparison also works without system-wide Accessibility access.
#import <AppKit/AppKit.h>
#include <stdlib.h>

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
        if ([node isKindOfClass:NSView.class]) [queue addObjectsFromArray:[node subviews]];
    }
    return nil;
}

static NSDictionary *perform(NSDictionary *request) {
    NSWindow *window = nil;
    for (NSWindow *w in NSApp.windows) {
        if (w.isVisible && !w.sheetParent && [w.title isEqual:@"QuotaBar"]) { window = w; break; }
    }
    if (!window) return @{@"pending": @"dashboard window"};
    NSString *op = request[@"op"], *label = request[@"label"];
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
    id element = control(window.attachedSheet ?: window, label, available);
    if (!element) return @{@"pending": @"control", @"label": label, @"available": available};
    BOOL enabled = ![element respondsToSelector:@selector(isAccessibilityEnabled)] || [element isAccessibilityEnabled];
    if ([op isEqual:@"enabled"]) return @{@"enabled": @(enabled)};
    if (![op isEqual:@"press"] || !enabled) return @{@"error": @"control cannot be pressed", @"label": label};
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
                    NSDictionary *result;
                    @try { result = perform(request); }
                    @catch (NSException *e) { result = @{@"error": e.reason ?: e.name}; }
                    NSData *reply = [NSJSONSerialization dataWithJSONObject:@{@"id": @(sequence), @"result": result} options:0 error:NULL];
                    [reply writeToURL:output atomically:YES];
                }
            });
            dispatch_resume(timer);
        });
    }
}
