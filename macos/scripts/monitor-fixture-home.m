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

// Serve the actual ReleaseClient's HTTPS requests from the fixture home, including
// error statuses. Neither the original monitor nor the Rust monitor needs a release
// or a network connection to exercise manual checks in its normal process.
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
    NSData *source = [NSData dataWithContentsOfFile:[fixtureHome() stringByAppendingPathComponent:@"release-response.json"]];
    NSDictionary *fixture = source ? [NSJSONSerialization JSONObjectWithData:source options:0 error:NULL] : nil;
    if (![self.request.URL.absoluteString isEqualToString:endpoint] || ![fixture[@"status"] isKindOfClass:NSNumber.class]) {
        [self.client URLProtocol:self didFailWithError:[NSError errorWithDomain:NSURLErrorDomain code:NSURLErrorBadURL userInfo:nil]];
        return;
    }
    NSString *log = [fixtureHome() stringByAppendingPathComponent:@"release-requests.log"];
    NSFileHandle *handle = [NSFileHandle fileHandleForWritingAtPath:log];
    [handle seekToEndOfFile];
    [handle writeData:[[NSString stringWithFormat:@"%@\n", fixture[@"status"]] dataUsingEncoding:NSUTF8StringEncoding]];
    [handle closeFile];
    NSData *body = [NSJSONSerialization dataWithJSONObject:fixture[@"body"] options:0 error:NULL];
    NSHTTPURLResponse *response = [[NSHTTPURLResponse alloc] initWithURL:self.request.URL
        statusCode:[fixture[@"status"] integerValue] HTTPVersion:@"HTTP/1.1"
        headerFields:@{@"Content-Type": @"application/json", @"Content-Length": [@(body.length) stringValue]}];
    [self.client URLProtocol:self didReceiveResponse:response cacheStoragePolicy:NSURLCacheStorageNotAllowed];
    [self.client URLProtocol:self didLoadData:body];
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
