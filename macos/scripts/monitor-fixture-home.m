// Loaded only into the two test monitor processes and their home-directory probe.
#import <Foundation/Foundation.h>
#import <objc/runtime.h>
#include <stdlib.h>

static NSString *fixtureHome(void) {
    return [NSString stringWithUTF8String:getenv("QUOTABAR_TEST_MONITOR_HOME")];
}

static NSURL *fixtureHomeURL(id manager, SEL selector) {
    return [NSURL fileURLWithPath:fixtureHome() isDirectory:YES];
}

__attribute__((used, section("__DATA,__interpose")))
static const struct { const void *replacement, *original; } interposeHome = {
    (const void *)fixtureHome, (const void *)NSHomeDirectory
};

__attribute__((constructor)) static void useFixtureHome(void) {
    @autoreleasepool {
        if (!getenv("QUOTABAR_TEST_MONITOR_HOME")) abort();
        Method method = class_getInstanceMethod(NSFileManager.class, @selector(homeDirectoryForCurrentUser));
        if (!method) abort();
        method_setImplementation(method, (IMP)fixtureHomeURL);
    }
}
