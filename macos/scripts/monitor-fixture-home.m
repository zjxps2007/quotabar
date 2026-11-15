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

// Serve scripted HTTPS responses in both unmodified monitor processes. Only
// fixture requests are accepted; the traces record operation names and statuses.
@interface QuotaBarFixtureRelease : NSURLProtocol
@end
@implementation QuotaBarFixtureRelease
+ (BOOL)canInitWithRequest:(NSURLRequest *)request {
    NSString *scheme = request.URL.scheme;
    return [scheme isEqualToString:@"https"] || [scheme isEqualToString:@"http"];
}
+ (NSURLRequest *)canonicalRequestForRequest:(NSURLRequest *)request { return request; }
- (void)startLoading {
    NSString *endpoint = @"https://api.github.com/repos/zjxps2007/quotabar/releases/latest";
    NSString *url = self.request.URL.absoluteString;
    BOOL release = [url isEqual:endpoint];
    BOOL asset = [url hasPrefix:@"https://github.com/zjxps2007/quotabar/releases/download/"];
    NSString *file = release ? @"release-response.json" : asset ? @"asset-responses.json" : @"provider-responses.json";
    NSData *source = [NSData dataWithContentsOfFile:[fixtureHome() stringByAppendingPathComponent:file]];
    NSDictionary *fixture = source ? [NSJSONSerialization JSONObjectWithData:source options:0 error:NULL] : nil;
    if (release || asset) {
        if (asset) fixture = fixture[url];
        NSString *agent = [self.request valueForHTTPHeaderField:@"User-Agent"];
        BOOL expected = release ? [agent hasPrefix:@"QuotaBar/"] && [[self.request valueForHTTPHeaderField:@"Accept"] isEqual:@"application/vnd.github+json"]
                                : [agent isEqual:@"QuotaBar-Updater"];
        if (!expected || ![self.request.HTTPMethod isEqual:@"GET"] || [self.request valueForHTTPHeaderField:@"Authorization"] ||
            [self.request valueForHTTPHeaderField:@"Cookie"]) fixture = nil;
    } else {
        fixture = fixture[url];
        NSString *bearer = [@"Bearer " stringByAppendingString:fixture[@"token"] ?: @""];
        if (![self.request.HTTPMethod isEqual:fixture[@"method"]] ||
            ![[self.request valueForHTTPHeaderField:@"Authorization"] isEqual:bearer] ||
            (fixture[@"account"] && ![[self.request valueForHTTPHeaderField:@"ChatGPT-Account-Id"] isEqual:fixture[@"account"]])) {
            fixture = nil;
        }
    }
    if (![fixture[@"status"] isKindOfClass:NSNumber.class]) {
        [self.client URLProtocol:self didFailWithError:[NSError errorWithDomain:NSURLErrorDomain code:NSURLErrorBadURL userInfo:nil]];
        return;
    }
    NSString *log = [fixtureHome() stringByAppendingPathComponent:(release ? @"release-requests.log" : asset ? @"asset-requests.log" : @"provider-requests.log")];
    NSString *line = release ? [fixture[@"status"] stringValue] : [NSString stringWithFormat:@"%@ %@", fixture[@"name"], fixture[@"status"]];
    @synchronized (QuotaBarFixtureRelease.class) {
        NSFileHandle *handle = [NSFileHandle fileHandleForWritingAtPath:log];
        [handle seekToEndOfFile];
        [handle writeData:[[line stringByAppendingString:@"\n"] dataUsingEncoding:NSUTF8StringEncoding]];
        [handle closeFile];
    }
    NSData *body = asset ? [NSData dataWithContentsOfFile:[[fixtureHome() stringByAppendingPathComponent:@"update-test"] stringByAppendingPathComponent:fixture[@"file"]]]
                         : [NSJSONSerialization dataWithJSONObject:fixture[@"body"] options:0 error:NULL];
    NSHTTPURLResponse *response = [[NSHTTPURLResponse alloc] initWithURL:self.request.URL
        statusCode:[fixture[@"status"] integerValue] HTTPVersion:@"HTTP/1.1"
        headerFields:@{@"Content-Type": asset ? @"application/octet-stream" : @"application/json", @"Content-Length": [@(body.length) stringValue]}];
    [self.client URLProtocol:self didReceiveResponse:response cacheStoragePolicy:NSURLCacheStorageNotAllowed];
    // Exercise more than one data-delegate callback without using the network.
    for (NSUInteger offset = 0; offset < body.length; offset += 8192) {
        [self.client URLProtocol:self didLoadData:[body subdataWithRange:NSMakeRange(offset, MIN(8192, body.length-offset))]];
    }
    [self.client URLProtocolDidFinishLoading:self];
}
- (void)stopLoading {}
@end

@interface NSURLSessionConfiguration (QuotaBarFixture)
+ (instancetype)qb_fixtureEphemeralConfiguration;
@end
@implementation NSURLSessionConfiguration (QuotaBarFixture)
+ (instancetype)qb_fixtureEphemeralConfiguration {
    NSURLSessionConfiguration *configuration = [self qb_fixtureEphemeralConfiguration];
    configuration.protocolClasses = [@[QuotaBarFixtureRelease.class] arrayByAddingObjectsFromArray:configuration.protocolClasses ?: @[]];
    return configuration;
}
@end

__attribute__((constructor)) static void useFixtureHome(void) {
    @autoreleasepool {
        if (!getenv("QUOTABAR_TEST_MONITOR_HOME")) abort();
        Method method = class_getInstanceMethod(NSFileManager.class, @selector(homeDirectoryForCurrentUser));
        if (!method) abort();
        method_setImplementation(method, (IMP)fixtureHomeURL);
        Method ephemeral = class_getClassMethod(NSURLSessionConfiguration.class, @selector(ephemeralSessionConfiguration));
        Method fixture = class_getClassMethod(NSURLSessionConfiguration.class, @selector(qb_fixtureEphemeralConfiguration));
        if (!ephemeral || !fixture) abort();
        method_exchangeImplementations(ephemeral, fixture);
    }
}
